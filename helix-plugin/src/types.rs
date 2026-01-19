use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents a plugin event type that can be subscribed to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    /// Plugin system initialized
    OnInit,
    /// Editor ready
    OnReady,
    /// Buffer opened
    OnBufferOpen,
    /// Buffer is about to be saved
    OnBufferPreSave,
    /// Buffer was saved
    OnBufferPostSave,
    /// Buffer closed
    OnBufferClose,
    /// Buffer content changed
    OnBufferChanged,
    /// Editor mode changed
    OnModeChange,
    /// Key was pressed
    OnKeyPress,
    /// LSP attached to buffer
    OnLspAttach,
    /// LSP diagnostics received
    OnLspDiagnostic,
    /// LSP server initialized
    OnLspInitialized,
    /// Selection changed
    OnSelectionChange,
    /// View/window changed
    OnViewChange,
}

impl EventType {
    /// Returns the event name as a string for Lua
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OnInit => "init",
            Self::OnReady => "ready",
            Self::OnBufferOpen => "buffer_open",
            Self::OnBufferPreSave => "buffer_pre_save",
            Self::OnBufferPostSave => "buffer_post_save",
            Self::OnBufferClose => "buffer_close",
            Self::OnBufferChanged => "buffer_changed",
            Self::OnModeChange => "mode_change",
            Self::OnKeyPress => "key_press",
            Self::OnLspAttach => "lsp_attach",
            Self::OnLspDiagnostic => "lsp_diagnostic",
            Self::OnLspInitialized => "lsp_initialized",
            Self::OnSelectionChange => "selection_change",
            Self::OnViewChange => "view_change",
        }
    }
}

impl std::str::FromStr for EventType {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "init" => Ok(Self::OnInit),
            "ready" => Ok(Self::OnReady),
            "buffer_open" => Ok(Self::OnBufferOpen),
            "buffer_pre_save" => Ok(Self::OnBufferPreSave),
            "buffer_post_save" => Ok(Self::OnBufferPostSave),
            "buffer_close" => Ok(Self::OnBufferClose),
            "buffer_changed" => Ok(Self::OnBufferChanged),
            "mode_change" => Ok(Self::OnModeChange),
            "key_press" => Ok(Self::OnKeyPress),
            "lsp_attach" => Ok(Self::OnLspAttach),
            "lsp_diagnostic" => Ok(Self::OnLspDiagnostic),
            "lsp_initialized" => Ok(Self::OnLspInitialized),
            "selection_change" => Ok(Self::OnSelectionChange),
            "view_change" => Ok(Self::OnViewChange),
            _ => Err(()),
        }
    }
}

/// Plugin event data
#[derive(Debug, Clone)]
pub struct PluginEvent {
    pub event_type: EventType,
    pub data: EventData,
}

/// Event data variants
#[derive(Debug, Clone)]
pub enum EventData {
    /// No data
    None,
    /// Buffer-related data
    Buffer {
        document_id: helix_view::DocumentId,
        path: Option<PathBuf>,
    },
    /// Mode change data
    ModeChange { old_mode: String, new_mode: String },
    /// Key press data
    KeyPress { key: String },
    /// LSP attach data
    LspAttach {
        document_id: helix_view::DocumentId,
        language_server_id: usize,
    },
    /// LSP diagnostic data
    LspDiagnostic {
        document_id: helix_view::DocumentId,
        diagnostic_count: usize,
    },
}

/// Plugin metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// Plugin name
    pub name: String,
    /// Plugin version
    pub version: String,
    /// Plugin description
    pub description: Option<String>,
    /// Plugin author
    pub author: Option<String>,
    /// Plugin entry point (default: init.lua)
    pub entry: Option<String>,
}

impl Default for PluginMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "0.1.0".to_string(),
            description: None,
            author: None,
            entry: Some("init.lua".to_string()),
        }
    }
}

/// Represents a loaded plugin
#[derive(Debug)]
pub struct Plugin {
    /// Plugin metadata
    pub metadata: PluginMetadata,
    /// Plugin root path
    pub path: PathBuf,
    /// Whether the plugin is enabled
    pub enabled: bool,
}

/// Plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Whether plugins are enabled globally
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Plugin directories to search
    #[serde(default)]
    pub plugin_dirs: Vec<PathBuf>,
    /// Individual plugin configurations
    #[serde(default)]
    pub plugins: Vec<IndividualPluginConfig>,
}

fn default_true() -> bool {
    true
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plugin_dirs: vec![],
            plugins: vec![],
        }
    }
}

/// Configuration for an individual plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualPluginConfig {
    /// Plugin name
    pub name: String,
    /// Whether this plugin is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Plugin-specific configuration
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Metadata for a registered command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMetadata {
    /// Command name
    pub name: String,
    /// Command documentation/help text
    pub doc: String,
    /// Arguments description (optional)
    pub args: Option<String>,
}

/// Interface for executing builtin editor commands from plugins
pub trait EditorCommandRegistry: Send + Sync {
    fn execute(
        &self,
        editor: &mut helix_view::Editor,
        name: &str,
        args: &[String],
    ) -> std::result::Result<(), anyhow::Error>;
}

/// Wrapper for EditorCommandRegistry to store in Lua app data
pub struct CommandRegistryWrapper(pub std::sync::Arc<dyn EditorCommandRegistry>);

/// Interface for handling UI elements (prompts, pickers) that require compositor access
pub trait UiHandler: Send + Sync {
    fn prompt(
        &self,
        editor: &mut helix_view::Editor,
        message: String,
        default: Option<String>,
        plugin_name: String,
        callback_id: u64,
    );
    fn confirm(
        &self,
        editor: &mut helix_view::Editor,
        message: String,
        plugin_name: String,
        callback_id: u64,
    );
    fn picker(
        &self,
        editor: &mut helix_view::Editor,
        items: Vec<String>,
        prompt: String,
        plugin_name: String,
        callback_id: u64,
    );
}

/// Wrapper for UiHandler to store in Lua app data
pub struct UiHandlerWrapper(pub std::sync::Arc<dyn UiHandler>);

/// Wrapper for UI callbacks to store in Lua app data
pub struct UiCallbackRegistry(
    pub  std::sync::Arc<
        parking_lot::RwLock<std::collections::HashMap<(String, u64), mlua::RegistryKey>>,
    >,
);

/// Wrapper for UI callback counter to store in Lua app data
pub struct UiCallbackCounter(pub std::sync::Arc<std::sync::atomic::AtomicU64>);
