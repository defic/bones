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
/// per session, not once per clone). `labels` parallels `plugins` (same order) — human names
/// for diagnostics like the per-script [`StageTimings`] rows. `SkipSerialize`: runtime state
/// every peer builds itself.
#[derive(HasSchema, Clone, Default)]
#[schema(opaque)]
#[type_data(SkipSerialize)]
pub struct ScriptPlugins {
    pub plugins: Arc<Vec<Arc<LuaPlugin>>>,
    pub labels: Arc<Vec<String>>,
}

struct SourceSlot {
    /// The ORDERED `(source, label)`s pending for the next session. `None` = "use the game's
    /// default"; `Some(vec)` = exactly these (each file becomes its own plugin, in order —
    /// scripts self-schedule onto stages, files are never flattened together).
    pending: Option<Vec<(String, String)>>,
    active: Option<Arc<Vec<(String, String)>>>,
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
    push_active_source_labeled(bytes, "")
}

/// [`push_active_source`] with a human label for diagnostics (per-script timing rows, logs).
/// An empty label falls back to `file-<index>`.
pub fn push_active_source_labeled(bytes: Vec<u8>, label: &str) -> bool {
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
    let pending = slot.pending.get_or_insert_with(Vec::new);
    let label = if label.is_empty() { format!("file-{}", pending.len()) } else { label.to_string() };
    pending.push((source, label));
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
                let s = Arc::new(vec![(default_source.to_string(), "default".to_string())]);
                slot.active = Some(s.clone());
                s
            }
        } else {
            let s = Arc::new(match slot.pending.take() {
                Some(sources) if !sources.is_empty() => sources,
                _ => vec![(default_source.to_string(), "default".to_string())],
            });
            slot.active = Some(s.clone());
            slot.consumed = true;
            s
        }
    };
    ScriptPlugins {
        plugins: Arc::new(
            sources
                .iter()
                .map(|(source, _)| {
                    Arc::new(LuaPlugin {
                        source: source.clone(),
                        systems: Arc::new(AtomicCell::new(Default::default())),
                    })
                })
                .collect(),
        ),
        labels: Arc::new(sources.iter().map(|(_, label)| label.clone()).collect()),
    }
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
        // Named explicitly: the closure would otherwise report THIS function's name in the
        // per-system timing table, reading as if "install" ran every frame.
        let mut runner = IntoSystem::system(
            move |engine: Res<LuaEngine>, scripts: Res<ScriptPlugins>, world: &World| {
                // Dormant fast path: the `engine.exec` hop costs microseconds×allocs, so skip it
                // unless some plugin actually needs this stage (still loading, pending startup,
                // or has a closure registered here).
                let stage_has_work = scripts.plugins.iter().any(|p| {
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
                // Per-script timing (see bones_ecs `StageTimings`): only when an enabled
                // resource is present, and recorded AFTER the VM scope ends (the resource
                // must not be borrowed while scripts run).
                let timing = world
                    .resources
                    .get::<StageTimings>()
                    .map(|t| t.enabled)
                    .unwrap_or(false);
                let mut plugin_ns: Vec<u64> = if timing {
                    vec![0; scripts.plugins.len()]
                } else {
                    Vec::new()
                };
                engine.exec(|lua| {
                    Frozen::<Freeze![&'freeze World]>::in_scope(world, |world| {
                        lua.enter(|ctx| {
                            let env = ctx.singletons().get(ctx, bindings::env);
                            let worldref = WorldRef(world);
                            worldref.add_to_env(ctx, env);
                        });

                        for (plugin_idx, plugin) in scripts.plugins.iter().enumerate() {
                            let t0 = timing.then(std::time::Instant::now);
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
                            if let Some(t0) = t0 {
                                plugin_ns[plugin_idx] += t0.elapsed().as_nanos() as u64;
                            }
                        }
                    })
                });
                if timing {
                    if let Some(mut t) = world.resources.get_mut::<StageTimings>() {
                        let stage_name = lua_stage.name();
                        for (i, ns) in plugin_ns.iter().enumerate() {
                            if *ns > 0 {
                                let label = scripts
                                    .labels
                                    .get(i)
                                    .map(|l| l.as_str())
                                    .unwrap_or("?");
                                t.record(&stage_name, &format!("lua:{label}"), *ns);
                            }
                        }
                    }
                }
            },
        );
        runner.name = "lua_runner";
        builder.add_system_to_stage(lua_stage, runner);
    }
}

/// One-shot console chunks queued by the host — e.g. an admin "eval" arriving as a REPLICATED
/// game input, pushed here by the host's resolve system and drained the same run by the
/// runner from [`install_eval_runner`]. `SkipSerialize` scratch: determinism comes from the
/// chunks being replicated inputs (every peer queues the same sources at the same tick), not
/// from this resource riding the snapshot.
#[derive(HasSchema, Clone, Default)]
#[schema(opaque)]
#[type_data(SkipSerialize)]
pub struct EvalQueue(pub Vec<String>);

/// One formatted result line per executed [`EvalQueue`] chunk, in execution order — every
/// peer computes them locally (same chunks, same state), so the server can answer the admin
/// request AND an in-client console can render them without any extra wire traffic. Capped
/// FIFO (a peer with no console just cycles the buffer). `SkipSerialize`: presentation.
#[derive(HasSchema, Clone, Default)]
#[schema(opaque)]
#[type_data(SkipSerialize)]
pub struct EvalResults(pub Vec<String>);

/// Most results a peer keeps before dropping the oldest.
const EVAL_RESULTS_CAP: usize = 32;

/// Game-defined console prelude: Lua PREPENDED to every [`eval_chunk`] source at COMPILE time
/// (never on the wire — a replicated console input stays just the user's line). The intended
/// use is `local` aliases that make console lines terse:
/// `local tune = resources:get(s("Tune"))` → users type `tune.race.laps = 5`.
/// Insert it as a resource at session build; identical on every peer by construction (it's
/// compiled-in game code). `SkipSerialize`: static config, not state.
#[derive(HasSchema, Clone, Default)]
#[schema(opaque)]
#[type_data(SkipSerialize)]
pub struct EvalPrelude(pub String);

/// Format a Lua value for the console: primitives natively; tables/functions/userdata as a
/// type tag (calling `__tostring` metamethods would need another executor pump — reflected
/// field reads land here as primitives anyway, which is the case that matters).
fn fmt_lua_value(v: &crate::lua::piccolo::Value) -> String {
    use crate::lua::piccolo::Value;
    match v {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        Value::Table(_) => "<table>".to_string(),
        Value::Function(_) => "<function>".to_string(),
        Value::Thread(_) => "<thread>".to_string(),
        Value::UserData(_) => "<userdata>".to_string(),
    }
}

/// Compile + execute ONE console chunk against `world` (which must carry a [`LuaEngine`]
/// resource) and return its formatted result line. An expression chunk is auto-wrapped as
/// `return <src>` (fallback: plain statement chunk, result "ok"), so `…race.laps` answers
/// with the value. Errors come back as `error: …` — never a panic.
///
/// Two callers, two determinism postures:
/// - the replicated eval runner (below) — every peer runs the same chunk on the same state;
/// - a host's read-only QUERY path — run it on a CLONE of the state, so a chunk that
///   accidentally writes mutates the discarded clone, not the live sim.
pub fn eval_chunk(world: &World, src: &str) -> String {
    let Some(engine) = world.resources.get::<LuaEngine>() else {
        return "error: no LuaEngine in world".to_string();
    };
    let prelude = world
        .resources
        .get::<EvalPrelude>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let mut result = String::new();
    engine.exec(|lua| {
        Frozen::<Freeze![&'freeze World]>::in_scope(world, |world| {
            lua.enter(|ctx| {
                let env = ctx.singletons().get(ctx, bindings::env);
                let worldref = WorldRef(world);
                worldref.add_to_env(ctx, env);
            });
            // Game prelude (alias locals), then: expression first (captures a return value),
            // statement as fallback.
            let with_return = format!("{prelude}\nreturn {src}");
            let plain = format!("{prelude}\n{src}");
            let executor = lua.try_enter(|ctx| {
                let env = ctx.singletons().get(ctx, bindings::env);
                let closure = crate::lua::piccolo::Closure::load_with_env(
                    ctx,
                    None,
                    with_return.as_bytes(),
                    env,
                )
                .or_else(|_| {
                    crate::lua::piccolo::Closure::load_with_env(ctx, None, plain.as_bytes(), env)
                })?;
                let ex = crate::lua::piccolo::Executor::start(ctx, closure.into(), ());
                Ok(ctx.registry().stash(&ctx, ex))
            });
            let line = executor.and_then(|ex| {
                lua.finish(&ex);
                lua.try_enter(|ctx| {
                    let ex = ctx.registry().fetch(&ex);
                    let vals = ex.take_result::<crate::lua::piccolo::Variadic<
                        Vec<crate::lua::piccolo::Value>,
                    >>(ctx)??;
                    Ok(if vals.is_empty() {
                        "ok".to_string()
                    } else {
                        vals.iter().map(fmt_lua_value).collect::<Vec<_>>().join(", ")
                    })
                })
            });
            result = match line {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("lua eval error: {e}");
                    format!("error: {e}")
                }
            };
        })
    });
    result
}

/// Register the console-eval runner: drains [`EvalQueue`], executing each chunk via
/// [`eval_chunk`] and pushing one result line per chunk into [`EvalResults`]. Install it
/// AFTER the host's resolve systems on the same stage, so a chunk queued while applying a
/// batch runs inside that same authoritative step on every peer. A bad chunk becomes an
/// `error: …` line — never kills the sim, and errors identically on every peer.
pub fn install_eval_runner(builder: &mut SystemStagesBuilder) {
    let mut runner = IntoSystem::system(
        move |world: &World| {
            let chunks: Vec<String> = {
                let Some(mut q) = world.resources.get_mut::<EvalQueue>() else {
                    return;
                };
                if q.0.is_empty() {
                    return;
                }
                std::mem::take(&mut q.0)
            };
            let results: Vec<String> =
                chunks.iter().map(|src| eval_chunk(world, src)).collect();
            if let Some(mut r) = world.resources.get_mut::<EvalResults>() {
                r.0.extend(results);
                let overflow = r.0.len().saturating_sub(EVAL_RESULTS_CAP);
                if overflow > 0 {
                    r.0.drain(..overflow);
                }
            }
        },
    );
    runner.name = "lua_eval_console";
    builder.add_system_to_stage(CoreStage::Update, runner);
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
    let mut runner = IntoSystem::system(
        move |engine: Res<LuaEngine>, scripts: Res<ScriptPlugins>, world: &World| {
            // Dormant fast path: no plugin has resolve hooks (or still needs loading) → no VM hop.
            let has_work = scripts.plugins.iter().any(|p| {
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
            let timing = world
                .resources
                .get::<StageTimings>()
                .map(|t| t.enabled)
                .unwrap_or(false);
            let mut plugin_ns: Vec<u64> =
                if timing { vec![0; scripts.plugins.len()] } else { Vec::new() };
            engine.exec(|lua| {
                Frozen::<Freeze![&'freeze World]>::in_scope(world, |world| {
                    lua.enter(|ctx| {
                        let env = ctx.singletons().get(ctx, bindings::env);
                        let worldref = WorldRef(world);
                        worldref.add_to_env(ctx, env);
                    });

                    for (plugin_idx, plugin) in scripts.plugins.iter().enumerate() {
                        let t0 = timing.then(std::time::Instant::now);
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
                        if let Some(t0) = t0 {
                            plugin_ns[plugin_idx] += t0.elapsed().as_nanos() as u64;
                        }
                    }
                })
            });
            if timing {
                if let Some(mut t) = world.resources.get_mut::<StageTimings>() {
                    for (i, ns) in plugin_ns.iter().enumerate() {
                        if *ns > 0 {
                            let label =
                                scripts.labels.get(i).map(|l| l.as_str()).unwrap_or("?");
                            t.record("resolve", &format!("lua:{label}"), *ns);
                        }
                    }
                }
            }
        },
    );
    runner.name = "lua_resolve_hooks";
    builder.add_system_to_stage(CoreStage::Last, runner);
}
