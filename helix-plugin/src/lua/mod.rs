use crate::error::{PluginError, Result};
use crate::types::{EventType, PluginEvent};
use helix_view::Editor;
use mlua::prelude::*;
use mlua::RegistryKey;
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

thread_local! {
    static CURRENT_EDITOR: RefCell<Option<*mut Editor>> = const { RefCell::new(None) };
}

/// Helper to set the current editor context during a function execution
pub(crate) fn with_editor_context<F, R>(editor: &mut Editor, f: F) -> R
where
    F: FnOnce() -> R,
{
    CURRENT_EDITOR.with(|e| {
        *e.borrow_mut() = Some(editor as *mut _);
    });
    let result = f();
    CURRENT_EDITOR.with(|e| {
        *e.borrow_mut() = None;
    });
    result
}

/// Internal helper to get the active editor context
pub(crate) fn get_editor_mut() -> std::result::Result<&'static mut Editor, mlua::Error> {
    CURRENT_EDITOR.with(|e| {
        let ptr = *e.borrow();
        match ptr {
            Some(p) => Ok(unsafe { &mut *p }),
            None => Err(mlua::Error::RuntimeError(
                "No active editor context. This function can only be called from within a plugin callback.".to_string(),
            )),
        }
    })
}

pub mod api;
pub mod loader;

type EventHandlers = HashMap<EventType, Vec<(String, RegistryKey)>>;

/// Lua scripting engine for Helix plugins
pub struct LuaEngine {
    /// The Lua runtime
    lua: Lua,
    /// Registered event handlers: EventType -> Vec<(plugin_name, callback_ref)>
    event_handlers: Arc<RwLock<EventHandlers>>,
    /// Loaded plugins by name
    /// Loaded plugins by name
    plugins: HashMap<String, crate::types::Plugin>,
    /// Registered commands: name -> (metadata, callback_ref)
    commands: Arc<RwLock<HashMap<String, (crate::types::CommandMetadata, RegistryKey)>>>,
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
        let lua = Lua::new();

        // Set up sandboxing - remove dangerous functions
        lua.load(
            r#"
            -- Remove dangerous functions
            os.execute = nil
            os.exit = nil
            io = nil
            loadfile = nil
            dofile = nil
            "#,
        )
        .exec()
        .map_err(|e| {
            PluginError::InitializationFailed(format!("Failed to setup sandbox: {}", e))
        })?;

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
        let lua = Lua::new();

        // Set up sandboxing - remove dangerous functions
        lua.load(
            r#"
            -- Remove dangerous functions
            os.execute = nil
            os.exit = nil
            io = nil
            loadfile = nil
            dofile = nil
            "#,
        )
        .exec()
        .map_err(|e| {
            PluginError::InitializationFailed(format!("Failed to setup sandbox: {}", e))
        })?;

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

                let plugin_name = lua
                    .globals()
                    .get::<String>("_current_plugin_name")
                    .unwrap_or_else(|_| "unknown".to_string());

                let callback_ref = lua.create_registry_value(callback)?;

                // Add to event handlers
                let mut handlers = event_handlers.write();
                handlers
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
            let plugin_name = lua
                .globals()
                .get::<String>("_current_plugin_name")
                .unwrap_or_else(|_| "unknown".to_string());

            if let Some(config) = lua.app_data_ref::<crate::types::PluginConfig>() {
                if let Some(plugin_config) = config.plugins.iter().find(|p| p.name == plugin_name) {
                    // Convert serde_json::Value to LuaValue
                    let val = match &plugin_config.config {
                        serde_json::Value::Object(map) => {
                            let table = lua.create_table()?;
                            for (k, v) in map {
                                // Simple conversion for common types
                                match v {
                                    serde_json::Value::String(s) => {
                                        table.set(k.clone(), s.clone())?
                                    }
                                    serde_json::Value::Number(n) => {
                                        table.set(k.clone(), n.as_f64().unwrap_or(0.0))?
                                    }
                                    serde_json::Value::Bool(b) => table.set(k.clone(), *b)?,
                                    _ => {} // Skip complex types for now
                                }
                            }
                            Some(table)
                        }
                        _ => None,
                    };
                    return Ok(val);
                }
            }
            Ok(None)
        })?;

        helix.set("get_config", get_config)?;

        Ok(())
    }

    /// Register the command API
    fn register_command_api(&self, helix: &LuaTable) -> Result<()> {
        let commands = Arc::clone(&self.commands);

        // helix.register_command({ ... })
        let reg_fn = self.lua.create_function(move |lua, table: LuaTable| {
            let name: String = table
                .get("name")
                .map_err(|_| LuaError::RuntimeError("Command name required".into()))?;
            let doc: String = table.get("doc").unwrap_or_else(|_| "".into());
            let args: Option<String> = table.get("args").ok();
            let handler: LuaFunction = table
                .get("handler")
                .map_err(|_| LuaError::RuntimeError("Command handler function required".into()))?;

            let callback_ref = lua.create_registry_value(handler)?;

            let meta = crate::types::CommandMetadata {
                name: name.clone(),
                doc,
                args,
            };

            commands.write().insert(name, (meta, callback_ref));
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
        let commands = self.commands.read();
        if let Some((_, callback_ref)) = commands.get(name) {
            let callback: LuaFunction = self.lua.registry_value(callback_ref).map_err(|e| {
                PluginError::CommandExecutionFailed(format!("Failed to retrieve callback: {}", e))
            })?;

            with_editor_context(editor, || {
                callback.call::<()>(args).map_err(|e| {
                    PluginError::CommandExecutionFailed(format!("Execution failed: {}", e))
                })
            })?;
        } else {
            return Err(PluginError::CommandExecutionFailed(format!(
                "Command not found: {}",
                name
            )));
        }
        Ok(())
    }

    /// Get all registered commands metadata
    pub fn get_commands(&self) -> Vec<crate::types::CommandMetadata> {
        self.commands
            .read()
            .values()
            .map(|(meta, _)| meta.clone())
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
        let mut callbacks = self.ui_callbacks.write();
        if let Some(callback_ref) = callbacks.remove(&(plugin_name, callback_id)) {
            let callback: LuaFunction = self
                .lua
                .registry_value(&callback_ref)
                .map_err(PluginError::LuaError)?;

            let lua_value = self.lua.to_value(&value).map_err(PluginError::LuaError)?;

            with_editor_context(editor, || {
                callback
                    .call::<()>(lua_value)
                    .map_err(PluginError::LuaError)
            })?;
        }
        Ok(())
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

        // Set the current plugin name in globals so event handlers know which plugin they belong to
        let globals = self.lua.globals();
        globals.set("_current_plugin_name", plugin.metadata.name.clone())?;

        // Load and execute the plugin
        let code = std::fs::read_to_string(&entry_file)?;
        self.lua
            .load(&code)
            .set_name(&plugin.metadata.name)
            .exec()
            .map_err(PluginError::LuaError)?;

        // Clear the current plugin name
        globals.set("_current_plugin_name", LuaValue::Nil)?;

        self.plugins.insert(plugin.metadata.name.clone(), plugin);

        Ok(())
    }

    /// Call all event handlers for a given event
    pub fn call_event_handlers(&self, editor: &mut Editor, event: &PluginEvent) -> Result<()> {
        let handlers = self.event_handlers.read();

        if let Some(callbacks) = handlers.get(&event.event_type) {
            for (plugin_name, callback_ref) in callbacks {
                // Get the callback from the registry
                let callback: LuaFunction = self.lua.registry_value(callback_ref).map_err(|e| {
                    PluginError::EventHandlerError {
                        plugin: plugin_name.clone(),
                        error: format!("Failed to retrieve callback: {}", e),
                    }
                })?;

                // Call the callback with event data
                let event_data = self.lua.create_table().map_err(PluginError::LuaError)?;
                event_data
                    .set("type", event.event_type.as_str())
                    .map_err(PluginError::LuaError)?;

                // Set event-specific data
                match &event.data {
                    crate::types::EventData::Buffer { document_id, path } => {
                        event_data
                            .set("document_id", format!("{:?}", document_id))
                            .ok();
                        event_data
                            .set(
                                "path",
                                path.as_ref().map(|p| p.to_string_lossy().to_string()),
                            )
                            .ok();
                    }
                    crate::types::EventData::ModeChange { old_mode, new_mode } => {
                        event_data.set("old_mode", old_mode.clone()).ok();
                        event_data.set("new_mode", new_mode.clone()).ok();
                    }
                    crate::types::EventData::KeyPress { key } => {
                        event_data.set("key", key.clone()).ok();
                    }
                    crate::types::EventData::LspDiagnostic {
                        document_id,
                        diagnostic_count,
                    } => {
                        event_data
                            .set("document_id", format!("{:?}", document_id))
                            .ok();
                        event_data.set("diagnostic_count", *diagnostic_count).ok();
                    }
                    _ => {}
                }

                let plugin_name_captured = plugin_name.clone();
                with_editor_context(editor, || {
                    callback
                        .call::<()>(event_data)
                        .map_err(|e| PluginError::EventHandlerError {
                            plugin: plugin_name_captured,
                            error: format!("Handler execution failed: {}", e),
                        })
                })?;
            }
        }

        Ok(())
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
