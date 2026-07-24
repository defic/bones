//! Embedded / runtime-installed script plugins WITHOUT the asset server — for games that drive
//! bare [`SystemStages`] (e.g. under an external netcode loop) and distribute script sources
//! themselves (compiled-in defaults + content-addressed downloads).
//!
//! Flow (mirrors an ADR-0005-style "slot"): the host calls [`set_active_source`] with verified
//! bytes BEFORE constructing a session (server boot / client prefetch), then builds the session's
//! [`ScriptPlugins`] resource with [`plugins_for_session`] (which falls back to the game's
//! compiled-in default source) and installs the per-stage runners with [`install_lua_runners`].
//!
//! Determinism rules for scripts running inside a predicted, rolled-back tick:
//! - every peer must run byte-identical sources (content-address them!);
//! - ALL persistent state lives in reflected ECS resources/components — the Lua VM is never
//!   snapshotted, so Lua globals are per-tick scratch at best.

use std::sync::{Arc, Mutex, Once, OnceLock};

use bones_ecs::prelude::*;

use crate::lua::{
    bindings, CtxExt, Freeze, Frozen, LuaEngine, LuaPlugin, LuaPluginSystemsState, WorldRef,
};

/// The loaded Lua plugins for a session, in registration order. Shared `Arc` so rollback clones
/// reuse the same compiled plugins (their `systems` cell is shared — startup closures run once
/// per session, not once per clone). `SkipSerialize`: runtime state every peer builds itself.
#[derive(HasSchema, Clone, Default)]
#[schema(opaque)]
#[type_data(SkipSerialize)]
pub struct ScriptPlugins(pub Arc<Vec<Arc<LuaPlugin>>>);

struct SourceSlot {
    /// The ORDERED sources pending for the next session. `None` = "use the game's default";
    /// `Some(vec)` = exactly these (each file becomes its own plugin, in order — scripts
    /// self-schedule onto stages, files are never flattened together).
    pending: Option<Vec<String>>,
    active: Option<Arc<Vec<String>>>,
    consumed: bool,
}

fn slot() -> &'static Mutex<SourceSlot> {
    static SLOT: OnceLock<Mutex<SourceSlot>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(SourceSlot { pending: None, active: None, consumed: false }))
}

/// Append one runtime script source for the NEXT session (call once per file, in run order —
/// the client prefetch and the server boot both install files individually so each stays its
/// own cacheable, content-addressed asset). Returns `false` if a session already baked the
/// pending set (the install came too late — surface it, peers may diverge). Non-UTF-8 rejected.
pub fn push_active_source(bytes: Vec<u8>) -> bool {
    let Ok(source) = String::from_utf8(bytes) else {
        tracing::error!("push_active_source: script asset is not UTF-8 — ignoring");
        return false;
    };
    let mut slot = slot().lock().unwrap();
    let in_time = !slot.consumed;
    if slot.consumed {
        // A stale pending set must not leak into the next install sequence.
        slot.pending = None;
        slot.consumed = false;
    }
    slot.pending.get_or_insert_with(Vec::new).push(source);
    in_time
}

/// Replace the pending set with exactly one source (single-script convenience).
pub fn set_active_source(bytes: Vec<u8>) -> bool {
    {
        let mut slot = slot().lock().unwrap();
        slot.pending = None;
        slot.consumed = false;
    }
    push_active_source(bytes)
}

/// Back to the game's compiled-in default source for the NEXT session.
pub fn reset_active_source() {
    let mut slot = slot().lock().unwrap();
    slot.pending = None;
    slot.consumed = false;
}

/// Commit the pending source set for this session (`default_source` when none installed) and
/// build the session's [`ScriptPlugins`] — one plugin per file, in install order. Also installs
/// (once per process) the lua type data the stock session plugin would have registered —
/// without it `Entities` has no `iter_with` metatable and every script query fails.
pub fn plugins_for_session(default_source: &str) -> ScriptPlugins {
    static LUA_TYPEDATA: Once = Once::new();
    LUA_TYPEDATA.call_once(bindings::register_lua_typedata);

    let sources = {
        let mut slot = slot().lock().unwrap();
        if slot.consumed {
            if let Some(active) = &slot.active {
                active.clone()
            } else {
                let s = Arc::new(vec![default_source.to_string()]);
                slot.active = Some(s.clone());
                s
            }
        } else {
            let s = Arc::new(match slot.pending.take() {
                Some(sources) if !sources.is_empty() => sources,
                _ => vec![default_source.to_string()],
            });
            slot.active = Some(s.clone());
            slot.consumed = true;
            s
        }
    };
    ScriptPlugins(Arc::new(
        sources
            .iter()
            .map(|source| {
                Arc::new(LuaPlugin {
                    source: source.clone(),
                    systems: Arc::new(AtomicCell::new(Default::default())),
                })
            })
            .collect(),
    ))
}

/// Register a Lua runner system at the END of every core stage of `builder` (after that stage's
/// host systems, by insertion order). Fork-variant of the stock `LuaPluginLoaderSessionPlugin`
/// body, reading plugins from the [`ScriptPlugins`] resource instead of asset handles.
pub fn install_lua_runners(builder: &mut SystemStagesBuilder) {
    for lua_stage in [
        CoreStage::First,
        CoreStage::PreUpdate,
        CoreStage::Update,
        CoreStage::PostUpdate,
        CoreStage::Last,
    ] {
        builder.add_system_to_stage(
            lua_stage,
            move |engine: Res<LuaEngine>, scripts: Res<ScriptPlugins>, world: &World| {
                // Dormant fast path: the `engine.exec` hop costs microseconds×allocs, so skip it
                // unless some plugin actually needs this stage (still loading, pending startup,
                // or has a closure registered here).
                let stage_has_work = scripts.0.iter().any(|p| {
                    let systems = p.systems.borrow();
                    match &*systems {
                        LuaPluginSystemsState::NotLoaded => true,
                        LuaPluginSystemsState::Loaded { .. } => {
                            let s = systems.as_loaded();
                            s.startup.iter().any(|(has_run, _)| !has_run)
                                || s.core_stages.iter().any(|(stage, _)| *stage == lua_stage)
                        }
                        LuaPluginSystemsState::Unloaded => false,
                    }
                });
                if !stage_has_work {
                    return;
                }
                engine.exec(|lua| {
                    Frozen::<Freeze![&'freeze World]>::in_scope(world, |world| {
                        lua.enter(|ctx| {
                            let env = ctx.singletons().get(ctx, bindings::env);
                            let worldref = WorldRef(world);
                            worldref.add_to_env(ctx, env);
                        });

                        for plugin in scripts.0.iter() {
                            if !plugin.has_loaded() {
                                if let Err(e) = plugin.load(engine.executor().clone(), lua) {
                                    eprintln!("lua plugin load error: {e}");
                                    tracing::error!("Error loading lua plugin: {e}");
                                }
                            }
                            if matches!(&*plugin.systems.borrow(), LuaPluginSystemsState::NotLoaded)
                            {
                                continue;
                            }

                            let mut systems = plugin.systems.borrow_mut();
                            let systems = systems.as_loaded_mut();

                            for (has_run, closure) in &mut systems.startup {
                                if !*has_run {
                                    let executor = lua.enter(|ctx| {
                                        let closure = ctx.registry().fetch(closure);
                                        let ex = crate::lua::piccolo::Executor::start(
                                            ctx,
                                            closure.into(),
                                            (),
                                        );
                                        ctx.registry().stash(&ctx, ex)
                                    });
                                    if let Err(e) = lua.execute::<()>(&executor) {
                                        eprintln!("lua startup system error: {e}");
                                        tracing::error!("Error running lua startup system: {e}");
                                    }
                                    *has_run = true;
                                }
                            }

                            for (stage, closure) in &systems.core_stages {
                                if stage == &lua_stage {
                                    let executor = lua.enter(|ctx| {
                                        let closure = ctx.registry().fetch(closure);
                                        let ex = crate::lua::piccolo::Executor::start(
                                            ctx,
                                            closure.into(),
                                            (),
                                        );
                                        ctx.registry().stash(&ctx, ex)
                                    });
                                    if let Err(e) = lua.execute::<()>(&executor) {
                                        eprintln!("lua system error: {e}");
                                        tracing::error!("Error running lua system: {e}");
                                    }
                                }
                            }
                        }
                    })
                });
            },
        );
    }
}

/// Register the CONFIRMED-phase Lua runner: appended after the host's resolve system(s), it
/// executes every plugin's `add_resolve_system` closures once per authoritative batch, in
/// plugin order — post-application, so hooks see the batch's effects (joins applied, kills
/// logged) atomically. Effects are confirmed-tick-late and never mispredicted.
///
/// v1 contract: resolve hooks read/write reflected state directly; outbound channels the game
/// drains after its SIMULATE stages (e.g. a damage queue) are NOT re-drained here — pushing
/// into them from a resolve hook does nothing until the game adds a resolve-side pipeline.
pub fn install_resolve_lua_runner(builder: &mut SystemStagesBuilder) {
    builder.add_system_to_stage(
        CoreStage::Last,
        move |engine: Res<LuaEngine>, scripts: Res<ScriptPlugins>, world: &World| {
            // Dormant fast path: no plugin has resolve hooks (or still needs loading) → no VM hop.
            let has_work = scripts.0.iter().any(|p| {
                let systems = p.systems.borrow();
                match &*systems {
                    LuaPluginSystemsState::NotLoaded => true,
                    LuaPluginSystemsState::Loaded { .. } => {
                        !systems.as_loaded().resolve_systems.is_empty()
                    }
                    LuaPluginSystemsState::Unloaded => false,
                }
            });
            if !has_work {
                return;
            }
            engine.exec(|lua| {
                Frozen::<Freeze![&'freeze World]>::in_scope(world, |world| {
                    lua.enter(|ctx| {
                        let env = ctx.singletons().get(ctx, bindings::env);
                        let worldref = WorldRef(world);
                        worldref.add_to_env(ctx, env);
                    });

                    for plugin in scripts.0.iter() {
                        // Lazy-load here too: a resolve batch can arrive before the first
                        // simulate tick (server boot joins).
                        if !plugin.has_loaded() {
                            if let Err(e) = plugin.load(engine.executor().clone(), lua) {
                                eprintln!("lua plugin load error: {e}");
                                tracing::error!("Error loading lua plugin: {e}");
                            }
                        }
                        if matches!(&*plugin.systems.borrow(), LuaPluginSystemsState::NotLoaded) {
                            continue;
                        }
                        let systems = plugin.systems.borrow();
                        let systems = systems.as_loaded();
                        for closure in &systems.resolve_systems {
                            let executor = lua.enter(|ctx| {
                                let closure = ctx.registry().fetch(closure);
                                let ex =
                                    crate::lua::piccolo::Executor::start(ctx, closure.into(), ());
                                ctx.registry().stash(&ctx, ex)
                            });
                            if let Err(e) = lua.execute::<()>(&executor) {
                                eprintln!("lua resolve system error: {e}");
                                tracing::error!("Error running lua resolve system: {e}");
                            }
                        }
                    }
                })
            });
        },
    );
}
