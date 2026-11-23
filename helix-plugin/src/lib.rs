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
    PluginMetadata, TextChange,
};

use helix_view::Editor;

/// The columns a plugin picker may have.
pub const MAX_PICKER_COLUMNS: usize = 8;
use log::info;
use parking_lot::RwLock;
use std::sync::Arc;

/// The main plugin manager
pub struct PluginManager {
    /// The Lua engine
    engine: Arc<RwLock<LuaEngine>>,
    /// Plugin configuration (`:config-reload` puts a new one in, see
    /// [`PluginManager::update_config`])
    config: RwLock<PluginConfig>,
}

/// What [`PluginManager::update_config`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigUpdate {
    /// The settings are the same (or plugins are off): the plugins were left as they are.
    Unchanged,
    /// The plugins were loaded again, with the new settings.
    Reloaded,
    /// Plugins were switched on or off (`enabled`), which takes a restart: nothing changed.
    NeedsRestart,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(config: PluginConfig) -> Result<Self> {
        let mut engine = LuaEngine::new()?;
        engine.register_api(config.clone())?;

        Ok(Self {
            engine: Arc::new(RwLock::new(engine)),
            config: RwLock::new(config),
        })
    }

    /// Returns true if the plugin system is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.read().enabled
    }

    /// Puts `config` (the `[plugins]` section read again) in place of the settings the plugins
    /// run with: when they differ, the plugins are loaded again with it, as
    /// `:reload-all-plugins` does, so a plugin's new `config`, its permissions and the
    /// directories take effect. Switching plugins on or off (`enabled`) takes a restart.
    pub fn update_config(&self, editor: &mut Editor, config: PluginConfig) -> Result<ConfigUpdate> {
        {
            let current = self.config.read();
            if *current == config {
                return Ok(ConfigUpdate::Unchanged);
            }
            if current.enabled != config.enabled {
                return Ok(ConfigUpdate::NeedsRestart);
            }
        }
        let enabled = config.enabled;
        *self.config.write() = config;
        if !enabled {
            return Ok(ConfigUpdate::Unchanged);
        }
        self.reload_plugins(editor)?;
        Ok(ConfigUpdate::Reloaded)
    }

    /// Initialize and load all plugins
    pub fn initialize(&self, editor: &mut Editor) -> Result<()> {
        // Determine plugin directories
        let configured_dirs = self.config.read().plugin_dirs.clone();
        let plugin_dirs = if configured_dirs.is_empty() {
            lua::loader::PluginLoader::default_plugin_dirs()
        } else {
            configured_dirs
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
        // Their bindings go with them (a function binding's command is gone), and what they
        // drew: they set it again as they load.
        editor.plugin_keymaps.clear();
        editor.plugin_statusline.clear();
        editor.plugin_window_bar.clear();
        editor.plugin_floats.clear();
        editor.sync_window_bars();
        editor.plugin_explorer.decorations.clear();
        editor.plugin_actions.clear();
        editor.plugin_completion_sources.clear();
        editor.plugin_quickfix_marks.clear();
        for session in &mut editor.diff_sessions {
            session.plugin_marks.clear();
        }
        for entries in editor.diagnostics.values_mut() {
            entries.retain(|(_, provider)| {
                !matches!(
                    provider,
                    helix_core::diagnostic::DiagnosticProvider::Plugin { .. }
                )
            });
        }
        for doc in editor.documents.values_mut() {
            doc.clear_plugin_diagnostics();
            doc.plugin_signs.clear();
            doc.plugin_gutters.clear();
            doc.plugin_highlights.clear();
            doc.plugin_annotations.clear();
            doc.plugin_panels.clear();
            doc.plugin_overlays.clear();
            doc.plugin_line_numbers.clear();
            doc.plugin_scrollbar_marks.clear();
        }
        if !editor.plugin_theme_overrides.is_empty() {
            editor.plugin_theme_overrides.clear();
            match editor.theme_loader.load(editor.theme.name()) {
                Ok(theme) => {
                    if let Err(e) = editor.set_theme(theme) {
                        log::error!("Plugins reloaded: theme not restored: {e}");
                    }
                }
                Err(e) => log::error!("Plugins reloaded: theme not restored: {e}"),
            }
        }
        // Reset engine state
        {
            let mut engine = self.engine.write();
            engine.reset()?;

            // Re-register API
            engine.register_api(self.config.read().clone())?;
        }

        // Re-initialize (discover and load)
        self.initialize(editor)
    }

    /// Check if a plugin is enabled in the configuration
    fn is_plugin_enabled(&self, name: &str) -> bool {
        // If there's specific config for this plugin, use that
        if let Some(plugin_config) = self.config.read().plugins.iter().find(|p| p.name == name) {
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

    /// Runs the handlers of a hook event now: whether what it guards may go on (no handler
    /// returned `false`). Plugin code that is running (a command calling `helix.editor.run`)
    /// is not interrupted by its own hooks: they are skipped, and it goes on.
    pub fn run_hook(&self, editor: &mut Editor, event: PluginEvent) -> bool {
        if !event.event_type.is_subscribed() {
            return true;
        }
        // (running plugin code holds the engine)
        if self.engine.try_write().is_none() {
            return true;
        }
        self.engine.read().call_hook(editor, &event)
    }

    /// Calls the handlers of the events plugins emitted (`helix.emit`), and of the ones these
    /// emit in turn, a few rounds at most: more wait for the next frame.
    pub fn fire_custom_events(&self, editor: &mut Editor) {
        for _ in 0..8 {
            let engine = self.engine.read();
            if !engine.has_custom_events() {
                return;
            }
            engine.fire_custom_events(editor);
        }
    }

    /// Get plugin configuration for a specific plugin
    pub fn get_plugin_config(&self, name: &str) -> Option<serde_json::Value> {
        self.config
            .read()
            .plugins
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.config.clone())
    }

    /// Get the Lua engine (for advanced operations)
    pub fn engine(&self) -> Arc<RwLock<LuaEngine>> {
        Arc::clone(&self.engine)
    }

    /// Runs action `id` of `plugin` with `context` (see [`LuaEngine::run_action`]).
    pub fn run_action(
        &self,
        editor: &mut Editor,
        plugin: &str,
        id: u64,
        context: &helix_view::plugin_ui::ActionContext,
    ) -> Result<()> {
        self.engine.read().run_action(editor, plugin, id, context)
    }

    /// Runs function `function` of float `float` of `plugin` (a key of the float, or its
    /// `on_close`).
    pub fn run_float(
        &self,
        editor: &mut Editor,
        plugin: &str,
        float: u64,
        function: u64,
    ) -> Result<()> {
        self.engine
            .read()
            .run_float(editor, plugin, float, function)
    }

    /// Drops the functions of a float of `plugin` that was closed.
    pub fn forget_float(&self, plugin: &str, float: u64) {
        self.engine.read().forget_float(plugin, float)
    }

    /// Asks the completion sources of the plugins that want `request` (see
    /// [`helix_view::plugin_ui::PluginCompletionSource::wanted`]): the items of each, with the
    /// source's name. A source that fails is logged and skipped.
    pub fn complete(
        &self,
        editor: &mut Editor,
        request: &helix_view::plugin_ui::PluginCompletionRequest,
    ) -> Vec<(String, Vec<helix_view::plugin_ui::PluginCompletionItem>)> {
        let language = editor
            .document(request.document)
            .and_then(|doc| doc.language_name().map(String::from));
        let sources: Vec<_> = editor
            .plugin_completion_sources
            .iter()
            .filter(|source| {
                source.wanted(
                    language.as_deref(),
                    &request.before,
                    &request.prefix,
                    request.manual,
                )
            })
            .cloned()
            .collect();
        let mut found = Vec::new();
        for source in sources {
            match self
                .engine
                .read()
                .complete(editor, &source.plugin, source.id, request)
            {
                Ok(items) => found.push((source.name, items)),
                Err(err) => log::error!(
                    "completion source '{}' of {} failed: {err}",
                    source.name,
                    source.plugin
                ),
            }
        }
        found
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

    /// Runs the callback of a plugin's timer that is due.
    pub fn fire_timer(&self, editor: &mut Editor, plugin_name: &str, timer_id: u64) -> Result<()> {
        self.engine.read().fire_timer(editor, plugin_name, timer_id)
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
