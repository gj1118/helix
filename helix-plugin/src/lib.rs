//! Helix Plugin System
//!
//! This crate provides a Lua-based plugin system for the Helix text editor.
//! Plugins can register event handlers, custom commands, and interact with
//! the editor through a safe API.
//!
//! # Example Plugin
//!
//! ```lua
//! -- init.lua
//! helix.on("buffer_open", function(event)
//!     helix.log.info("Buffer opened: " .. tostring(event.path))
//! end)
//!
//! helix.on("buffer_post_save", function(event)
//!     helix.ui.notify("Saved " .. tostring(event.path))
//! end)
//! ```

pub mod error;
pub mod lua;
pub mod permissions;
pub mod types;

// Re-exports
pub use error::{PluginError, Result};
pub use lua::LuaEngine;
pub use permissions::{Permission, Permissions};
pub use types::{
    EventData, EventType, EventValue, IndividualPluginConfig, Plugin, PluginConfig, PluginEvent,
    PluginMetadata,
};

use helix_view::Editor;
use log::info;
use parking_lot::RwLock;
use std::sync::Arc;

/// The main plugin manager
pub struct PluginManager {
    /// The Lua engine
    engine: Arc<RwLock<LuaEngine>>,
    /// Plugin configuration
    config: PluginConfig,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(config: PluginConfig) -> Result<Self> {
        let mut engine = LuaEngine::new()?;
        engine.register_api(config.clone())?;

        Ok(Self {
            engine: Arc::new(RwLock::new(engine)),
            config,
        })
    }

    /// Returns true if the plugin system is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Initialize and load all plugins
    pub fn initialize(&self, editor: &mut Editor) -> Result<()> {
        // Determine plugin directories
        let plugin_dirs = if self.config.plugin_dirs.is_empty() {
            lua::loader::PluginLoader::default_plugin_dirs()
        } else {
            self.config.plugin_dirs.clone()
        };

        info!("Searching for plugins in: {:?}", plugin_dirs);

        // Discover plugins
        let loader = lua::loader::PluginLoader::new(plugin_dirs);
        let plugins = loader.discover_plugins()?;

        info!("Discovered {} plugins", plugins.len());

        // Load each plugin
        let mut engine = self.engine.write();
        for plugin in plugins {
            // Check if plugin is enabled in config
            let enabled = self.is_plugin_enabled(&plugin.metadata.name);

            if !enabled {
                info!("Skipping disabled plugin: {}", plugin.metadata.name);
                continue;
            }

            info!("Loading plugin: {}", plugin.metadata.name);
            let name = plugin.metadata.name.clone();
            // The editor API works while a plugin loads too (e.g. `helix.ui.notify`).
            match lua::with_editor_context(editor, || engine.load_plugin(plugin)) {
                Ok(missing) if !missing.is_empty() => {
                    // Asked for in plugin.toml, not granted in config.toml: say so once,
                    // the calls that need them fail with the same advice.
                    let message = format!(
                        "Plugin '{name}' asks for permissions not granted: {missing} \
                         (see :log-open)"
                    );
                    log::warn!(
                        "{message}; grant them with permissions = [...] in its \
                         [[plugins.plugins]] entry of config.toml"
                    );
                    editor.set_status(message);
                }
                Ok(_) => {}
                Err(e) => log::error!("Failed to load plugin: {}", e),
            }
        }
        drop(engine);

        // Fire init event
        self.fire_event(
            editor,
            PluginEvent {
                event_type: EventType::OnInit,
                data: EventData::None,
            },
        )?;

        Ok(())
    }

    /// Reload all plugins
    pub fn reload_plugins(&self, editor: &mut Editor) -> Result<()> {
        // Their bindings go with them (a function binding's command is gone).
        editor.plugin_keymaps.clear();
        // Reset engine state
        {
            let mut engine = self.engine.write();
            engine.reset()?;

            // Re-register API
            engine.register_api(self.config.clone())?;
        }

        // Re-initialize (discover and load)
        self.initialize(editor)
    }

    /// Check if a plugin is enabled in the configuration
    fn is_plugin_enabled(&self, name: &str) -> bool {
        // If there's specific config for this plugin, use that
        if let Some(plugin_config) = self.config.plugins.iter().find(|p| p.name == name) {
            return plugin_config.enabled;
        }

        // Otherwise, enabled by default
        true
    }

    /// Fire an event to all registered handlers
    pub fn fire_event(&self, editor: &mut Editor, event: PluginEvent) -> Result<()> {
        let engine = self.engine.read();
        engine.call_event_handlers(editor, &event)
    }

    /// Get plugin configuration for a specific plugin
    pub fn get_plugin_config(&self, name: &str) -> Option<&serde_json::Value> {
        self.config
            .plugins
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.config)
    }

    /// Get the Lua engine (for advanced operations)
    pub fn engine(&self) -> Arc<RwLock<LuaEngine>> {
        Arc::clone(&self.engine)
    }

    /// Get registered commands
    pub fn get_commands(&self) -> Vec<crate::types::CommandMetadata> {
        self.engine.read().get_commands()
    }

    /// Execute a plugin command
    pub fn execute_command(
        &self,
        editor: &mut Editor,
        name: &str,
        args: Vec<String>,
    ) -> Result<()> {
        self.engine.read().execute_command(editor, name, args)
    }

    /// Handle a UI/Picker callback from the editor
    pub fn handle_ui_callback(
        &self,
        editor: &mut Editor,
        plugin_name: String,
        callback_id: u64,
        value: serde_json::Value,
    ) -> Result<()> {
        let engine = self.engine.read();
        engine.handle_ui_callback(editor, plugin_name, callback_id, value)
    }

    /// Release a pending UI callback without calling it (e.g. the prompt was
    /// aborted or the picker was closed). Returns whether one was pending.
    pub fn release_ui_callback(&self, plugin_name: String, callback_id: u64) -> bool {
        // `try_read` so that dropping a UI component while the engine is being
        // reloaded can never deadlock; a reload clears all callbacks anyway.
        match self.engine.try_read() {
            Some(engine) => engine.release_ui_callback(plugin_name, callback_id),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager_creation() {
        let config = PluginConfig::default();
        let manager = PluginManager::new(config);
        assert!(manager.is_ok());
    }

    #[test]
    fn test_disabled_plugin_system() {
        let config = PluginConfig {
            enabled: false,
            ..Default::default()
        };

        let manager = PluginManager::new(config).unwrap();
        assert!(!manager.is_enabled());
    }
}
