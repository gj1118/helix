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

/// Event handlers: event -> [(plugin name, callback)].
type EventHandlers = HashMap<EventType, Vec<(Arc<str>, Arc<RegistryKey>)>>;
/// Commands: name -> (metadata, plugin name, callback).
type Commands = HashMap<String, (crate::types::CommandMetadata, Arc<str>, Arc<RegistryKey>)>;

/// Lua scripting engine for Helix plugins
pub struct LuaEngine {
    /// The Lua runtime
    lua: Lua,
    /// Registered event handlers: EventType -> Vec<(plugin_name, callback_ref)>
    event_handlers: Arc<RwLock<EventHandlers>>,
    /// Loaded plugins by name
    plugins: HashMap<String, crate::types::Plugin>,
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
}

impl LuaEngine {
    /// Create a new Lua engine
    pub fn new() -> Result<Self> {
        let lua = new_sandboxed_lua()?;

        let event_handlers = Arc::new(RwLock::new(HashMap::new()));
        let commands = Arc::new(RwLock::new(HashMap::new()));
        let ui_callbacks = Arc::new(RwLock::new(HashMap::new()));
        let next_ui_callback_id = Arc::new(std::sync::atomic::AtomicU64::new(1));

        Ok(Self {
            lua,
            event_handlers,
            plugins: HashMap::new(),
            commands,
            builtin_commands: None,
            ui_callbacks,
            next_ui_callback_id,
            ui_handler: None,
        })
    }

    /// Reset the Lua engine, clearing all state and plugins
    pub fn reset(&mut self) -> Result<()> {
        let lua = new_sandboxed_lua()?;

        self.lua = lua;
        self.event_handlers.write().clear();
        self.commands.write().clear();
        self.plugins.clear();
        self.ui_callbacks.write().clear();

        Ok(())
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

    /// Register the Helix API with Lua
    pub fn register_api(&self, config: crate::types::PluginConfig) -> Result<()> {
        let globals = self.lua.globals();

        if let Some(ref registry) = self.builtin_commands {
            self.lua
                .set_app_data(crate::types::CommandRegistryWrapper(Arc::clone(registry)));
        }

        if let Some(ref handler) = self.ui_handler {
            self.lua
                .set_app_data(crate::types::UiHandlerWrapper(Arc::clone(handler)));
        }

        self.lua
            .set_app_data(crate::types::UiCallbackRegistry(Arc::clone(
                &self.ui_callbacks,
            )));
        self.lua
            .set_app_data(crate::types::UiCallbackCounter(Arc::clone(
                &self.next_ui_callback_id,
            )));
        self.lua.set_app_data(config);

        // Create the main helix table
        let helix = self.lua.create_table()?;

        // Register the event system
        self.register_event_api(&helix)?;
        self.register_command_api(&helix)?;
        self.register_config_api(&helix)?;

        // Register API namespaces
        api::register_buffer_api(&self.lua, &helix)?;
        api::register_editor_api(&self.lua, &helix)?;
        api::register_ui_api(&self.lua, &helix)?;
        api::register_window_api(&self.lua, &helix)?;
        api::register_lsp_api(&self.lua, &helix)?;
        api::register_log_api(&self.lua, &helix)?;

        // Register version info
        helix.set("version", env!("CARGO_PKG_VERSION"))?;

        // Set the global helix table
        globals.set("helix", helix)?;

        Ok(())
    }

    /// Register the event API
    fn register_event_api(&self, helix: &LuaTable) -> Result<()> {
        let event_handlers: Arc<RwLock<EventHandlers>> = Arc::clone(&self.event_handlers);

        // helix.on(event_name, callback) - Subscribe to an event
        let on = self.lua.create_function(
            move |lua, (event_name, callback): (String, LuaFunction)| {
                let event_type = EventType::from_str(&event_name).map_err(|_| {
                    LuaError::RuntimeError(format!("Invalid event type: {}", event_name))
                })?;

                let plugin_name = current_plugin_name();
                let callback_ref = Arc::new(lua.create_registry_value(callback)?);

                // Add to event handlers (no Lua code runs while the lock is held)
                event_handlers
                    .write()
                    .entry(event_type)
                    .or_default()
                    .push((plugin_name, callback_ref));

                Ok(())
            },
        )?;

        helix.set("on", on)?;

        Ok(())
    }

    /// Register the config API
    fn register_config_api(&self, helix: &LuaTable) -> Result<()> {
        // helix.get_config() - Get configuration for the current plugin
        let get_config = self.lua.create_function(move |lua, ()| {
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

    /// Register the command API
    fn register_command_api(&self, helix: &LuaTable) -> Result<()> {
        let commands = Arc::clone(&self.commands);

        // helix.register_command({ ... })
        let reg_fn = self.lua.create_function(move |lua, table: LuaTable| {
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

            let callback_ref = Arc::new(lua.create_registry_value(handler)?);

            let meta = crate::types::CommandMetadata {
                name: name.clone(),
                doc,
                args,
            };

            commands
                .write()
                .insert(name, (meta, current_plugin_name(), callback_ref));
            Ok(())
        })?;

        helix.set("register_command", reg_fn)?;

        Ok(())
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

        let callback: LuaFunction = self.lua.registry_value(&callback_ref).map_err(|e| {
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

    /// Get all registered commands metadata
    pub fn get_commands(&self) -> Vec<crate::types::CommandMetadata> {
        self.commands
            .read()
            .values()
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
            let callback: LuaFunction = self
                .lua
                .registry_value(&callback_ref)
                .map_err(PluginError::LuaError)?;

            let lua_value = self.lua.to_value(&value).map_err(PluginError::LuaError)?;

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

    pub fn load_plugin(&mut self, plugin: crate::types::Plugin) -> Result<()> {
        let entry_file = plugin
            .path
            .join(plugin.metadata.entry.as_deref().unwrap_or("init.lua"));

        if !entry_file.exists() {
            return Err(PluginError::InvalidPluginStructure(format!(
                "Entry file not found: {:?}",
                entry_file
            )));
        }

        // `require("<plugin>.<module>")` finds the plugin's other files.
        if let Some(dir) = plugin.path.parent() {
            sandbox::add_module_dir(&self.lua, dir)?;
        }

        // Load and execute the plugin; what it registers belongs to it.
        let code = std::fs::read_to_string(&entry_file)?;
        let name: Arc<str> = Arc::from(plugin.metadata.name.as_str());
        with_plugin(&name, || {
            with_deadline(|| {
                self.lua
                    .load(&code)
                    .set_name(format!("@{}", entry_file.display()))
                    .exec()
            })
        })
        .map_err(PluginError::LuaError)?;

        self.plugins.insert(plugin.metadata.name.clone(), plugin);

        Ok(())
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
        let callback: LuaFunction = self
            .lua
            .registry_value(callback_ref)
            .map_err(|e| handler_error(format!("Failed to retrieve callback: {}", e)))?;

        // A fresh table per handler: a handler may modify it.
        let event_data = self.event_table(event).map_err(PluginError::LuaError)?;

        with_plugin(plugin_name, || {
            with_editor_context(editor, || {
                callback
                    .call::<()>(event_data)
                    .map_err(|e| handler_error(format!("Handler execution failed: {}", e)))
            })
        })
    }

    /// The table passed to event handlers.
    fn event_table(&self, event: &PluginEvent) -> LuaResult<LuaTable> {
        use crate::types::EventData;
        let data = self.lua.create_table()?;
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
        }
        Ok(data)
    }

    /// Get the Lua runtime (for advanced operations)
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Get loaded plugins
    pub fn plugins(&self) -> &HashMap<String, crate::types::Plugin> {
        &self.plugins
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = LuaEngine::new();
        assert!(engine.is_ok());
    }

    #[test]
    fn test_api_registration() {
        let engine = LuaEngine::new().unwrap();
        assert!(engine
            .register_api(crate::types::PluginConfig::default())
            .is_ok());

        // Test that helix global exists
        let result: std::result::Result<(), mlua::Error> =
            engine.lua.load("assert(helix ~= nil)").exec();
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_registration() {
        let engine = LuaEngine::new().unwrap();
        engine
            .register_api(crate::types::PluginConfig::default())
            .unwrap();

        let result: std::result::Result<(), mlua::Error> = engine
            .lua
            .load(
                r#"
            local called = false
            helix.on("buffer_open", function(event)
                called = true
            end)
            assert(called == false) -- Not called yet
            "#,
            )
            .exec();

        assert!(result.is_ok());
    }

    #[test]
    fn test_sandbox() {
        let engine = LuaEngine::new().unwrap();

        // These should fail due to sandboxing
        let result: std::result::Result<(), mlua::Error> =
            engine.lua.load("os.execute('ls')").exec();
        assert!(result.is_err());

        let result: std::result::Result<(), mlua::Error> =
            engine.lua.load("io.open('/etc/passwd')").exec();
        assert!(result.is_err());
    }

    #[test]
    fn test_full_api_availability() {
        let engine = LuaEngine::new().unwrap();
        engine
            .register_api(crate::types::PluginConfig::default())
            .unwrap();

        let code = r#"
            -- Test Buffer API
            assert(helix.buffer ~= nil)
            assert(helix.buffer.get_current ~= nil)
            
            -- Test Editor API
            assert(helix.editor ~= nil)
            assert(helix.editor.mode ~= nil)
            
            -- Test UI API
            assert(helix.ui ~= nil)
            assert(helix.ui.notify ~= nil)
        "#;

        let result: std::result::Result<(), mlua::Error> = engine.lua.load(code).exec();
        assert!(result.is_ok());
    }
}
