pub mod embedded;
pub mod lua;

#[cfg(not(target_os = "emscripten"))]
use bones_asset::UntypedHandle;
#[cfg(not(target_os = "emscripten"))]
use bones_lib::prelude::*;

/// The prelude.
pub mod prelude {
    pub use super::lua::*;
    #[cfg(not(target_os = "emscripten"))]
    pub use super::ScriptingGamePlugin;
    #[cfg(not(target_os = "emscripten"))]
    pub(crate) use bones_asset::prelude::*;
    #[cfg(not(target_os = "emscripten"))]
    pub(crate) use bones_lib::prelude::*;
    // Emscripten builds have no asset machinery (see Cargo.toml) — the ECS prelude supplies
    // everything the lua machinery needs there.
    #[cfg(target_os = "emscripten")]
    pub(crate) use bones_ecs::prelude::*;
}

/// Scripting plugin for the bones framework.
#[cfg(not(target_os = "emscripten"))]
pub struct ScriptingGamePlugin {
    pub enable_lua: bool,
}

#[cfg(not(target_os = "emscripten"))]
impl Default for ScriptingGamePlugin {
    fn default() -> Self {
        Self { enable_lua: true }
    }
}

#[cfg(not(target_os = "emscripten"))]
impl GamePlugin for ScriptingGamePlugin {
    fn install(self, game: &mut Game) {
        UntypedHandle::register_schema();

        if self.enable_lua {
            game.install_plugin(lua::lua_game_plugin);
        }
    }
}
