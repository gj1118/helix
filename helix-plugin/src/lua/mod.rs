use crate::error::{PluginError, Result};
use crate::types::{EventType, PluginEvent};
use helix_view::{Document, Editor, View};
use mlua::prelude::*;
use mlua::{ffi, RegistryKey};
use parking_lot::RwLock;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Editor access
//
// Invariant: at most one `&mut Editor` derived from `CURRENT_EDITOR` is alive
// at any time, and it never outlives the `&mut Editor` handed to
// `with_editor_context`.
//
// * `with_editor_context` publishes a pointer to an editor the caller has
//   exclusively borrowed for the duration of `f`, and restores the previous
//   slot on exit (also on panic/unwind) through a drop guard.
// * `with_editor` is the only way API functions get at the editor. It marks the
//   slot as borrowed while its closure runs; any nested `with_editor` (e.g. Lua
//   code re-entered through a metamethod or a `__gc` finalizer while the
//   closure is running) gets a Lua error instead of a second, aliasing `&mut`.
//   The reference handed to the closure is higher-ranked, so it cannot escape.
// * API functions read/convert all their Lua arguments *before* calling
//   `with_editor` and build Lua return values *after* it returns, so in normal
//   operation no Lua code runs while the editor is borrowed.
// * A nested `with_editor_context` (e.g. a builtin command that fires plugin
//   events) is sound: its `&mut Editor` is a reborrow of the one currently held,
//   which is unusable until the nested call returns and the slot is restored.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct EditorSlot {
    editor: Option<NonNull<Editor>>,
    borrowed: bool,
}

const EMPTY_SLOT: EditorSlot = EditorSlot {
    editor: None,
    borrowed: false,
};

thread_local! {
    static CURRENT_EDITOR: Cell<EditorSlot> = const { Cell::new(EMPTY_SLOT) };
    /// Point in time after which running plugin code is aborted (see `install_timeout_hook`).
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The plugin whose code is running (loading, event handler, command, UI callback).
    static CURRENT_PLUGIN: RefCell<Option<Arc<str>>> = const { RefCell::new(None) };
}

/// Restores the current plugin on drop (panic safe).
struct PluginRestore(Option<Arc<str>>);

impl Drop for PluginRestore {
    fn drop(&mut self) {
        let prev = self.0.take();
        CURRENT_PLUGIN.with(|p| *p.borrow_mut() = prev);
    }
}

/// Run `f` as code of the plugin `name`: what it registers (handlers, commands, UI
/// callbacks) belongs to that plugin and `helix.get_config()` returns its configuration.
///
/// This is tracked on the Rust side: the global `_current_plugin_name` used before was
/// only set while a plugin was loading, and any plugin could overwrite it.
pub(crate) fn with_plugin<F, R>(name: &Arc<str>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = CURRENT_PLUGIN.with(|p| p.borrow_mut().replace(Arc::clone(name)));
    let _restore = PluginRestore(prev);
    f()
}

/// The name of the plugin whose code is running, or `"unknown"`.
pub(crate) fn current_plugin_name() -> Arc<str> {
    CURRENT_PLUGIN
        .with(|p| p.borrow().clone())
        .unwrap_or_else(|| Arc::from("unknown"))
}

/// Restores the editor slot on drop (panic safe).
struct SlotRestore(EditorSlot);

impl Drop for SlotRestore {
    fn drop(&mut self) {
        CURRENT_EDITOR.with(|slot| slot.set(self.0));
    }
}

/// Restores the execution deadline on drop (panic safe).
struct DeadlineRestore(Option<Instant>);

impl Drop for DeadlineRestore {
    fn drop(&mut self) {
        DEADLINE.with(|d| d.set(self.0));
    }
}

/// Maximum time a single entry into plugin code (event handler, command, UI
/// callback, plugin init) may run before it is aborted with a Lua error.
pub const PLUGIN_EXECUTION_TIMEOUT: Duration = Duration::from_secs(2);

/// How many VM instructions run between two deadline checks.
const HOOK_INSTRUCTION_INTERVAL: u32 = 10_000;

/// Maximum memory the plugin Lua state may allocate. Beyond it allocations fail with a Lua
/// "not enough memory" error instead of exhausting the machine (`("x"):rep(2^34)`).
pub const PLUGIN_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

/// Run `f` with an execution deadline. Nested calls keep the outer deadline.
pub(crate) fn with_deadline<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = DEADLINE.with(|d| d.get());
    let _restore = DeadlineRestore(prev);
    if prev.is_none() {
        DEADLINE.with(|d| d.set(Some(Instant::now() + PLUGIN_EXECUTION_TIMEOUT)));
    }
    f()
}

/// The error raised when plugin code runs past its deadline.
const TIMEOUT_MESSAGE: &std::ffi::CStr = c"plugin code exceeded the execution time limit";

/// Raw Lua count hook: raises [`TIMEOUT_MESSAGE`] once the running plugin code is past its
/// deadline.
///
/// # Safety
/// Called by Lua only. `lua_error` long-jumps out of this frame, which holds no values
/// with destructors at that point.
unsafe extern "C-unwind" fn timeout_hook(state: *mut ffi::lua_State, _ar: *mut ffi::lua_Debug) {
    if deadline_passed() {
        ffi::lua_pushstring(state, TIMEOUT_MESSAGE.as_ptr());
        ffi::lua_error(state);
    }
}

/// Install the instruction hook that aborts plugin code running past its deadline.
///
/// This is a plain Lua hook rather than mlua's `set_hook`: mlua runs its hook on a single
/// thread and switches it off in any other, so a loop inside a coroutine would never be
/// interrupted. Lua copies a thread's hook into every coroutine it creates, so this one
/// covers coroutines too.
fn install_timeout_hook(lua: &Lua) -> Result<()> {
    // SAFETY: setting a hook on the state mlua hands us; `timeout_hook` only reads a
    // thread-local and raises a Lua error.
    unsafe {
        lua.exec_raw::<()>((), |state| {
            ffi::lua_sethook(
                state,
                Some(timeout_hook),
                ffi::LUA_MASKCOUNT,
                HOOK_INSTRUCTION_INTERVAL as std::ffi::c_int,
            );
        })
    }
    .map_err(|e| PluginError::InitializationFailed(format!("Failed to install time limit: {}", e)))
}

/// Create a sandboxed Lua state with the execution time limit hook installed.
///
/// Besides the instruction hook, the time limit needs: wrappers for every standard
/// function that catches errors (see `make_timeout_uncatchable`), the pattern functions
/// of the string library ported so they check the deadline (`patterns`), and guards for
/// the C loops of `string.rep`/`table.insert`/`table.remove`/`table.move` and for `__gc`
/// finalizers, which run with the hook switched off (`sandbox`).
fn new_sandboxed_lua() -> Result<Lua> {
    let setup =
        |e: LuaError| PluginError::InitializationFailed(format!("Failed to setup sandbox: {}", e));
    let lua = Lua::new_with(sandbox::libs(), LuaOptions::new()).map_err(setup)?;
    sandbox::restrict(&lua).map_err(setup)?;
    patterns::install(&lua).map_err(setup)?;
    install_timeout_hook(&lua)?;
    make_timeout_uncatchable(&lua)?;
    Ok(lua)
}

/// Whether the plugin code that is running is past its deadline.
fn deadline_passed() -> bool {
    DEADLINE
        .with(|d| d.get())
        .is_some_and(|deadline| Instant::now() >= deadline)
}

/// The error raised by Rust code that notices the deadline has passed.
fn timeout_error() -> LuaError {
    LuaError::RuntimeError(TIMEOUT_MESSAGE.to_string_lossy().into_owned())
}

/// The timeout hook raises an ordinary Lua error, which `pcall`, `xpcall`,
/// `coroutine.resume`, `coroutine.close` and `load` (for errors of a reader function)
/// would catch: `while true do pcall(spin) end` would never end. Once the deadline has
/// passed, these wrappers raise the error again instead of returning it, so the abort
/// reaches the Rust caller however deeply the plugin catches errors. Before the deadline
/// they behave like the originals. The originals are kept only in upvalues, and the debug
/// library (which could reach them or remove the hook) is not loaded.
///
/// `load` also only accepts source text: Lua does not verify binary chunks, and a crafted
/// one corrupts memory.
fn make_timeout_uncatchable(lua: &Lua) -> Result<()> {
    let passed = lua
        .create_function(|_, ()| Ok(deadline_passed()))
        .map_err(|e| {
            PluginError::InitializationFailed(format!("Failed to setup sandbox: {}", e))
        })?;
    lua.load(
        r##"
        local deadline_passed = ...
        local raw_pcall, raw_xpcall, raw_error, raw_load = pcall, xpcall, error, load
        local raw_resume, raw_close = coroutine.resume, coroutine.close
        local select = select

        -- Re-raise a caught error once the deadline has passed.
        local function check(ok, ...)
            if not ok and deadline_passed() then
                raw_error((...), 0)
            end
            return ok, ...
        end

        pcall = function(...)
            return check(raw_pcall(...))
        end

        xpcall = function(f, handler, ...)
            -- Past the deadline the plugin's handler is not run: it could loop too.
            local function guarded(e)
                if deadline_passed() then
                    return e
                end
                return handler(e)
            end
            return check(raw_xpcall(f, guarded, ...))
        end

        coroutine.resume = function(...)
            return check(raw_resume(...))
        end

        coroutine.close = function(...)
            return check(raw_close(...))
        end

        load = function(...)
            local chunk, chunkname, _, env = ...
            local f, err
            -- An explicit `env` (even nil) must be passed on as such.
            if select("#", ...) >= 4 then
                f, err = raw_load(chunk, chunkname, "t", env)
            else
                f, err = raw_load(chunk, chunkname, "t")
            end
            if f == nil then
                if deadline_passed() then
                    raw_error(err, 0)
                end
                return nil, err
            end
            return f
        end
        "##,
    )
    .set_name("=sandbox")
    .call::<()>(passed)
    .map_err(|e| PluginError::InitializationFailed(format!("Failed to setup sandbox: {}", e)))
}

/// Run `f` with `editor` available to the plugin API (and with an execution deadline).
pub(crate) fn with_editor_context<F, R>(editor: &mut Editor, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = CURRENT_EDITOR.with(|slot| {
        slot.replace(EditorSlot {
            editor: Some(NonNull::from(editor)),
            borrowed: false,
        })
    });
    let _restore = SlotRestore(prev);
    with_deadline(f)
}

/// Borrow the active editor for the duration of `f`.
///
/// Errors if there is no active editor context or if the editor is already
/// borrowed (re-entrant access). See the invariant at the top of this module.
pub(crate) fn with_editor<F, R>(f: F) -> LuaResult<R>
where
    F: FnOnce(&mut Editor) -> LuaResult<R>,
{
    try_with_editor(f)?.ok_or_else(|| {
        LuaError::RuntimeError(
            "No active editor context. This function can only be called from within a plugin callback."
                .to_string(),
        )
    })
}

/// Like [`with_editor`] but returns `Ok(None)` when there is no editor context.
pub(crate) fn try_with_editor<F, R>(f: F) -> LuaResult<Option<R>>
where
    F: FnOnce(&mut Editor) -> LuaResult<R>,
{
    let slot = CURRENT_EDITOR.with(|slot| slot.get());
    let Some(mut ptr) = slot.editor else {
        return Ok(None);
    };
    if slot.borrowed {
        return Err(LuaError::RuntimeError(
            "The editor is already in use: Helix API functions cannot be called re-entrantly \
             (e.g. from a metamethod or finalizer triggered by another API call)"
                .to_string(),
        ));
    }
    CURRENT_EDITOR.with(|s| {
        s.set(EditorSlot {
            editor: slot.editor,
            borrowed: true,
        })
    });
    let _restore = SlotRestore(slot);
    // SAFETY: the pointer was created in `with_editor_context` from a `&mut Editor`
    // that stays exclusively borrowed until the slot is restored. The `borrowed`
    // flag guarantees this is the only live reference derived from it, and the
    // closure's higher-ranked lifetime prevents the reference from escaping.
    let editor = unsafe { ptr.as_mut() };
    f(editor).map(Some)
}

/// The focused view and its document, or a Lua error if there is none.
pub(crate) fn focused_mut(editor: &mut Editor) -> LuaResult<(&mut View, &mut Document)> {
    let focus = editor.tree.focus;
    if editor.tree.try_get(focus).is_none() {
        return Err(LuaError::RuntimeError("No focused window".to_string()));
    }
    let view = editor.tree.get_mut(focus);
    let doc = editor
        .documents
        .get_mut(&view.doc)
        .ok_or_else(|| LuaError::RuntimeError("Focused window has no document".to_string()))?;
    Ok((view, doc))
}

/// The focused view and its document, or a Lua error if there is none.
pub(crate) fn focused_ref(editor: &Editor) -> LuaResult<(&View, &Document)> {
    let view = editor
        .tree
        .try_get(editor.tree.focus)
        .ok_or_else(|| LuaError::RuntimeError("No focused window".to_string()))?;
    let doc = editor
        .documents
        .get(&view.doc)
        .ok_or_else(|| LuaError::RuntimeError("Focused window has no document".to_string()))?;
    Ok((view, doc))
}

pub mod api;
#[cfg(test)]
mod editor_tests;
pub mod loader;
mod patterns;
mod sandbox;

/// Event handlers: event -> [(plugin name, callback in that plugin's state)].
type EventHandlers = HashMap<EventType, Vec<(Arc<str>, Arc<RegistryKey>)>>;
/// Commands: name -> (metadata, plugin name, callback in that plugin's state).
type Commands = HashMap<String, (crate::types::CommandMetadata, Arc<str>, Arc<RegistryKey>)>;

/// The Lua state of one plugin. Plugins share no Lua value: one cannot change the functions
/// another one calls, nor call anything with another one's permissions.
struct PluginState {
    lua: Lua,
    /// `None` for a state that runs code without a plugin directory (tests).
    plugin: Option<crate::types::Plugin>,
}

/// Lua scripting engine for Helix plugins
pub struct LuaEngine {
    /// The Lua state of each plugin, by plugin name.
    states: HashMap<Arc<str>, PluginState>,
    /// Registered event handlers: EventType -> Vec<(plugin_name, callback_ref)>
    event_handlers: Arc<RwLock<EventHandlers>>,
    /// Registered commands: name -> (metadata, callback_ref)
    commands: Arc<RwLock<Commands>>,
    /// Builtin editor command registry
    builtin_commands: Option<Arc<dyn crate::types::EditorCommandRegistry>>,
    /// UI callbacks: (plugin_name, callback_id) -> callback_ref
    ui_callbacks: Arc<RwLock<HashMap<(String, u64), RegistryKey>>>,
    /// Next available UI callback ID
    next_ui_callback_id: Arc<std::sync::atomic::AtomicU64>,
    /// UI handler registry
    ui_handler: Option<Arc<dyn crate::types::UiHandler>>,
    /// The `[plugins]` configuration (each plugin's `config` and grants).
    config: crate::types::PluginConfig,
}

impl LuaEngine {
    /// Create a new Lua engine
    pub fn new() -> Result<Self> {
        Ok(Self {
            states: HashMap::new(),
            event_handlers: Arc::new(RwLock::new(HashMap::new())),
            commands: Arc::new(RwLock::new(HashMap::new())),
            builtin_commands: None,
            ui_callbacks: Arc::new(RwLock::new(HashMap::new())),
            next_ui_callback_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            ui_handler: None,
            config: crate::types::PluginConfig::default(),
        })
    }

    /// Reset the Lua engine, clearing all state and plugins
    pub fn reset(&mut self) -> Result<()> {
        self.clear_handlers();
        self.commands.write().clear();
        self.ui_callbacks.write().clear();
        // Closes the states (after the registry keys into them are gone).
        self.states.clear();
        Ok(())
    }

    /// Drops the event handlers, and their count in the process-wide subscriptions.
    fn clear_handlers(&self) {
        let mut handlers = self.event_handlers.write();
        for (event_type, callbacks) in handlers.drain() {
            event_type.unsubscribe(callbacks.len());
        }
    }

    /// Set the builtin command registry
    pub fn set_builtin_command_registry(
        &mut self,
        registry: Arc<dyn crate::types::EditorCommandRegistry>,
    ) {
        self.builtin_commands = Some(registry);
    }

    /// Set the UI handler
    pub fn set_ui_handler(&mut self, handler: Arc<dyn crate::types::UiHandler>) {
        self.ui_handler = Some(handler);
    }

    /// Set the configuration the plugins loaded from now on get (their `config` and grants).
    pub fn register_api(&mut self, config: crate::types::PluginConfig) -> Result<()> {
        self.config = config;
        Ok(())
    }

    /// The permissions a plugin gets: the defaults and its grants in `config.toml`, no more
    /// than its manifest lists.
    pub fn permissions_for(
        &self,
        metadata: &crate::types::PluginMetadata,
    ) -> crate::permissions::Permissions {
        use crate::permissions::{parse_requested, Permissions};
        let granted = self
            .config
            .plugins
            .iter()
            .find(|p| p.name == metadata.name)
            .map(|p| p.permissions)
            .unwrap_or(Permissions::NONE);
        let requested = metadata
            .permissions
            .as_deref()
            .map(|names| parse_requested(&metadata.name, names));
        Permissions::effective(requested, granted)
    }

    /// A new Lua state for the plugin `name`, with the API and its permissions.
    fn new_state(&self, name: &str, permissions: crate::permissions::Permissions) -> Result<Lua> {
        let lua = new_sandboxed_lua()?;
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: name.to_string(),
            permissions,
        });
        if let Some(ref registry) = self.builtin_commands {
            lua.set_app_data(crate::types::CommandRegistryWrapper(Arc::clone(registry)));
        }
        if let Some(ref handler) = self.ui_handler {
            lua.set_app_data(crate::types::UiHandlerWrapper(Arc::clone(handler)));
        }
        lua.set_app_data(crate::types::UiCallbackRegistry(Arc::clone(
            &self.ui_callbacks,
        )));
        lua.set_app_data(crate::types::UiCallbackCounter(Arc::clone(
            &self.next_ui_callback_id,
        )));
        lua.set_app_data(self.config.clone());

        let helix = lua.create_table()?;
        self.register_event_api(&lua, &helix)?;
        self.register_command_api(&lua, &helix)?;
        self.register_keymap_api(&lua, &helix)?;
        register_config_api(&lua, &helix)?;
        api::register_buffer_api(&lua, &helix)?;
        api::register_editor_api(&lua, &helix)?;
        api::register_ui_api(&lua, &helix)?;
        api::register_window_api(&lua, &helix)?;
        api::register_lsp_api(&lua, &helix)?;
        api::register_log_api(&lua, &helix)?;
        helix.set("version", env!("CARGO_PKG_VERSION"))?;
        lua.globals().set("helix", helix)?;
        Ok(lua)
    }

    /// Creates the state of a plugin that is not loaded from a directory and returns it
    /// (tests, embedders running plugin code of their own).
    pub fn add_state(
        &mut self,
        name: &str,
        permissions: crate::permissions::Permissions,
    ) -> Result<&Lua> {
        let lua = self.new_state(name, permissions)?;
        let name: Arc<str> = Arc::from(name);
        let state = self
            .states
            .entry(name)
            .insert_entry(PluginState { lua, plugin: None });
        Ok(&state.into_mut().lua)
    }

    /// The Lua state of the plugin `name`.
    pub fn state(&self, name: &str) -> Option<&Lua> {
        self.states.get(name).map(|state| &state.lua)
    }

    /// Runs `code` as code of the plugin `name` (whose state must exist), with `editor`.
    pub fn run_as(&self, name: &str, editor: &mut Editor, code: &str) -> LuaResult<()> {
        let (name, state) = self
            .states
            .get_key_value(name)
            .ok_or_else(|| LuaError::RuntimeError(format!("No plugin '{name}'")))?;
        with_plugin(name, || {
            with_editor_context(editor, || state.lua.load(code).exec())
        })
    }

    /// Register the event API
    fn register_event_api(&self, lua: &Lua, helix: &LuaTable) -> Result<()> {
        let event_handlers: Arc<RwLock<EventHandlers>> = Arc::clone(&self.event_handlers);

        // helix.on(event_name, callback) - Subscribe to an event
        let on =
            lua.create_function(move |lua, (event_name, callback): (String, LuaFunction)| {
                let event_type = EventType::from_str(&event_name).map_err(|_| {
                    LuaError::RuntimeError(format!("Invalid event type: {}", event_name))
                })?;
                crate::permissions::require(lua, crate::permissions::Permission::Read, "helix.on")?;

                let plugin_name = current_plugin_name();
                let callback_ref = Arc::new(lua.create_registry_value(callback)?);

                // Add to event handlers (no Lua code runs while the lock is held)
                event_handlers
                    .write()
                    .entry(event_type)
                    .or_default()
                    .push((plugin_name, callback_ref));
                event_type.subscribe();

                Ok(())
            })?;

        helix.set("on", on)?;

        // helix.permissions() - What the calling plugin may do: { "read", "ui", ... }
        let permissions = lua.create_function(|lua, ()| {
            let permissions = lua
                .app_data_ref::<crate::permissions::PluginIdentity>()
                .map(|identity| identity.permissions)
                .unwrap_or_default();
            lua.create_sequence_from(permissions.iter().map(|p| p.as_str()))
        })?;
        helix.set("permissions", permissions)?;

        // helix.has_permission(name) - Whether the calling plugin has that permission
        let has_permission = lua.create_function(|lua, name: String| {
            let permission: crate::permissions::Permission =
                name.parse().map_err(LuaError::RuntimeError)?;
            Ok(lua
                .app_data_ref::<crate::permissions::PluginIdentity>()
                .is_some_and(|identity| identity.permissions.contains(permission)))
        })?;
        helix.set("has_permission", has_permission)?;

        Ok(())
    }

    /// Register the command API
    fn register_command_api(&self, lua: &Lua, helix: &LuaTable) -> Result<()> {
        let commands = Arc::clone(&self.commands);

        // helix.register_command({ ... })
        let reg_fn = lua.create_function(move |lua, table: LuaTable| {
            crate::permissions::require(
                lua,
                crate::permissions::Permission::Commands,
                "helix.register_command",
            )?;
            // Errors are propagated, not replaced by defaults: one may be the time limit
            // aborting an `__index` metamethod.
            let name: String = table
                .get::<Option<String>>("name")?
                .filter(|name| !name.is_empty())
                .ok_or_else(|| LuaError::RuntimeError("Command name required".into()))?;
            // `description` is accepted as an alias of `doc`.
            let doc: String = match table.get::<Option<String>>("doc")? {
                Some(doc) => doc,
                None => table
                    .get::<Option<String>>("description")?
                    .unwrap_or_default(),
            };
            let args: Option<String> = table.get("args")?;
            let handler: LuaFunction =
                table
                    .get::<Option<LuaFunction>>("handler")?
                    .ok_or_else(|| {
                        LuaError::RuntimeError("Command handler function required".into())
                    })?;

            let plugin = current_plugin_name();
            let mut commands = commands.write();
            // A plugin cannot take over another plugin's command (it would run in its
            // place, with the arguments meant for the other one).
            if let Some((_, owner, _)) = commands.get(&name) {
                if **owner != *plugin {
                    return Err(LuaError::RuntimeError(format!(
                        "Command '{name}' belongs to plugin '{owner}'"
                    )));
                }
            }
            let callback_ref = Arc::new(lua.create_registry_value(handler)?);
            let meta = crate::types::CommandMetadata {
                name: name.clone(),
                doc,
                args,
                hidden: false,
            };
            commands.insert(name, (meta, plugin, callback_ref));
            Ok(())
        })?;

        helix.set("register_command", reg_fn)?;

        Ok(())
    }

    /// Register `helix.keymap`
    fn register_keymap_api(&self, lua: &Lua, helix: &LuaTable) -> Result<()> {
        use crate::permissions::{require, require_all, Permission};
        let keymap = lua.create_table()?;
        let commands = Arc::clone(&self.commands);
        let next_id = Arc::clone(&self.next_ui_callback_id);

        // helix.keymap.set(modes, keys, action, opts) - Bind `keys` ("<space>z", "gx",
        // "C-s") in `modes` ("normal", "insert", "select", or a list) to `action`: a command
        // as in `[keys]` ("move_line_down", ":write", "@ihi<esc>", "no_op" to unbind) or a
        // Lua function. `opts.desc`: what the key help shows.
        let set = lua.create_function(
            move |lua,
                  (modes, keys, action, opts): (LuaValue, String, LuaValue, Option<LuaTable>)| {
                require(lua, Permission::Keymaps, "helix.keymap.set")?;
                let modes = parse_modes(&modes)?;
                let keys = parse_keys(&keys)?;
                let description: Option<String> = match &opts {
                    Some(opts) => opts.get("desc")?,
                    None => None,
                };
                let plugin = current_plugin_name();
                let command = match action {
                    LuaValue::Function(function) => {
                        // A hidden command of the plugin runs the function: with the
                        // plugin's permissions, so binding it needs no more.
                        let id = next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let name = format!("plugin-key-{plugin}-{id}");
                        let doc = description
                            .clone()
                            .unwrap_or_else(|| format!("{plugin}: key binding"));
                        let key = Arc::new(lua.create_registry_value(function)?);
                        commands.write().insert(
                            name.clone(),
                            (
                                crate::types::CommandMetadata {
                                    name: name.clone(),
                                    doc,
                                    args: None,
                                    hidden: true,
                                },
                                Arc::clone(&plugin),
                                key,
                            ),
                        );
                        format!(":{name}")
                    }
                    LuaValue::String(command) => {
                        let command = command.to_str()?.trim().to_string();
                        // The key runs the command with the user's rights: the plugin must
                        // have what the command needs. A plugin command of its own is fine.
                        let own = command
                            .strip_prefix(':')
                            .map(|line| line.split_whitespace().next().unwrap_or(""))
                            .and_then(|name| {
                                commands
                                    .read()
                                    .get(name)
                                    .map(|(_, owner, _)| **owner == *plugin)
                            });
                        match own {
                            Some(true) => {}
                            Some(false) => {
                                return Err(LuaError::RuntimeError(format!(
                                    "'{command}' is a command of another plugin"
                                )))
                            }
                            None => {
                                let registry = lua
                                    .app_data_ref::<crate::types::CommandRegistryWrapper>()
                                    .map(|wrapper| Arc::clone(&wrapper.0))
                                    .ok_or_else(|| {
                                        LuaError::RuntimeError(
                                            "Builtin command registry not available".into(),
                                        )
                                    })?;
                                let needed = registry
                                    .binding_permissions(&command)
                                    .map_err(LuaError::RuntimeError)?;
                                require_all(
                                    lua,
                                    &needed,
                                    &format!("a key binding to '{command}'"),
                                )?;
                            }
                        }
                        command
                    }
                    other => {
                        return Err(LuaError::RuntimeError(format!(
                            "A key binding needs a command name or a function (got a value of type {})",
                            other.type_name()
                        )))
                    }
                };
                with_editor(|editor| {
                    for mode in modes {
                        editor
                            .plugin_keymaps
                            .set(helix_view::editor::PluginBinding {
                                plugin: plugin.to_string(),
                                mode,
                                keys: keys.clone(),
                                command: command.clone(),
                                description: description.clone(),
                            });
                    }
                    Ok(())
                })
            },
        )?;
        keymap.set("set", set)?;

        // helix.keymap.del(modes, keys) - Remove a binding this plugin set (the configured
        // one comes back); whether there was one
        let del = lua.create_function(|lua, (modes, keys): (LuaValue, String)| {
            require(lua, Permission::Keymaps, "helix.keymap.del")?;
            let modes = parse_modes(&modes)?;
            let keys = parse_keys(&keys)?;
            let plugin = current_plugin_name();
            with_editor(|editor| {
                let mut removed = false;
                for mode in modes {
                    removed |= editor.plugin_keymaps.remove(&plugin, mode, &keys);
                }
                Ok(removed)
            })
        })?;
        keymap.set("del", del)?;

        // helix.keymap.list() - The bindings of all plugins:
        // { {plugin, mode, keys, command, desc}, ... }
        let list = lua.create_function(|lua, ()| {
            require(lua, Permission::Read, "helix.keymap.list")?;
            let bindings = with_editor(|editor| Ok(editor.plugin_keymaps.bindings.clone()))?;
            let list = lua.create_table()?;
            for binding in bindings {
                let entry = lua.create_table()?;
                entry.set("plugin", binding.plugin)?;
                entry.set("mode", mode_name(binding.mode))?;
                let keys: String = binding
                    .keys
                    .iter()
                    .map(|key| {
                        let key = key.to_string();
                        if key.chars().count() == 1 {
                            key
                        } else {
                            format!("<{key}>")
                        }
                    })
                    .collect();
                entry.set("keys", keys)?;
                entry.set("command", binding.command)?;
                entry.set("desc", binding.description)?;
                list.push(entry)?;
            }
            Ok(list)
        })?;
        keymap.set("list", list)?;

        helix.set("keymap", keymap)?;
        Ok(())
    }

    /// The state of `plugin`, or an error.
    fn plugin_lua(&self, plugin: &str) -> Result<&Lua> {
        self.states
            .get(plugin)
            .map(|state| &state.lua)
            .ok_or_else(|| PluginError::PluginNotFound(plugin.to_string()))
    }

    /// Execute a registered plugin command
    pub fn execute_command(
        &self,
        editor: &mut Editor,
        name: &str,
        args: Vec<String>,
    ) -> Result<()> {
        // Clone the key out so the lock is not held while Lua runs (the command
        // may call `helix.register_command`, which takes the write lock).
        let (plugin_name, callback_ref) = self
            .commands
            .read()
            .get(name)
            .map(|(_, plugin, key)| (Arc::clone(plugin), Arc::clone(key)))
            .ok_or_else(|| {
                PluginError::CommandExecutionFailed(format!("Command not found: {}", name))
            })?;

        let callback: LuaFunction = self
            .plugin_lua(&plugin_name)?
            .registry_value(&callback_ref)
            .map_err(|e| {
                PluginError::CommandExecutionFailed(format!("Failed to retrieve callback: {}", e))
            })?;

        with_plugin(&plugin_name, || {
            with_editor_context(editor, || {
                callback.call::<()>(args).map_err(|e| {
                    PluginError::CommandExecutionFailed(format!("Execution failed: {}", e))
                })
            })
        })
    }

    /// Get all registered commands metadata (but the hidden ones of key bindings)
    pub fn get_commands(&self) -> Vec<crate::types::CommandMetadata> {
        self.commands
            .read()
            .values()
            .filter(|(meta, _, _)| !meta.hidden)
            .map(|(meta, _, _)| meta.clone())
            .collect()
    }

    /// Handle a UI/Picker callback from the editor
    pub fn handle_ui_callback(
        &self,
        editor: &mut Editor,
        plugin_name: String,
        callback_id: u64,
        value: serde_json::Value,
    ) -> Result<()> {
        // Remove the entry and release the lock before running Lua: the callback
        // may open another prompt/picker, which inserts into the same map.
        let callback_ref = self
            .ui_callbacks
            .write()
            .remove(&(plugin_name.clone(), callback_id));
        if let Some(callback_ref) = callback_ref {
            let lua = self.plugin_lua(&plugin_name)?;
            let callback: LuaFunction = lua
                .registry_value(&callback_ref)
                .map_err(PluginError::LuaError)?;

            let lua_value = lua.to_value(&value).map_err(PluginError::LuaError)?;

            with_plugin(&Arc::from(plugin_name), || {
                with_editor_context(editor, || {
                    callback
                        .call::<()>(lua_value)
                        .map_err(PluginError::LuaError)
                })
            })?;
        }
        Ok(())
    }

    /// Drop a pending UI callback without calling it (prompt aborted, picker closed).
    /// Returns whether a callback was pending.
    pub fn release_ui_callback(&self, plugin_name: String, callback_id: u64) -> bool {
        self.ui_callbacks
            .write()
            .remove(&(plugin_name, callback_id))
            .is_some()
    }

    /// Loads a plugin into a Lua state of its own, with the permissions it gets. Returns
    /// the permissions its manifest asks for and it does not get.
    pub fn load_plugin(
        &mut self,
        plugin: crate::types::Plugin,
    ) -> Result<crate::permissions::Permissions> {
        let entry_file = plugin
            .path
            .join(plugin.metadata.entry.as_deref().unwrap_or("init.lua"));

        if !entry_file.exists() {
            return Err(PluginError::InvalidPluginStructure(format!(
                "Entry file not found: {:?}",
                entry_file
            )));
        }

        let name: Arc<str> = Arc::from(plugin.metadata.name.as_str());
        // Handlers find their plugin's state by name: two plugins cannot share one.
        if self.states.contains_key(&name) {
            return Err(PluginError::InvalidPluginStructure(format!(
                "a plugin named '{name}' is loaded already; {:?} is skipped",
                plugin.path
            )));
        }
        let permissions = self.permissions_for(&plugin.metadata);
        let requested = plugin
            .metadata
            .permissions
            .as_deref()
            .map(|names| crate::permissions::parse_requested(&plugin.metadata.name, names));
        let missing = requested
            .map(|requested| requested.difference(permissions))
            .unwrap_or_default();
        log::info!("Plugin '{name}' may use: {permissions}");

        let lua = self.new_state(&name, permissions)?;
        // `require("<plugin>.<module>")` finds the plugin's other files.
        if let Some(dir) = plugin.path.parent() {
            sandbox::add_module_dir(&lua, dir)?;
        }

        // Load and execute the plugin; what it registers belongs to it.
        let code = std::fs::read_to_string(&entry_file)?;
        let loaded = with_plugin(&name, || {
            with_deadline(|| {
                lua.load(&code)
                    .set_name(format!("@{}", entry_file.display()))
                    .exec()
            })
        });
        if let Err(error) = loaded {
            // A plugin that failed to load does nothing: what it registered before the error
            // goes (its state is not kept).
            self.forget_plugin(&name);
            return Err(PluginError::LuaError(error));
        }

        self.states.insert(
            name,
            PluginState {
                lua,
                plugin: Some(plugin),
            },
        );

        Ok(missing)
    }

    /// Drops the handlers, commands and UI callbacks of the plugin `name`.
    fn forget_plugin(&self, name: &str) {
        let mut handlers = self.event_handlers.write();
        for (event_type, callbacks) in handlers.iter_mut() {
            let before = callbacks.len();
            callbacks.retain(|(plugin, _)| **plugin != *name);
            event_type.unsubscribe(before - callbacks.len());
        }
        drop(handlers);
        self.commands
            .write()
            .retain(|_, (_, plugin, _)| **plugin != *name);
        self.ui_callbacks
            .write()
            .retain(|(plugin, _), _| plugin != name);
    }

    /// Whether some plugin of this engine has a handler for `event_type`.
    pub fn has_handlers(&self, event_type: EventType) -> bool {
        self.event_handlers
            .read()
            .get(&event_type)
            .is_some_and(|handlers| !handlers.is_empty())
    }

    /// Call all event handlers for a given event
    pub fn call_event_handlers(&self, editor: &mut Editor, event: &PluginEvent) -> Result<()> {
        // Snapshot the handlers and release the lock before running Lua: handlers
        // may call `helix.on`, which takes the write lock.
        let callbacks: Vec<(Arc<str>, Arc<RegistryKey>)> =
            match self.event_handlers.read().get(&event.event_type) {
                Some(callbacks) if !callbacks.is_empty() => callbacks.clone(),
                _ => return Ok(()),
            };

        // A failing handler does not keep the others (often of other plugins) from running.
        let mut first_error = None;
        for (plugin_name, callback_ref) in &callbacks {
            if let Err(e) = self.call_event_handler(editor, event, plugin_name, callback_ref) {
                if first_error.is_none() {
                    first_error = Some(e);
                } else {
                    log::error!("{}", e);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn call_event_handler(
        &self,
        editor: &mut Editor,
        event: &PluginEvent,
        plugin_name: &Arc<str>,
        callback_ref: &RegistryKey,
    ) -> Result<()> {
        let handler_error = |error: String| PluginError::EventHandlerError {
            plugin: plugin_name.to_string(),
            error,
        };
        let lua = self.plugin_lua(plugin_name)?;
        let callback: LuaFunction = lua
            .registry_value(callback_ref)
            .map_err(|e| handler_error(format!("Failed to retrieve callback: {}", e)))?;

        // A fresh table per handler: a handler may modify it.
        let event_data = event_table(lua, event).map_err(PluginError::LuaError)?;

        with_plugin(plugin_name, || {
            with_editor_context(editor, || {
                callback
                    .call::<()>(event_data)
                    .map_err(|e| handler_error(format!("Handler execution failed: {}", e)))
            })
        })
    }

    /// Get loaded plugins
    pub fn plugins(&self) -> impl Iterator<Item = &crate::types::Plugin> {
        self.states
            .values()
            .filter_map(|state| state.plugin.as_ref())
    }
}

impl Drop for LuaEngine {
    fn drop(&mut self) {
        self.clear_handlers();
    }
}

/// The modes of `helix.keymap`: a name or a list of names.
fn parse_modes(value: &LuaValue) -> LuaResult<Vec<helix_view::document::Mode>> {
    use helix_view::document::Mode;
    let parse = |name: &str| match name {
        "normal" | "n" => Ok(Mode::Normal),
        "insert" | "i" => Ok(Mode::Insert),
        "select" | "s" | "v" => Ok(Mode::Select),
        _ => Err(LuaError::RuntimeError(format!(
            "Invalid mode '{name}' (expected normal, insert or select)"
        ))),
    };
    match value {
        LuaValue::String(name) => Ok(vec![parse(&name.to_str()?)?]),
        LuaValue::Table(names) => {
            let mut modes = Vec::new();
            for i in 1..=names.raw_len() {
                let name: String = names.raw_get(i)?;
                modes.push(parse(&name)?);
            }
            if modes.is_empty() {
                return Err(LuaError::RuntimeError("No mode given".into()));
            }
            Ok(modes)
        }
        other => Err(LuaError::RuntimeError(format!(
            "Invalid modes: a name or a list of names, not a {}",
            other.type_name()
        ))),
    }
}

fn mode_name(mode: helix_view::document::Mode) -> &'static str {
    use helix_view::document::Mode;
    match mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Select => "select",
    }
}

/// The keys of `helix.keymap`: like a macro, `"<space>z"`, `"gx"`, `"<C-s>"`; a single
/// named key may leave out the brackets (`"C-s"`, `"space"`).
fn parse_keys(keys: &str) -> LuaResult<Vec<helix_view::input::KeyEvent>> {
    let single = keys.parse::<helix_view::input::KeyEvent>();
    let parsed = match single {
        Ok(key) if keys.chars().count() > 1 && !keys.contains('<') => Ok(vec![key]),
        _ => helix_view::input::parse_macro(keys),
    };
    match parsed {
        Ok(keys) if !keys.is_empty() => Ok(keys),
        Ok(_) => Err(LuaError::RuntimeError("No keys given".into())),
        Err(e) => Err(LuaError::RuntimeError(format!(
            "Invalid keys '{keys}': {e}"
        ))),
    }
}

/// Register the config API
fn register_config_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    // helix.get_config() - Get configuration for the current plugin
    let get_config = lua.create_function(move |lua, ()| {
        let plugin_name = current_plugin_name();
        // Clone the value out so no app data borrow is held while creating Lua values.
        let config = lua
            .app_data_ref::<crate::types::PluginConfig>()
            .and_then(|config| {
                config
                    .plugins
                    .iter()
                    .find(|p| *p.name == *plugin_name)
                    .map(|p| p.config.clone())
            });
        match config {
            // Nested tables and arrays included; JSON null becomes nil.
            Some(config @ serde_json::Value::Object(_)) => lua.to_value_with(
                &config,
                LuaSerializeOptions::new()
                    .serialize_none_to_null(false)
                    .serialize_unit_to_null(false)
                    .set_array_metatable(false),
            ),
            _ => Ok(LuaValue::Nil),
        }
    })?;

    helix.set("get_config", get_config)?;

    Ok(())
}

/// The table passed to event handlers.
fn event_table(lua: &Lua, event: &PluginEvent) -> LuaResult<LuaTable> {
    use crate::types::{EventData, EventValue};
    let data = lua.create_table()?;
    data.raw_set("type", event.event_type.as_str())?;
    match &event.data {
        EventData::None => {}
        EventData::Buffer { document_id, path } => {
            data.raw_set("document_id", format!("{:?}", document_id))?;
            data.raw_set(
                "path",
                path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            )?;
        }
        EventData::ModeChange { old_mode, new_mode } => {
            // Spelled like `helix.editor.mode()` returns them ("normal", "insert", ...).
            data.raw_set("old_mode", old_mode.to_ascii_lowercase())?;
            data.raw_set("new_mode", new_mode.to_ascii_lowercase())?;
        }
        EventData::KeyPress { key } => data.raw_set("key", key.as_str())?,
        EventData::LspAttach {
            document_id,
            language_server_id,
        } => {
            data.raw_set("document_id", format!("{:?}", document_id))?;
            data.raw_set("language_server_id", *language_server_id)?;
        }
        EventData::LspDiagnostic {
            document_id,
            diagnostic_count,
        } => {
            data.raw_set("document_id", format!("{:?}", document_id))?;
            data.raw_set("diagnostic_count", *diagnostic_count)?;
        }
        EventData::Fields(fields) => {
            for (key, value) in fields {
                match value {
                    EventValue::Nil => {}
                    EventValue::Bool(b) => data.raw_set(*key, *b)?,
                    EventValue::Int(n) => data.raw_set(*key, *n)?,
                    EventValue::Str(s) => data.raw_set(*key, s.as_str())?,
                    EventValue::Document(id) => data.raw_set(*key, format!("{:?}", id))?,
                    EventValue::View(id) => data.raw_set(*key, format!("{:?}", id))?,
                }
            }
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{Permission, Permissions};

    /// An engine with a plugin state `p` of the given permissions.
    fn engine_with(permissions: Permissions) -> LuaEngine {
        let mut engine = LuaEngine::new().unwrap();
        engine.add_state("p", permissions).unwrap();
        engine
    }

    fn exec(engine: &LuaEngine, code: &str) -> LuaResult<()> {
        let name: Arc<str> = Arc::from("p");
        with_plugin(&name, || engine.state("p").unwrap().load(code).exec())
    }

    #[test]
    fn test_api_registration() {
        let engine = engine_with(Permissions::DEFAULT);
        let result: LuaResult<()> = exec(&engine, "assert(helix ~= nil)");
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_registration() {
        let engine = engine_with(Permissions::DEFAULT);
        let result: LuaResult<()> = exec(
            &engine,
            r#"
            local called = false
            helix.on("buffer_open", function(event)
                called = true
            end)
            assert(called == false) -- Not called yet
            "#,
        );
        assert!(result.is_ok());
        assert!(engine.has_handlers(EventType::OnBufferOpen));
        assert!(EventType::OnBufferOpen.is_subscribed());
    }

    #[test]
    fn every_event_has_a_name_that_parses_back() {
        for &event in EventType::ALL {
            let parsed: EventType = event.as_str().parse().unwrap();
            assert_eq!(parsed, event);
        }
        let names: std::collections::HashSet<&str> =
            EventType::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(names.len(), EventType::ALL.len(), "names are unique");
        assert!(
            EventType::ALL.len() <= 64,
            "the subscriber counts hold 64 events"
        );
        let unknown: std::result::Result<EventType, ()> = "nope".parse();
        assert!(unknown.is_err());
    }

    #[test]
    fn test_sandbox() {
        let engine = engine_with(Permissions::all());
        let result: LuaResult<()> = exec(&engine, "os.execute('ls')");
        assert!(result.is_err());
        let result: LuaResult<()> = exec(&engine, "io.open('/etc/passwd')");
        assert!(result.is_err());
    }

    #[test]
    fn test_full_api_availability() {
        let engine = engine_with(Permissions::DEFAULT);
        let code = r#"
            assert(helix.buffer ~= nil)
            assert(helix.buffer.get_current ~= nil)
            assert(helix.editor ~= nil)
            assert(helix.editor.mode ~= nil)
            assert(helix.ui ~= nil)
            assert(helix.ui.notify ~= nil)
        "#;
        let result: LuaResult<()> = exec(&engine, code);
        assert!(result.is_ok());
    }

    #[test]
    fn plugins_see_their_own_permissions() {
        let engine = engine_with([Permission::Read, Permission::Edit].into_iter().collect());
        exec(
            &engine,
            r#"
            names = table.concat(helix.permissions(), ",")
            can_edit = helix.has_permission("edit")
            can_shell = helix.has_permission("shell")
            "#,
        )
        .unwrap();
        let lua = engine.state("p").unwrap();
        let names: String = lua.globals().get("names").unwrap();
        assert_eq!(names, "read,edit");
        let (can_edit, can_shell): (bool, bool) = (
            lua.globals().get("can_edit").unwrap(),
            lua.globals().get("can_shell").unwrap(),
        );
        assert_eq!((can_edit, can_shell), (true, false));
        let unknown = exec(&engine, r#"helix.has_permission("network")"#);
        assert!(unknown
            .unwrap_err()
            .to_string()
            .contains("unknown plugin permission"));
    }

    #[test]
    fn registering_needs_the_permissions() {
        // without `read` no events, without `commands` no commands
        let engine = engine_with(Permissions::NONE.union([Permission::Ui].into_iter().collect()));
        let on = exec(&engine, r#"helix.on("buffer_open", function() end)"#);
        assert!(on.unwrap_err().to_string().contains("'read' permission"));
        let command = exec(
            &engine,
            r#"helix.register_command{ name = "x", handler = function() end }"#,
        );
        assert!(command
            .unwrap_err()
            .to_string()
            .contains("'commands' permission"));
        assert!(!engine.has_handlers(EventType::OnBufferOpen));
    }

    /// Plugins share no Lua value: one cannot replace what another calls.
    #[test]
    fn plugins_are_isolated_from_each_other() {
        let mut engine = LuaEngine::new().unwrap();
        engine.add_state("a", Permissions::DEFAULT).unwrap();
        engine.add_state("b", Permissions::all()).unwrap();
        let run = |name: &str, code: &str| -> LuaResult<()> {
            let name: Arc<str> = Arc::from(name);
            with_plugin(&name, || engine.state(&name).unwrap().load(code).exec())
        };
        run(
            "a",
            r#"
            helix.editor.execute_command = function() stolen = true end
            string.upper = function() return "evil" end
            shared = "from a"
            "#,
        )
        .unwrap();
        run(
            "b",
            r#"
            seen = tostring(shared)
            upper = ("x"):upper()
            same = helix.editor.execute_command == nil
            "#,
        )
        .unwrap();
        let b = engine.state("b").unwrap();
        let seen: String = b.globals().get("seen").unwrap();
        let upper: String = b.globals().get("upper").unwrap();
        let same: bool = b.globals().get("same").unwrap();
        assert_eq!((seen.as_str(), upper.as_str(), same), ("nil", "X", false));
    }

    #[test]
    fn a_plugin_cannot_take_over_another_plugin_s_command() {
        let mut engine = LuaEngine::new().unwrap();
        let commands: Permissions = [Permission::Commands].into_iter().collect();
        engine.add_state("a", commands).unwrap();
        engine.add_state("b", commands).unwrap();
        let run = |name: &str, code: &str| -> LuaResult<()> {
            let name: Arc<str> = Arc::from(name);
            with_plugin(&name, || engine.state(&name).unwrap().load(code).exec())
        };
        let register = r#"helix.register_command{ name = "greet", handler = function() end }"#;
        run("a", register).unwrap();
        // its owner may register it again (`:reload-all-plugins` aside)
        run("a", register).unwrap();
        let error = run("b", register).unwrap_err().to_string();
        assert!(
            error.contains("Command 'greet' belongs to plugin 'a'"),
            "{error}"
        );
    }

    /// A plugin is loaded from `dir/<name>` with `init.lua` `code`.
    fn plugin(dir: &std::path::Path, name: &str, code: &str) -> crate::types::Plugin {
        let path = dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("init.lua"), code).unwrap();
        crate::types::Plugin {
            metadata: crate::types::PluginMetadata {
                name: name.to_string(),
                ..Default::default()
            },
            path,
            enabled: true,
        }
    }

    #[test]
    fn a_plugin_that_fails_to_load_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = LuaEngine::new().unwrap();
        // `exit` is not used by any other test of this crate
        let before = EventType::OnExit.is_subscribed();
        let failing = plugin(
            dir.path(),
            "failing",
            r#"helix.on("exit", function() end)
            error("broken")"#,
        );
        let error = engine.load_plugin(failing).unwrap_err().to_string();
        assert!(error.contains("broken"), "{error}");
        assert!(!engine.has_handlers(EventType::OnExit));
        assert_eq!(EventType::OnExit.is_subscribed(), before);
        assert_eq!(engine.plugins().count(), 0);
    }

    #[test]
    fn a_second_plugin_of_the_same_name_is_skipped() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut engine = LuaEngine::new().unwrap();
        let loaded = engine.load_plugin(plugin(first.path(), "twin", "x = 1"));
        let missing: crate::permissions::Permissions = loaded.unwrap();
        assert!(missing.is_empty());
        let error = engine
            .load_plugin(plugin(second.path(), "twin", "x = 2"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("a plugin named 'twin' is loaded already"),
            "{error}"
        );
        let x: i64 = engine.state("twin").unwrap().globals().get("x").unwrap();
        assert_eq!(x, 1);
    }

    #[test]
    fn handlers_are_unsubscribed_on_reset() {
        let mut engine = engine_with(Permissions::DEFAULT);
        // `idle` is not used by any other test of this crate
        let before = EventType::OnIdle.is_subscribed();
        exec(&engine, r#"helix.on("idle", function() end)"#).unwrap();
        assert!(EventType::OnIdle.is_subscribed());
        engine.reset().unwrap();
        assert_eq!(EventType::OnIdle.is_subscribed(), before);
        assert!(engine.state("p").is_none());
    }
}
