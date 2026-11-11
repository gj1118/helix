use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Defines [`EventType`] with the name of each event in Lua, in one place.
macro_rules! event_types {
    ($($(#[$doc:meta])* $variant:ident => $name:literal,)*) => {
        /// An editor event plugins subscribe to with `helix.on(name, handler)`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[repr(u8)]
        pub enum EventType {
            $($(#[$doc])* $variant,)*
        }

        impl EventType {
            /// Every event.
            pub const ALL: &'static [EventType] = &[$(EventType::$variant,)*];

            /// The event name in Lua (`"buffer_open"`).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(EventType::$variant => $name,)*
                }
            }
        }

        impl std::str::FromStr for EventType {
            type Err = ();

            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                match s {
                    $($name => Ok(EventType::$variant),)*
                    _ => Err(()),
                }
            }
        }
    };
}

event_types! {
    /// The plugins were loaded (also after `:reload-all-plugins`).
    OnInit => "init",
    /// Helix has started (after `buffer_open` of the files of the command line).
    OnReady => "ready",
    /// Helix is about to exit.
    OnExit => "exit",
    /// A file was opened.
    OnBufferOpen => "buffer_open",
    /// A buffer is about to be written.
    OnBufferPreSave => "buffer_pre_save",
    /// A buffer was written.
    OnBufferPostSave => "buffer_post_save",
    /// Writing a buffer failed.
    OnBufferSaveFailed => "buffer_save_failed",
    /// A buffer was closed.
    OnBufferClose => "buffer_close",
    /// The text of a buffer changed.
    OnBufferChanged => "buffer_changed",
    /// The path of a buffer changed (`:write <path>`, `:move`).
    OnBufferRename => "buffer_rename",
    /// The focus left a buffer.
    OnBufferFocusLost => "buffer_focus_lost",
    /// The language of a buffer changed.
    OnLanguageChange => "language_change",
    /// The editor mode changed.
    OnModeChange => "mode_change",
    /// A key was pressed.
    OnKeyPress => "key_press",
    /// A character was typed in insert mode.
    OnInsertChar => "insert_char",
    /// A command ran (a key binding, `:` command or macro).
    OnCommand => "command",
    /// The selections of a buffer changed.
    OnSelectionChange => "selection_change",
    /// A window scrolled.
    OnScroll => "scroll",
    /// A window was opened.
    OnWindowOpen => "window_open",
    /// A window was closed.
    OnWindowClose => "window_close",
    /// Another window got the focus.
    OnWindowFocus => "window_focus",
    /// The terminal was resized.
    OnViewChange => "view_change",
    /// The terminal got the focus.
    OnFocusGained => "focus_gained",
    /// The terminal lost the focus.
    OnFocusLost => "focus_lost",
    /// Text was pasted into the terminal (bracketed paste).
    OnPaste => "paste",
    /// A mouse button or wheel was used.
    OnMouse => "mouse",
    /// Nothing happened for `idle-timeout`.
    OnIdle => "idle",
    /// The editor settings changed (`:set`, `:config-reload`).
    OnConfigChange => "config_change",
    /// The theme changed.
    OnThemeChange => "theme_change",
    /// The working directory changed.
    OnCwdChange => "cwd_change",
    /// A language server finished initializing.
    OnLspInitialized => "lsp_initialized",
    /// A language server was attached to a buffer.
    OnLspAttach => "lsp_attach",
    /// A language server was detached from a buffer.
    OnLspDetach => "lsp_detach",
    /// A language server exited.
    OnLspExit => "lsp_exit",
    /// The diagnostics of a buffer changed.
    OnLspDiagnostic => "lsp_diagnostic",
    /// The explorer created a file or folder.
    OnFileCreate => "file_create",
    /// The explorer renamed or moved a file or folder.
    OnFileRename => "file_rename",
    /// The explorer copied a file or folder.
    OnFileCopy => "file_copy",
    /// The explorer deleted a file or folder.
    OnFileDelete => "file_delete",
    /// The cursor of the explorer moved to another path.
    OnExplorerSelect => "explorer_select",
}

/// How many plugin handlers each event has, over every plugin engine of the process: the
/// editor does not even queue an event no handler wants (`insert_char`, `command` and
/// `mouse` come with every key or click).
static SUBSCRIBERS: [std::sync::atomic::AtomicUsize; 64] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 64];

impl EventType {
    /// Whether some plugin has a handler for this event.
    pub fn is_subscribed(self) -> bool {
        SUBSCRIBERS[self as usize].load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    pub(crate) fn subscribe(self) {
        SUBSCRIBERS[self as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn unsubscribe(self, handlers: usize) {
        SUBSCRIBERS[self as usize].fetch_sub(handlers, std::sync::atomic::Ordering::Relaxed);
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
    /// Named fields (the table handed to the handlers has these keys).
    Fields(Vec<(&'static str, EventValue)>),
}

/// A field of an event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventValue {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    /// A buffer: its id, as `buffer:id()` returns it.
    Document(helix_view::DocumentId),
    /// A window: its id, as `window:id()` returns it.
    View(helix_view::ViewId),
}

impl From<&str> for EventValue {
    fn from(value: &str) -> Self {
        EventValue::Str(value.to_string())
    }
}

impl From<String> for EventValue {
    fn from(value: String) -> Self {
        EventValue::Str(value)
    }
}

impl From<Option<String>> for EventValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(EventValue::Nil, EventValue::Str)
    }
}

impl From<bool> for EventValue {
    fn from(value: bool) -> Self {
        EventValue::Bool(value)
    }
}

impl From<i64> for EventValue {
    fn from(value: i64) -> Self {
        EventValue::Int(value)
    }
}

impl From<helix_view::DocumentId> for EventValue {
    fn from(value: helix_view::DocumentId) -> Self {
        EventValue::Document(value)
    }
}

impl From<helix_view::ViewId> for EventValue {
    fn from(value: helix_view::ViewId) -> Self {
        EventValue::View(value)
    }
}

impl PluginEvent {
    /// An event with named fields.
    pub fn with_fields(event_type: EventType, fields: Vec<(&'static str, EventValue)>) -> Self {
        PluginEvent {
            event_type,
            data: EventData::Fields(fields),
        }
    }
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
    /// What the plugin needs (`["edit", "files"]`): it gets no more, whatever is granted.
    /// Without the list it gets what `config.toml` grants (and the defaults).
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
}

impl Default for PluginMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "0.1.0".to_string(),
            description: None,
            author: None,
            entry: Some("init.lua".to_string()),
            permissions: None,
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
    /// What the plugin may do beyond the defaults (`read`, `ui`): `["edit", "files"]`,
    /// `["all"]`.
    #[serde(default)]
    pub permissions: crate::permissions::Permissions,
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
    /// A command made for a key binding to a Lua function (`helix.keymap.set`): not listed
    /// in the command palette.
    #[serde(default)]
    pub hidden: bool,
}

/// Interface for executing builtin editor commands from plugins
pub trait EditorCommandRegistry: Send + Sync {
    fn execute(
        &self,
        editor: &mut helix_view::Editor,
        name: &str,
        args: &[String],
    ) -> std::result::Result<(), anyhow::Error>;

    /// What running the typable command `name` (or an alias) with `args` needs, besides the
    /// `commands` permission that `helix.editor.execute_command` needs for any command;
    /// `None` when there is no such command.
    fn permissions(
        &self,
        name: &str,
        args: &[String],
    ) -> Option<Vec<crate::permissions::Permission>>;

    /// Runs the command of key bindings `name` (`"move_line_down"`) `count` times.
    fn run_static(
        &self,
        editor: &mut helix_view::Editor,
        name: &str,
        count: usize,
    ) -> std::result::Result<(), anyhow::Error>;

    /// What running the command of key bindings `name` needs, besides `commands`; `None` when
    /// there is no such command.
    fn static_permissions(&self, name: &str) -> Option<Vec<crate::permissions::Permission>>;

    /// What binding a key to `command` (as in `[keys]`: `"move_line_down"`, `":write"`,
    /// `"@ihello<esc>"`) needs: the permissions the command needs when the key is pressed.
    /// An error for a command that does not exist.
    fn binding_permissions(
        &self,
        command: &str,
    ) -> std::result::Result<Vec<crate::permissions::Permission>, String>;
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
    /// Opens a picker of structured items; the callback gets `{index, action}`.
    fn open_picker(
        &self,
        _editor: &mut helix_view::Editor,
        _picker: PluginPicker,
        _plugin_name: String,
        _callback_id: u64,
    ) {
    }
    /// Adds items to the open picker `picker_id` (nothing once it is closed).
    fn picker_add(&self, _picker_id: u64, _items: Vec<PluginPickerItem>) {}
    /// Closes the picker `picker_id` if it is open.
    fn picker_close(&self, _picker_id: u64) {}
    /// Shows the quicklist in the quickfix panel (`open`), or closes the panel.
    fn quickfix(&self, _open: bool) {}
}

/// A picker a plugin opens (`helix.ui.picker`).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginPicker {
    /// Identifies the picker for `add` and `close` of its handle.
    pub id: u64,
    pub title: String,
    pub prompt: String,
    /// The column names; one column without a header when empty.
    pub columns: Vec<String>,
    pub items: Vec<PluginPickerItem>,
    /// Whether to preview the items' files or buffers.
    pub preview: bool,
    /// In the quickfix panel at the bottom instead of over the editor.
    pub docked: bool,
}

/// An item of a plugin picker.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginPickerItem {
    /// Its index in the plugin's list (1-based), handed back when it is chosen.
    pub index: usize,
    /// The text of each column.
    pub columns: Vec<String>,
    /// A file to preview (and to open by the plugin)...
    pub path: Option<std::path::PathBuf>,
    /// ...or a buffer.
    pub document: Option<helix_view::DocumentId>,
    /// The lines to show in the preview (0-based, inclusive).
    pub lines: Option<(usize, usize)>,
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
