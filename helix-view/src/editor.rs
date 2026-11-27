use crate::terminal_launcher::Launcher;
use crate::{
    annotations::diagnostics::{DiagnosticFilter, InlineDiagnosticsConfig},
    clipboard::ClipboardProvider,
    document::{
        DocumentOpenError, DocumentSavedEventFuture, DocumentSavedEventResult, Mode, SavePoint,
    },
    events::{DocumentDidClose, DocumentDidOpen, DocumentFocusLost},
    explorer::{ExplorerClipboard, ExplorerConfig},
    graphics::{CursorKind, Rect, Style},
    handlers::Handlers,
    info::Info,
    input::KeyEvent,
    persistence::{self, FileHistoryEntry},
    quicklist::Quicklist,
    regex::EqRegex,
    register::Registers,
    snippets::Snippets,
    theme::{self, Theme},
    tree::{self, Dimension, Resize, Tree},
    view::{Jump, ViewPosition},
    Document, DocumentId, View, ViewId,
};
use helix_event::dispatch;
use helix_loader::workspace_trust::{ImplicitTrustLevel, TrustQuery, WorkspaceTrust};
use helix_vcs::DiffProviderRegistry;
use regex::Regex;

use futures_util::stream::select_all::SelectAll;
use futures_util::StreamExt;
use helix_lsp::{Call, LanguageServerId};
use tokio_stream::wrappers::UnboundedReceiverStream;

use std::{
    borrow::Cow,
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    io::{self, stdin},
    num::{NonZeroU8, NonZeroUsize},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::{sleep, Duration, Instant, Sleep},
};

use anyhow::{anyhow, bail, Error};

pub use helix_core::diagnostic::Severity;
use helix_core::{
    auto_pairs::AutoPairs,
    diagnostic::DiagnosticProvider,
    syntax::{
        self,
        config::{AutoPairConfig, IndentationHeuristic, LanguageServerFeature, SoftWrap},
    },
    Change, LineEnding, Position, Range, Selection, Uri, NATIVE_LINE_ENDING,
};
use helix_dap::{self as dap, registry::DebugAdapterId};
use helix_lsp::lsp;
use helix_stdx::path::canonicalize;

use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};

use arc_swap::{
    access::{DynAccess, DynGuard},
    ArcSwap,
};

pub const DIR_STACK_CAP: usize = 10;
pub const DEFAULT_AUTO_SAVE_DELAY: u64 = 3000;
pub const DEFAULT_AUTO_RELOAD_INTERVAL: u64 = 3000;

fn deserialize_duration_millis<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = u64::deserialize(deserializer)?;
    Ok(Duration::from_millis(millis))
}

fn serialize_duration_millis<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(
        duration
            .as_millis()
            .try_into()
            .map_err(|_| serde::ser::Error::custom("duration value overflowed u64"))?,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct GutterConfig {
    /// Gutter Layout
    pub layout: Vec<GutterType>,
    /// Options specific to the "line-numbers" gutter
    pub line_numbers: GutterLineNumbersConfig,
}

impl Default for GutterConfig {
    fn default() -> Self {
        Self {
            layout: vec![
                GutterType::Diagnostics,
                GutterType::Spacer,
                GutterType::LineNumbers,
                GutterType::Spacer,
                GutterType::Diff,
                GutterType::Folds,
            ],
            line_numbers: GutterLineNumbersConfig::default(),
        }
    }
}

impl From<Vec<GutterType>> for GutterConfig {
    fn from(x: Vec<GutterType>) -> Self {
        GutterConfig {
            layout: x,
            ..Default::default()
        }
    }
}

/// `popup-max-width-percent`: any number, brought into 10..=100.
fn deserialize_popup_width_percent<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Int(i64),
        Float(f64),
    }
    Ok(match Number::deserialize(deserializer)? {
        Number::Int(n) => n.clamp(10, 100) as u8,
        Number::Float(n) if n.is_nan() => 60,
        Number::Float(n) => n.round().clamp(10.0, 100.0) as u8,
    })
}

/// `cursor-roundness`: any number, brought into 0..=50 (a value out of range must not make
/// the whole config fail to load).
fn deserialize_cursor_roundness<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Int(i64),
        Float(f64),
    }
    Ok(match Number::deserialize(deserializer)? {
        Number::Int(n) => n.clamp(0, 50) as u8,
        // NaN becomes 0
        Number::Float(n) => n.round().clamp(0.0, 50.0) as u8,
    })
}

fn deserialize_gutter_seq_or_struct<'de, D>(deserializer: D) -> Result<GutterConfig, D::Error>
where
    D: Deserializer<'de>,
{
    struct GutterVisitor;

    impl<'de> serde::de::Visitor<'de> for GutterVisitor {
        type Value = GutterConfig;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(
                formatter,
                "an array of gutter names or a detailed gutter configuration"
            )
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: serde::de::SeqAccess<'de>,
        {
            let mut gutters = Vec::new();
            while let Some(gutter) = seq.next_element::<String>()? {
                gutters.push(
                    gutter
                        .parse::<GutterType>()
                        .map_err(serde::de::Error::custom)?,
                )
            }

            Ok(gutters.into())
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let deserializer = serde::de::value::MapAccessDeserializer::new(map);
            Deserialize::deserialize(deserializer)
        }
    }

    deserializer.deserialize_any(GutterVisitor)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct GutterLineNumbersConfig {
    /// Minimum number of characters to use for line number gutter. Defaults to 3.
    pub min_width: usize,
    /// When true, line numbers use the same color based on the mode,
    /// ignoring the selection state (selected vs not selected).
    pub same_color: bool,
}

impl Default for GutterLineNumbersConfig {
    fn default() -> Self {
        Self {
            min_width: 3,
            same_color: false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InlineBlameShow {
    /// Do not show inline blame, and do not request it in the background
    ///
    /// When manually requesting the inline blame, it may take several seconds to appear.
    Never,
    /// Show the inline blame on the cursor line
    CursorLine,
    /// Show the inline blame on every other line
    AllLines,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct InlineBlameConfig {
    /// How to show the inline blame
    pub show: InlineBlameShow,
    /// Whether the inline blame should be fetched in the background
    pub auto_fetch: bool,
    /// How the inline blame should look like and the information it includes
    pub format: String,
}

impl Default for InlineBlameConfig {
    fn default() -> Self {
        Self {
            show: InlineBlameShow::Never,
            format: "{author}, {time-ago} • {title} • {commit}".to_owned(),
            auto_fetch: false,
        }
    }
}

/// The mouse pointer shape requested from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    /// The terminal's own pointer.
    #[default]
    Default,
    /// Over a vertical resize handle (dragged left and right).
    ColResize,
    /// Over a horizontal resize handle (dragged up and down).
    RowResize,
    /// Over a corner handle running from top left to bottom right.
    NwseResize,
    /// Over a corner handle running from top right to bottom left.
    NeswResize,
    /// Over a handle that moves a window.
    Move,
}

impl PointerShape {
    /// The CSS cursor name used by the terminal's pointer shape escape
    /// sequence; empty means "restore the terminal's pointer".
    pub fn name(self) -> &'static str {
        match self {
            PointerShape::Default => "",
            PointerShape::ColResize => "col-resize",
            PointerShape::RowResize => "row-resize",
            PointerShape::NwseResize => "nwse-resize",
            PointerShape::NeswResize => "nesw-resize",
            PointerShape::Move => "move",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FilePickerConfig {
    /// IgnoreOptions
    /// Enables ignoring hidden files.
    /// Whether to hide hidden files in file picker and global search results. Defaults to true.
    pub hidden: bool,
    /// Enables following symlinks.
    /// Whether to follow symbolic links in file picker and file or directory completions. Defaults to true.
    pub follow_symlinks: bool,
    /// Hides symlinks that point into the current directory. Defaults to true.
    pub deduplicate_links: bool,
    /// Enables reading ignore files from parent directories. Defaults to true.
    pub parents: bool,
    /// Enables reading `.ignore` files.
    /// Whether to hide files listed in .ignore in file picker and global search results. Defaults to true.
    pub ignore: bool,
    /// Enables reading `.gitignore` files.
    /// Whether to hide files listed in .gitignore in file picker and global search results. Defaults to true.
    pub git_ignore: bool,
    /// Enables reading global .gitignore, whose path is specified in git's config: `core.excludefile` option.
    /// Whether to hide files listed in global .gitignore in file picker and global search results. Defaults to true.
    pub git_global: bool,
    /// Enables reading `.git/info/exclude` files.
    /// Whether to hide files listed in .git/info/exclude in file picker and global search results. Defaults to true.
    pub git_exclude: bool,
    /// WalkBuilder options
    /// Maximum Depth to recurse directories in file picker and global search. Defaults to `None`.
    pub max_depth: Option<usize>,
    /// Whether to hide the preview panel. Defaults to false.
    pub hide_preview: bool,
}

impl Default for FilePickerConfig {
    fn default() -> Self {
        Self {
            hidden: true,
            follow_symlinks: true,
            deduplicate_links: true,
            parents: true,
            ignore: true,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
            max_depth: None,
            hide_preview: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FileExplorerConfig {
    /// IgnoreOptions
    /// Enables ignoring hidden files.
    /// Whether to hide hidden files in file explorer and global search results. Defaults to false.
    pub hidden: bool,
    /// Enables following symlinks.
    /// Whether to follow symbolic links in file picker and file or directory completions. Defaults to false.
    pub follow_symlinks: bool,
    /// Enables reading ignore files from parent directories. Defaults to false.
    pub parents: bool,
    /// Enables reading `.ignore` files.
    /// Whether to hide files listed in .ignore in file picker and global search results. Defaults to false.
    pub ignore: bool,
    /// Enables reading `.gitignore` files.
    /// Whether to hide files listed in .gitignore in file picker and global search results. Defaults to false.
    pub git_ignore: bool,
    /// Enables reading global .gitignore, whose path is specified in git's config: `core.excludefile` option.
    /// Whether to hide files listed in global .gitignore in file picker and global search results. Defaults to false.
    pub git_global: bool,
    /// Enables reading `.git/info/exclude` files.
    /// Whether to hide files listed in .git/info/exclude in file picker and global search results. Defaults to false.
    pub git_exclude: bool,
    /// Whether to flatten single-child directories in file explorer. Defaults to true.
    pub flatten_dirs: bool,
}

impl Default for FileExplorerConfig {
    fn default() -> Self {
        Self {
            hidden: false,
            follow_symlinks: false,
            parents: false,
            ignore: false,
            git_ignore: false,
            git_global: false,
            git_exclude: false,
            flatten_dirs: true,
        }
    }
}

fn serialize_alphabet<S>(alphabet: &[char], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let alphabet: String = alphabet.iter().collect();
    serializer.serialize_str(&alphabet)
}

fn deserialize_alphabet<'de, D>(deserializer: D) -> Result<Vec<char>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let str = String::deserialize(deserializer)?;
    let chars: Vec<_> = str.chars().collect();
    let unique_chars: HashSet<_> = chars.iter().copied().collect();
    if unique_chars.len() != chars.len() {
        return Err(<D::Error as Error>::custom(
            "jump-label-alphabet must contain unique characters",
        ));
    }
    Ok(chars)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrolloffConfig {
    pub horizontal: usize,
    pub vertical: usize,
}

fn deserialize_scrolloff_config<'de, D>(deserializer: D) -> Result<ScrolloffConfig, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Debug, Deserialize)]
    #[serde(untagged)]
    enum ScrolloffConfigDe {
        Legacy(usize),
        Split(ScrolloffConfig),
    }

    let scrolloff = ScrolloffConfigDe::deserialize(deserializer)?;
    match scrolloff {
        ScrolloffConfigDe::Legacy(u) => Ok(ScrolloffConfig {
            horizontal: u,
            vertical: u,
        }),
        ScrolloffConfigDe::Split(s) => Ok(s),
    }
}

impl Default for ScrolloffConfig {
    fn default() -> Self {
        Self {
            horizontal: 5,
            vertical: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Config {
    /// Whether to enable the welcome screen
    pub welcome_screen: bool,
    /// Whether Helix listens on a socket of its own for `hx --remote` (Unix only).
    pub remote_control: bool,
    /// Whether Helix sets the window title of the terminal (to `title_format`). Off by default:
    /// the terminal (or the shell) keeps its own.
    pub set_title: bool,
    /// The window title when `set_title` is on. Supports expansions (`%{buffer_name}`, ...),
    /// except shell commands: the title is expanded for every frame.
    pub title_format: String,
    /// Padding to keep between the edge of the screen and the cursor when scrolling. Defaults to 5.
    #[serde(deserialize_with = "deserialize_scrolloff_config")]
    pub scrolloff: ScrolloffConfig,
    /// Number of lines to scroll at once. Defaults to 3
    pub scroll_lines: isize,
    /// Mouse support. Defaults to true.
    pub mouse: bool,
    /// Which register to use for mouse yank.
    pub mouse_yank_register: char,
    /// Shell to use for shell commands. Defaults to ["cmd", "/C"] on Windows and ["sh", "-c"] otherwise.
    pub shell: Vec<String>,
    /// Line number mode.
    pub line_number: LineNumber,
    /// Symbol to use for the picker. Defaults to ">".
    pub picker_symbol: String,
    /// Where a picker's preview goes when it is stacked with the list (a window taller than
    /// wide, or too narrow for both side by side): `above` or `below` the list.
    pub picker_stacked_preview: StackedPreview,
    /// Highlight the lines cursors are currently on. Defaults to false.
    pub cursorline: bool,
    /// Highlight the columns cursors are currently on. Defaults to false.
    pub cursorcolumn: bool,
    /// How round the corners of block cursors are, as in 4coder's `cursor_roundness`: the
    /// radius as a percentage of the cursor's width, from 0 (square, as the terminal draws
    /// it) to 50 (the left and right ends are half circles). Drawn as an image with the kitty
    /// graphics protocol; where the terminal doesn't have it, block cursors stay square.
    /// Any number is accepted: below 0 counts as 0, above 50 as 50.
    #[serde(deserialize_with = "deserialize_cursor_roundness")]
    pub cursor_roundness: u8,
    #[serde(deserialize_with = "deserialize_gutter_seq_or_struct")]
    pub gutters: GutterConfig,
    /// Middle click paste support. Defaults to true.
    pub middle_click_paste: bool,
    /// Automatic insertion of pairs to parentheses, brackets,
    /// etc. Optionally, this can be a list of 2-tuples to specify a
    /// global list of characters to pair. Defaults to true.
    pub auto_pairs: AutoPairConfig,
    /// Automatic auto-completion, automatically pop up without user trigger. Defaults to true.
    pub auto_completion: bool,
    /// Enable filepath completion.
    /// Show files and directories if an existing path at the cursor was recognized,
    /// either absolute or relative to the current opened document or current working directory (if the buffer is not yet saved).
    /// Defaults to true.
    pub path_completion: bool,
    /// Configures completion of words from open buffers.
    /// Defaults to enabled with a trigger length of 7.
    pub word_completion: WordCompletion,
    /// Offer user-defined snippets (see `snippets.toml`) in the completion menu.
    pub snippet_completion: bool,
    /// Automatic formatting on save. Defaults to true.
    pub auto_format: bool,
    /// Default register used for yank/paste. Defaults to '"'
    pub default_yank_register: char,
    /// How long (in milliseconds) yanked text is highlighted (`ui.highlight.yank`), like
    /// Neovim's `vim.hl.on_yank`. 0 turns it off. Defaults to 200.
    pub yank_highlight: u64,
    /// Smooth scrolling: the scroll commands glide to their position instead of jumping.
    pub smooth_scroll: SmoothScrollConfig,
    /// The word motions (`w`, `b`, `e`, `ge`) stop at the parts of `camelCase` and
    /// `snake_case` names, like nvim-spider. Off by default.
    pub subword_motions: bool,
    /// A buffer opens where its cursor was last: a file opened again (after `:buffer-close`,
    /// or after a restart with `[editor.persistence] old-files`), and a window showing a
    /// buffer for the first time. Off: they start at the start of the file. On by default.
    pub remember_cursor_position: bool,
    /// Automatic save on focus lost and/or after delay.
    /// Time delay in milliseconds since last edit after which auto save timer triggers.
    /// Time delay defaults to false with 3000ms delay. Focus lost defaults to false.
    #[serde(deserialize_with = "deserialize_auto_save")]
    pub auto_save: AutoSave,
    /// Automatic reload of the modified documents on a periodic time interval and/or when the editor gains focus.
    /// Time interval defaults to false with 3000ms delay. Focus gained defaults to false.
    #[serde(deserialize_with = "deserialize_auto_reload")]
    pub auto_reload: AutoReload,
    /// Set a global text_width
    pub text_width: usize,
    /// Time in milliseconds since last keypress before idle timers trigger.
    /// Used for various UI timeouts. Defaults to 250ms.
    #[serde(
        serialize_with = "serialize_duration_millis",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub idle_timeout: Duration,
    /// Time in milliseconds after typing a word character before auto completions
    /// are shown, set to 5 for instant. Defaults to 250ms.
    #[serde(
        serialize_with = "serialize_duration_millis",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub completion_timeout: Duration,
    /// Whether to insert the completion suggestion on hover. Defaults to true.
    pub preview_completion_insert: bool,
    pub completion_trigger_len: u8,
    /// Whether to instruct the LSP to replace the entire word when applying a completion
    /// or to only insert new text
    pub completion_replace: bool,
    /// `true` if helix should automatically add a line comment token if you're currently in a comment
    /// and press `enter`.
    pub continue_comments: bool,
    /// Whether to display infoboxes. Defaults to true.
    pub auto_info: bool,
    pub file_picker: FilePickerConfig,
    /// Configuration of the bufferline
    pub bufferline: BufferLineConfig,
    /// Persistently display a breadcrumb navigation bar along the top of each view
    pub breadcrumb: BreadcrumbConfig,
    /// A scrollbar on the right of each view, with marks of the cursor, search matches,
    /// diagnostics, git changes and plugins (like nvim-scrollbar).
    pub scrollbar: ScrollbarConfig,
    pub inline_completion: InlineCompletionConfig,
    /// Configuration of the file explorer
    pub file_explorer: FileExplorerConfig,
    /// Configuration of the file tree explorer panel.
    pub explorer: ExplorerConfig,
    /// Configuration of the statusline elements
    pub statusline: StatusLineConfig,
    /// Shape for cursor in each mode
    pub cursor_shape: CursorShapeConfig,
    /// Set to `true` to override automatic detection of terminal truecolor support in the event of a false negative. Defaults to `false`.
    pub true_color: bool,
    /// Set to `true` to override automatic detection of terminal undercurl support in the event of a false negative. Defaults to `false`.
    pub undercurl: bool,
    /// Search configuration.
    #[serde(default)]
    pub search: SearchConfig,
    pub lsp: LspConfig,
    /// Debugger display options.
    #[serde(default)]
    pub debugger: DebuggerConfig,
    /// Images in the picker preview.
    #[serde(default)]
    pub image_preview: crate::image_preview::ImagePreviewConfig,
    /// The `[keys.picker]`, `[keys.menu]`, `[keys.popup]` and `[keys.prompt]` tables. They are
    /// not part of `[editor]`: the application fills them in when it loads the config.
    #[serde(skip)]
    pub component_keys: crate::component_keys::ComponentKeys,
    pub terminal: Option<TerminalConfig>,
    /// Column numbers at which to draw the rulers. Defaults to `[]`, meaning no rulers.
    pub rulers: Vec<u16>,
    /// Character used to render rulers in the foreground. Defaults to "" (background-style rulers).
    /// Set to empty string "" to use background-style rulers instead of a glyph.
    pub ruler_char: String,
    #[serde(default)]
    pub whitespace: WhitespaceConfig,
    /// Vertical indent width guides.
    pub indent_guides: IndentGuidesConfig,
    /// Whether to color modes with different colors. Defaults to `false`.
    pub color_modes: bool,
    pub soft_wrap: SoftWrap,
    /// Workspace specific lsp ceiling dirs
    pub workspace_lsp_roots: Vec<PathBuf>,
    /// Which line ending to choose for new documents. Defaults to `native`. i.e. `crlf` on Windows, otherwise `lf`.
    pub default_line_ending: LineEndingConfig,
    /// Whether to automatically insert a trailing line-ending on write if missing. Defaults to `true`.
    pub insert_final_newline: bool,
    /// Whether to use atomic operations to write documents to disk.
    /// This prevents data loss if the editor is interrupted while writing the file, but may
    /// confuse some file watching/hot reloading programs. Defaults to `true`.
    pub atomic_save: bool,
    /// Whether to automatically remove all trailing line-endings after the final one on write.
    /// Defaults to `false`.
    pub trim_final_newlines: bool,
    /// Whether to automatically remove all whitespace characters preceding line-endings on write.
    /// Defaults to `false`.
    pub trim_trailing_whitespace: bool,
    /// Enables smart tab
    pub smart_tab: Option<SmartTabConfig>,
    /// Draw border around popups.
    pub popup_border: PopupBorderConfig,
    /// The most columns a popup at the cursor (hover, signature help) takes, as a percentage of the width of the buffer's window: 10 to 100
    /// (values out of range are brought into it). Its text wraps to it. Defaults to 60.
    #[serde(deserialize_with = "deserialize_popup_width_percent")]
    pub popup_max_width_percent: u8,
    /// Draw rounded border corners. Superseded by `border-style`, which takes
    /// priority when set.
    pub rounded_corners: bool,
    /// The glyphs of panel, popup and menu borders and of separators.
    pub border_style: Option<BorderStyle>,
    /// User-defined command-mode commands. Filled from the top-level
    /// `[commands]` table when the configuration is loaded, not from `[editor]`.
    #[serde(skip)]
    pub commands: std::collections::BTreeMap<String, crate::custom_commands::CustomCommand>,
    /// Which indent heuristic to use when a new line is inserted
    #[serde(default)]
    pub indent_heuristic: IndentationHeuristic,
    /// labels characters used in jumpmode
    #[serde(
        serialize_with = "serialize_alphabet",
        deserialize_with = "deserialize_alphabet"
    )]
    pub jump_label_alphabet: Vec<char>,
    /// Display diagnostic below the line they occur.
    pub inline_diagnostics: InlineDiagnosticsConfig,
    pub end_of_line_diagnostics: DiagnosticFilter,
    // Set to override the default clipboard provider
    pub clipboard_provider: ClipboardProvider,
    /// Whether to read settings from [EditorConfig](https://editorconfig.org) files. Defaults to
    /// `true`.
    pub editor_config: bool,
    /// Maximum width for panel resizing. Set to 0 for dynamic limit based on terminal width. Defaults to 50.
    pub max_panel_width: usize,
    /// Maximum height for panel resizing. Set to 0 for dynamic limit based on terminal height. Defaults to 50.
    pub max_panel_height: usize,
    /// Maximum panel width as percentage of terminal width (0.0-1.0). Used when max_panel_width is 0. Defaults to 0.8.
    pub max_panel_width_percent: f32,
    /// Maximum panel height as percentage of terminal height (0.0-1.0). Used when max_panel_height is 0. Defaults to 0.8.
    pub max_panel_height_percent: f32,
    /// Inline blame allows showing the latest commit that affected the line the cursor is on as virtual text
    #[serde(default)]
    pub inline_blame: InlineBlameConfig,
    /// Side-by-side diffs (`:diff`, `:diff-git`, `hx --diff`)
    pub diff: crate::diff_view::DiffConfig,
    /// Whether to render rainbow colors for matching brackets. Defaults to `false`.
    pub rainbow_brackets: bool,
    /// Whether to enable Kitty Keyboard Protocol
    pub kitty_keyboard_protocol: KittyKeyboardProtocolConfig,
    /// Command line configuration
    #[serde(default)]
    pub cmdline: CmdlineConfig,
    /// The quickfix panel, for lists of locations (`[editor.quickfix]`).
    pub quickfix: QuickfixConfig,
    /// The recently used files (`[editor.recent-picker]`).
    pub recent_picker: RecentPickerConfig,
    /// Picker gradient border configuration
    #[serde(default)]
    pub gradient_borders: GradientBorderConfig,
    /// Whether to show pickers in fullscreen mode. When true, pickers expand to
    /// full screen width on terminals narrower than 200 columns. Defaults to false.
    pub fullscreen_overlay: bool,
    /// Notification system configuration
    #[serde(default)]
    pub notifications: NotificationConfig,
    /// Completion Highlight configuration
    #[serde(default)]
    pub completion_highlight: CompletionHighlight,
    pub buffer_picker: BufferPickerConfig,
    /// Defines which text objects will be folded when a document is opened.
    #[serde(default)]
    pub fold_textobjects: Vec<String>,
    /// Deprecated: `insecure = true` is read as `[editor.workspace-trust] level = "insecure"`
    /// (see `Config::apply_deprecated_options`). Still accepted so older configs keep loading.
    #[serde(skip_serializing)]
    pub insecure: bool,
    /// Workspace-trust configuration.
    pub workspace_trust: WorkspaceTrustConfig,
    /// Cross-session persistence of command/search history, clipboard, and file positions.
    pub persistence: PersistenceConfig,
}

/// User-facing configuration for `[editor.workspace-trust]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorkspaceTrustConfig {
    /// What to trust implicitly without an explicit grant.
    pub level: ImplicitTrustLevel,
    /// Whether opening a file in an untrusted workspace surfaces the trust modal. The statusline
    /// `[⚠]` indicator is always shown either way; disabling the prompt is for users who would
    /// rather act explicitly via `:workspace-trust` than be interrupted. Defaults to `true`.
    pub prompt: bool,
    /// Glob patterns whose matching workspaces are implicitly trusted.
    pub trusted: Vec<String>,
}

impl Default for WorkspaceTrustConfig {
    fn default() -> Self {
        Self {
            level: ImplicitTrustLevel::default(),
            prompt: true,
            trusted: Vec::new(),
        }
    }
}

impl From<&WorkspaceTrustConfig> for helix_loader::workspace_trust::Config {
    fn from(v: &WorkspaceTrustConfig) -> Self {
        Self {
            level: v.level,
            prompt: v.prompt,
            trusted_globs: helix_loader::workspace_trust::build_trusted_globs(&v.trusted),
        }
    }
}

impl Config {
    /// Maps options kept only for compatibility onto their replacements. Returns a
    /// warning for each one that was set.
    pub fn apply_deprecated_options(&mut self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if std::mem::take(&mut self.insecure) {
            self.workspace_trust.level = ImplicitTrustLevel::Insecure;
            warnings.push(
                "`[editor] insecure` is deprecated, use `[editor.workspace-trust] level = \"insecure\"`",
            );
        }
        warnings
    }

    pub fn code_action_hint(&self) -> bool {
        self.gutters.layout.contains(&GutterType::CodeActionHint)
            || self
                .statusline
                .left
                .contains(&StatusLineElement::CodeActionHint)
            || self
                .statusline
                .center
                .contains(&StatusLineElement::CodeActionHint)
            || self
                .statusline
                .right
                .contains(&StatusLineElement::CodeActionHint)
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub struct BufferPickerConfig {
    pub start_position: PickerStartPosition,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum PickerStartPosition {
    #[default]
    Current,
    Previous,
}

impl PickerStartPosition {
    #[must_use]
    pub fn is_previous(self) -> bool {
        matches!(self, Self::Previous)
    }

    #[must_use]
    pub fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum KittyKeyboardProtocolConfig {
    #[default]
    Auto,
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct CmdlineConfig {
    /// Command line display style. Options: "bottom", "popup". Defaults to "bottom".
    pub style: CmdlineStyle,
    /// Enable command icons in cmdline. Defaults to true.
    pub show_icons: bool,
    /// Minimum width for popup cmdline. Defaults to 40.
    pub min_popup_width: u16,
    /// Maximum width for popup cmdline. Defaults to 80.
    pub max_popup_width: u16,
    /// Use full height when cmdline style is popup (removes the bottom space). Defaults to false.
    pub use_full_height: bool,
    /// Where the popup is on the screen. Defaults to `default` (the upper third).
    pub position: CmdlinePosition,
    /// Customizable icons for different command types.
    pub icons: CmdlineIcons,
}

impl Default for CmdlineConfig {
    fn default() -> Self {
        Self {
            style: CmdlineStyle::Bottom,
            show_icons: true,
            min_popup_width: 40,
            max_popup_width: 80,
            use_full_height: false,
            position: CmdlinePosition::Default,
            icons: CmdlineIcons::default(),
        }
    }
}

/// `[editor.recent-picker]`: the recently used files, for the recent file picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct RecentPickerConfig {
    /// Note the files opened and saved. Off: the picker shows what was noted before.
    pub enable: bool,
    /// The most files the picker shows.
    pub max_show_entries: usize,
    /// The most files the file keeps.
    pub max_save_entries: usize,
    /// The file the recent files are kept in (shared by the editors running).
    pub recents_path: PathBuf,
}

impl Default for RecentPickerConfig {
    fn default() -> Self {
        Self {
            enable: true,
            max_show_entries: 50,
            max_save_entries: 100,
            recents_path: helix_loader::default_recents_file(),
        }
    }
}

/// `[editor.quickfix]`: lists of locations (search results, diagnostics, references,
/// symbols, ...) in a panel at the bottom, like Vim's quickfix window, instead of a picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct QuickfixConfig {
    /// Show the lists of locations in the panel. Off by default: they are pickers.
    pub enable: bool,
    /// The rows of the panel, its border included.
    pub height: u16,
    /// While the panel has the focus, preview the selected location over the code.
    pub preview: bool,
}

impl Default for QuickfixConfig {
    fn default() -> Self {
        Self {
            enable: false,
            height: 12,
            preview: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct CmdlineIcons {
    /// Icon for search commands (/,?). Defaults to "🔍".
    pub search: String,
    /// Icon for command mode (:). Defaults to "🛠️".
    pub command: String,
    /// Icon for shell commands (!). Defaults to "⚡".
    pub shell: String,
    /// Icon for general prompts. Defaults to "💬".
    pub general: String,
}

impl Default for CmdlineIcons {
    fn default() -> Self {
        Self {
            search: "🔍".to_string(),
            command: "🛠️".to_string(),
            shell: "⚡".to_string(),
            general: "💬".to_string(),
        }
    }
}

/// Where a picker's preview goes when it is stacked with the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StackedPreview {
    /// Above the list, the prompt and the list at the bottom (as upstream Helix).
    #[default]
    Above,
    /// Below the prompt and the list.
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CmdlineStyle {
    /// Traditional bottom command line
    #[default]
    Bottom,
    /// Centered popup window (noice.nvim style)
    Popup,
}

/// `position` of `[editor.cmdline]`: where the command line popup is, centered across the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CmdlinePosition {
    /// In the upper third of the screen.
    #[default]
    Default,
    /// At the top, below the first row (the bufferline's).
    Top,
    /// In the middle.
    Center,
    /// At the bottom, above the last row; its completions and help above it.
    Bottom,
    /// Right below the line of the cursor, or right above it when there is no room below (its
    /// completions and help above it then).
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct GradientBorderConfig {
    /// Enable gradient borders for pickers. Defaults to false.
    pub enable: bool,
    /// Border thickness (1-5). Defaults to 1.
    pub thickness: u8,
    /// Gradient direction. Options: "horizontal", "vertical", "diagonal". Defaults to "horizontal".
    pub direction: GradientDirection,
    /// Start color (in hex format like "#FF0000"). Defaults to "#8A2BE2".
    pub start_color: String,
    /// End color (in hex format like "#FF0000"). Defaults to "#00BFFF".
    pub end_color: String,
    /// Middle color for 3-color gradients (optional). Defaults to "".
    pub middle_color: String,
    /// Animation speed (0-10, 0 = disabled). Defaults to 0.
    pub animation_speed: u8,
}

impl Default for GradientBorderConfig {
    fn default() -> Self {
        Self {
            enable: false,
            thickness: 1,
            direction: GradientDirection::Horizontal,
            start_color: "#8A2BE2".to_string(), // BlueViolet
            end_color: "#00BFFF".to_string(),   // DeepSkyBlue
            middle_color: "".to_string(),
            animation_speed: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GradientDirection {
    /// Left to right gradient
    #[default]
    Horizontal,
    /// Top to bottom gradient
    Vertical,
    /// Diagonal gradient (top-left to bottom-right)
    Diagonal,
    /// Radial gradient from center
    Radial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct NotificationConfig {
    /// Enable notification system. Defaults to true.
    pub enable: bool,
    /// Notification display style. Options: "popup", "statusline". Defaults to "popup".
    pub style: NotificationStyle,
    /// Maximum number of notifications to keep in history. Defaults to 100.
    pub max_history: usize,
    /// Default timeout for notifications in milliseconds. 0 = no timeout. Defaults to 5000.
    #[serde(
        serialize_with = "serialize_duration_millis",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub default_timeout: Duration,
    /// Position for popup notifications. Defaults to "top-right".
    pub position: NotificationPosition,
    /// Maximum width for notification popups. Defaults to 60.
    pub max_width: u16,
    /// Maximum height for notification popups. Defaults to 10.
    pub max_height: u16,
    /// Show notification icons. Defaults to true.
    pub show_icons: bool,
    /// Notification icons for different severity levels.
    pub icons: NotificationIcons,
    /// Padding inside the notification content area
    pub padding: u16,
    /// Show notification emojis. Defaults to true.
    pub show_emojis: bool,
    /// A header above the message: the icon and the severity ("Warning"), the time on the
    /// right, and a line under them. Defaults to true.
    pub header: bool,
    /// The time in the header. Defaults to true.
    pub show_time: bool,
    /// Notification emojis for different severity levels.
    pub emojis: NotificationEmojis,
    /// Enable notification history command. Defaults to true.
    pub enable_history: bool,
    /// Border configuration for notifications.
    pub border: NotificationBorderConfig,
    /// Drop shadow configuration for notifications
    pub shadow: NotificationShadowConfig,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enable: true,
            style: NotificationStyle::Popup,
            max_history: 100,
            default_timeout: Duration::from_millis(5000),
            position: NotificationPosition::TopRight,
            max_width: 60,
            max_height: 10,
            show_icons: true,
            icons: NotificationIcons::default(),
            padding: 1,
            show_emojis: true,
            header: true,
            show_time: true,
            emojis: NotificationEmojis::default(),
            enable_history: true,
            border: NotificationBorderConfig::default(),
            shadow: NotificationShadowConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationStyle {
    /// Show notifications as popup windows (noice.nvim style)
    #[default]
    Popup,
    /// Show notifications in statusline (traditional style)
    Statusline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationPosition {
    /// Top-left corner
    TopLeft,
    /// Top-center
    TopCenter,
    /// Top-right corner
    #[default]
    TopRight,
    /// Bottom-left corner
    BottomLeft,
    /// Bottom-center
    BottomCenter,
    /// Bottom-right corner
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct NotificationIcons {
    /// Icon for info notifications. Defaults to "ℹ".
    pub info: String,
    /// Icon for warning notifications. Defaults to "⚠".
    pub warning: String,
    /// Icon for error notifications. Defaults to "✗".
    pub error: String,
    /// Icon for success notifications. Defaults to "✓".
    pub success: String,
}

impl Default for NotificationIcons {
    fn default() -> Self {
        Self {
            info: "ℹ".to_string(),
            warning: "⚠".to_string(),
            error: "✗".to_string(),
            success: "✓".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct NotificationEmojis {
    /// Emoji for info notifications. Defaults to "💡".
    pub info: String,
    /// Emoji for warning notifications. Defaults to "⚠️".
    pub warning: String,
    /// Emoji for error notifications. Defaults to "❌".
    pub error: String,
    /// Emoji for success notifications. Defaults to "✅".
    pub success: String,
}

impl Default for NotificationEmojis {
    fn default() -> Self {
        Self {
            info: "💡".to_string(),
            warning: "⚠️".to_string(),
            error: "❌".to_string(),
            success: "✅".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct NotificationBorderConfig {
    /// Enable borders for notifications. Defaults to true.
    pub enable: bool,
    /// Border width (1-5). Defaults to 1.
    pub width: u8,
    /// Border radius for rounded corners (0-10). Defaults to 2.
    pub radius: u8,
    /// Draw the border with a gradient from `gradient_start` to `gradient_end` (whatever
    /// `[editor.gradient-borders]` says; its thickness and direction are used). Defaults to
    /// false.
    pub gradient: bool,
    /// Gradient start color (in hex format like "#FF0000"). Defaults to "#8A2BE2".
    pub gradient_start: String,
    /// Gradient end color (in hex format like "#FF0000"). Defaults to "#00BFFF".
    pub gradient_end: String,
    /// Border glyphs: "solid" (those of the editor's `border-style`), "dashed", "dotted", or
    /// one of the `border-style` values for notifications only. Defaults to "solid".
    pub style: NotificationBorderStyle,
}

impl Default for NotificationBorderConfig {
    fn default() -> Self {
        Self {
            enable: true,
            width: 1,
            radius: 2,
            gradient: false,
            gradient_start: "#8A2BE2".to_string(), // BlueViolet
            gradient_end: "#00BFFF".to_string(),   // DeepSkyBlue
            style: NotificationBorderStyle::Solid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationBorderStyle {
    /// The glyphs of the editor's `border-style` (with `radius` rounding a plain border).
    #[default]
    Solid,
    /// Dashed lines (`┄ ┆`), with square corners, or rounded ones when `radius` is above 0.
    Dashed,
    /// Dotted lines (`┈ ┊`), with square corners, or rounded ones when `radius` is above 0.
    Dotted,
    /// The glyphs of a [`BorderStyle`], for notifications only.
    None,
    Plain,
    Rounded,
    Double,
    Thick,
    QuadrantInside,
    QuadrantOutside,
}

impl NotificationBorderStyle {
    /// The [`BorderStyle`] this style names, if it names one.
    pub fn border_style(self) -> Option<BorderStyle> {
        match self {
            Self::Solid | Self::Dashed | Self::Dotted => Option::None,
            Self::None => Some(BorderStyle::None),
            Self::Plain => Some(BorderStyle::Plain),
            Self::Rounded => Some(BorderStyle::Rounded),
            Self::Double => Some(BorderStyle::Double),
            Self::Thick => Some(BorderStyle::Thick),
            Self::QuadrantInside => Some(BorderStyle::QuadrantInside),
            Self::QuadrantOutside => Some(BorderStyle::QuadrantOutside),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct NotificationShadowConfig {
    /// Enable drop shadow
    pub enable: bool,
    /// Shadow offset on x and y axes
    pub offset_x: u16,
    pub offset_y: u16,
}

impl Default for NotificationShadowConfig {
    fn default() -> Self {
        Self {
            enable: true,
            offset_x: 1,
            offset_y: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct DebuggerConfig {
    /// Show the values of the variables a line uses at the end of that line while the debuggee
    /// is stopped. Hidden for files with unsaved changes. Defaults to `true`.
    pub inline_values: bool,
}

impl Default for DebuggerConfig {
    fn default() -> Self {
        Self {
            inline_values: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Eq, PartialOrd, Ord)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct SmartTabConfig {
    pub enable: bool,
    pub supersede_menu: bool,
}

impl Default for SmartTabConfig {
    fn default() -> Self {
        SmartTabConfig {
            enable: true,
            supersede_menu: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct TerminalConfig {
    /// Open the debuggee's terminal with this terminal or multiplexer. Takes precedence over
    /// `command` and `args`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    /// The terminal to run the debuggee in (`command args... <debuggee>`), for a terminal that
    /// has no launcher.
    pub command: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl TerminalConfig {
    pub fn with_launcher(launcher: Launcher) -> Self {
        Self {
            launcher: Some(launcher),
            ..Self::default()
        }
    }
}

#[cfg(windows)]
pub fn get_terminal_provider() -> Option<TerminalConfig> {
    use helix_stdx::env::binary_exists;

    if binary_exists("wt") {
        return Some(TerminalConfig {
            launcher: None,
            command: "wt".to_string(),
            args: vec![
                "new-tab".to_string(),
                "--title".to_string(),
                "DEBUG".to_string(),
                "cmd".to_string(),
                "/C".to_string(),
            ],
        });
    }

    Some(TerminalConfig {
        launcher: None,
        command: "conhost".to_string(),
        args: vec!["cmd".to_string(), "/C".to_string()],
    })
}

#[cfg(not(any(windows, target_arch = "wasm32")))]
pub fn get_terminal_provider() -> Option<TerminalConfig> {
    use helix_stdx::env::{binary_exists, env_var_is_set};

    Launcher::detect(env_var_is_set, |program| binary_exists(program))
        .map(TerminalConfig::with_launcher)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct LspConfig {
    /// Enables LSP
    pub enable: bool,
    /// Display LSP messagess from $/progress below statusline
    pub display_progress_messages: bool,
    /// Display LSP messages from window/showMessage below statusline
    pub display_messages: bool,
    /// Enable automatic pop up of signature help (parameter hints)
    pub auto_signature_help: bool,
    /// Display docs under signature help popup
    pub display_signature_help_docs: bool,
    /// Position of signature help popup relative to cursor: "above" or "below"
    pub signature_help_position: SignatureHelpPosition,
    /// Display inlay hints
    pub display_inlay_hints: bool,
    /// Automatically highlight symbol references at the cursor.
    pub auto_document_highlight: bool,
    /// Maximum displayed length of inlay hints (excluding the added trailing `…`).
    /// If it's `None`, there's no limit
    pub inlay_hints_length_limit: Option<NonZeroU8>,
    /// Display code lenses (run/test/reference-count labels) at the end of lines.
    /// Off by default: it makes an extra request to the language server per document.
    pub display_code_lenses: bool,
    /// Display document color swatches
    pub display_color_swatches: bool,
    /// Color swatches string. Defaults to `"■"`.
    pub color_swatches_string: String,
    /// Whether to enable snippet support
    pub snippets: bool,
    /// Whether to include declaration in the goto reference query
    pub goto_reference_include_declaration: bool,
    /// Collect diagnostics for the whole workspace in the workspace diagnostics picker by
    /// asking language servers for diagnostics of files that are not open.
    pub workspace_diagnostics: bool,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enable: true,
            display_progress_messages: false,
            display_messages: true,
            auto_signature_help: true,
            display_signature_help_docs: true,
            signature_help_position: SignatureHelpPosition::Above,
            display_inlay_hints: false,
            auto_document_highlight: false,
            inlay_hints_length_limit: None,
            snippets: true,
            goto_reference_include_declaration: true,
            display_code_lenses: false,
            display_color_swatches: true,
            color_swatches_string: "■".to_owned(),
            workspace_diagnostics: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureHelpPosition {
    Above,
    Below,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SearchConfig {
    /// Smart case: Case insensitive searching unless pattern contains upper case characters. Defaults to true.
    pub smart_case: bool,
    /// Whether the search should wrap after depleting the matches. Default to true.
    pub wrap_around: bool,
}

/// bufferline render modes
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferLineRenderMode {
    /// Don't render bufferline
    #[default]
    Never,
    /// Always render
    Always,
    /// Only if multiple buffers are open
    Multiple,
}

/// `[editor.smooth-scroll]`: the scroll commands (`C-d` / `C-u`, `C-f` / `C-b`, page up /
/// down, `zz` / `zt` / `zb`, a count of `C-e` / `C-y`) glide to their position, like
/// Neovim's neoscroll. Only the drawing is animated: the cursor and the view are already
/// where the command puts them, so the next key never waits for the animation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SmoothScrollConfig {
    /// On by default (`enable = false` turns it off).
    pub enable: bool,
    /// How long a scroll takes, in milliseconds (whatever its distance). 0 turns it off.
    pub duration: u64,
    /// How the speed changes along the way.
    pub easing: ScrollEasing,
}

impl Default for SmoothScrollConfig {
    fn default() -> Self {
        Self {
            enable: true,
            duration: 150,
            easing: ScrollEasing::EaseOut,
        }
    }
}

/// The speed of a smooth scroll along the way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollEasing {
    /// The same speed all along.
    Linear,
    /// Fast at first, slowing down at the end.
    #[default]
    EaseOut,
    /// Slow at both ends, fast in the middle.
    EaseInOut,
}

impl ScrollEasing {
    /// The part of the distance covered at `progress` (both from 0 to 1).
    pub fn apply(self, progress: f64) -> f64 {
        let p = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => p,
            Self::EaseOut => 1.0 - (1.0 - p).powi(3),
            Self::EaseInOut => {
                if p < 0.5 {
                    4.0 * p * p * p
                } else {
                    1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// Selections saved to be restored later (`A-Z`, `A-z`), like Kakoune's marks: in the text
/// of a history revision, mapped through the edits made since when restored.
#[derive(Debug, Clone)]
pub struct SavedSelections {
    pub document: DocumentId,
    pub revision: usize,
    pub selection: Selection,
}

/// A smooth scroll under way (see [`SmoothScrollConfig`]).
#[derive(Debug, Clone)]
pub struct ScrollAnimation {
    pub view: ViewId,
    pub document: DocumentId,
    /// The document's version: an edit ends the animation (the positions would be stale).
    pub version: i32,
    /// Where the view was.
    pub from: ViewPosition,
    /// Where the command put the view (the view's actual position all along).
    pub to: ViewPosition,
    /// The visual rows from `from` to `to` (negative upwards).
    pub rows: isize,
    pub start: Instant,
    pub duration: Duration,
    pub easing: ScrollEasing,
    /// Whether a frame was drawn yet: until then, a change of the view's position (the
    /// cursor kept in view after the command) is taken as the target.
    pub drawn: bool,
}

/// The most rows a smooth scroll goes through (a farther one jumps).
const SMOOTH_SCROLL_MAX_ROWS: isize = 2000;

/// Counts the smooth scrolls: the frame timer of one stops when it is no longer the latest
/// (ended, cancelled or followed by another).
static SMOOTH_SCROLL_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Which buffers the bufferline lists (and `gn` / `gp` and the buffer picker go through).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferLineScope {
    /// Every open buffer, whatever tab page shows it.
    Global,
    /// The buffers of the tab page shown: the ones opened or shown in it.
    #[default]
    Tab,
}

/// The `bufferline` setting: a table (`[editor.bufferline]`), or the upstream string form
/// (`bufferline = "multiple"`), which sets the render mode and leaves the other fields at
/// their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BufferLineConfig {
    pub render_mode: BufferLineRenderMode,
    pub separator: String,
    pub scope: BufferLineScope,
}

/// The table form of [`BufferLineConfig`].
#[derive(Deserialize)]
#[serde(
    rename = "BufferLineConfig",
    rename_all = "kebab-case",
    default,
    deny_unknown_fields
)]
struct BufferLineTable {
    render_mode: BufferLineRenderMode,
    separator: String,
    scope: BufferLineScope,
}

impl Default for BufferLineTable {
    fn default() -> Self {
        let BufferLineConfig {
            render_mode,
            separator,
            scope,
        } = BufferLineConfig::default();
        Self {
            render_mode,
            separator,
            scope,
        }
    }
}

impl<'de> Deserialize<'de> for BufferLineConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BufferLineConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(
                    f,
                    "\"always\", \"never\", \"multiple\" or a table with `render-mode`, `separator` and `scope`"
                )
            }

            fn visit_str<E>(self, mode: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                use serde::de::IntoDeserializer;
                Ok(BufferLineConfig {
                    render_mode: BufferLineRenderMode::deserialize(mode.into_deserializer())?,
                    ..BufferLineConfig::default()
                })
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let table = BufferLineTable::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(BufferLineConfig {
                    render_mode: table.render_mode,
                    separator: table.separator,
                    scope: table.scope,
                })
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Default for BufferLineConfig {
    fn default() -> Self {
        Self {
            render_mode: BufferLineRenderMode::default(),
            separator: String::from("│"),
            scope: BufferLineScope::default(),
        }
    }
}

/// How much path information is shown in the breadcrumb bar
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreadcrumbPathOptions {
    /// Show the full relative path before the symbols
    #[default]
    Full,
    /// Show only the file name
    File,
    /// Show no path information
    None,
}

/// Ghost-text suggestions from language servers that support `textDocument/inlineCompletion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct InlineCompletionConfig {
    /// Ask for suggestions while typing in insert mode. When disabled they are
    /// only requested by `inline_completion_trigger`.
    pub auto_trigger: bool,
    /// Time in milliseconds after typing before suggestions are requested.
    #[serde(
        serialize_with = "serialize_duration_millis",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub trigger_delay: Duration,
}

impl Default for InlineCompletionConfig {
    fn default() -> Self {
        Self {
            auto_trigger: true,
            trigger_delay: Duration::from_millis(150),
        }
    }
}

/// The scrollbar on the right of each view (`[editor.scrollbar]`), after nvim-scrollbar: its
/// settings, by the same names (`show` is `enable`; `excluded_filetypes` is
/// `excluded-languages`; colours come from the theme unless set here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbarConfig {
    /// Show the scrollbar (it takes the rightmost column of each view). Defaults to false.
    pub enable: bool,
    /// Draw it only in the focused view (the others keep the column empty).
    pub show_in_active_only: bool,
    /// No scrollbar in buffers of more lines than this; `0`: no limit.
    pub max_lines: usize,
    /// Nothing drawn while the whole buffer is on screen.
    pub hide_if_all_visible: bool,
    /// In buffers of at most this many lines, folded lines are left out of the bar (it
    /// spans the lines that can be seen); `0`: never.
    pub folds: usize,
    /// The least time in milliseconds between two searches of the buffer for the search
    /// marks (they run in the background).
    pub throttle_ms: u64,
    /// The search marks only while the cursor is on a match of the last search, as
    /// nvim-hlslens's `calm_down` (after `/`, `n`, `*`...); `false`: as long as there is a
    /// last search.
    pub search_calm_down: bool,
    /// Languages without a scrollbar, by their name in `languages.toml` (`"markdown"`).
    pub excluded_languages: Vec<String>,
    /// A click on the bar scrolls there, dragging the handle scrolls the view (with
    /// `editor.mouse`).
    pub mouse: bool,
    pub handle: ScrollbarHandleConfig,
    pub marks: ScrollbarMarksConfig,
    pub handlers: ScrollbarHandlersConfig,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        Self {
            enable: false,
            show_in_active_only: false,
            max_lines: 0,
            hide_if_all_visible: false,
            folds: 1000,
            throttle_ms: 100,
            search_calm_down: true,
            excluded_languages: Vec::new(),
            mouse: true,
            handle: ScrollbarHandleConfig::default(),
            marks: ScrollbarMarksConfig::default(),
            handlers: ScrollbarHandlersConfig::default(),
        }
    }
}

/// The handle: the part of the bar beside the lines on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbarHandleConfig {
    /// Drawn in each row of the handle without a mark.
    pub text: String,
    /// 0 to 100: how much of the editor's background shows through the handle's colour (0:
    /// none, 100: only the background).
    pub blend: u8,
    /// Its colour (`"#rrggbb"` or a colour name), instead of the theme's.
    pub color: Option<String>,
    /// The theme key whose background is its colour, instead of `ui.scrollbar.handle` (then
    /// `ui.cursorcolumn`, `ui.cursorline.primary`, `ui.selection`).
    pub highlight: Option<String>,
    /// No handle while the whole buffer is on screen.
    pub hide_if_all_visible: bool,
    /// Its ends move by eighths of a row (drawn with block characters) instead of whole
    /// rows, when its `text` is blank: it shows small scrolls and glides with smooth scroll.
    pub smooth: bool,
}

impl Default for ScrollbarHandleConfig {
    fn default() -> Self {
        Self {
            text: " ".to_string(),
            blend: 30,
            color: None,
            highlight: None,
            hide_if_all_visible: true,
            smooth: true,
        }
    }
}

/// The text of a mark: one, or one per level (the second for a row with more than one
/// mark, and so on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScrollbarMarkText {
    One(String),
    Levels(Vec<String>),
}

/// One type of mark; what is not set is the type's default (see `crate::scrollbar`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbarMarkConfig {
    pub text: Option<ScrollbarMarkText>,
    /// Of the marks on one row, the one of the lowest priority is drawn.
    pub priority: Option<u8>,
    /// Its colour (`"#rrggbb"` or a colour name), instead of the theme's.
    pub color: Option<String>,
    /// The theme key whose foreground is its colour, instead of its `ui.scrollbar.*` key.
    pub highlight: Option<String>,
    /// Modifiers of its text, as in themes (`["bold"]`): nvim-scrollbar's `gui`.
    pub modifiers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbarMarksConfig {
    pub cursor: ScrollbarMarkConfig,
    pub search: ScrollbarMarkConfig,
    pub error: ScrollbarMarkConfig,
    pub warn: ScrollbarMarkConfig,
    pub info: ScrollbarMarkConfig,
    pub hint: ScrollbarMarkConfig,
    pub misc: ScrollbarMarkConfig,
    pub git_add: ScrollbarMarkConfig,
    pub git_change: ScrollbarMarkConfig,
    pub git_delete: ScrollbarMarkConfig,
}

/// What puts marks on the bar (and whether the handle is drawn).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbarHandlersConfig {
    /// The primary cursor's line.
    pub cursor: bool,
    /// The diagnostics' lines.
    pub diagnostic: bool,
    /// The lines added, changed and deleted since the last commit.
    pub git: bool,
    /// The lines of the last search's matches.
    pub search: bool,
    /// The handle.
    pub handle: bool,
    /// The marks of plugins (`buffer:set_scrollbar_marks`).
    pub plugins: bool,
}

impl Default for ScrollbarHandlersConfig {
    fn default() -> Self {
        Self {
            cursor: true,
            diagnostic: true,
            git: true,
            search: true,
            handle: true,
            plugins: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct BreadcrumbConfig {
    pub enable: bool,
    pub path: BreadcrumbPathOptions,
    /// Maximum number of symbol levels shown; when exceeded the outermost
    /// symbols are elided with `…`. `0` means unlimited.
    pub max_depth: usize,
    /// Maximum length of a single crumb name; longer names are truncated in
    /// the middle with `…`. `0` means unlimited.
    pub max_name_length: usize,
}

impl Default for BreadcrumbConfig {
    fn default() -> Self {
        Self {
            enable: false,
            path: BreadcrumbPathOptions::Full,
            max_depth: 8,
            max_name_length: 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct StatusLineConfig {
    pub left: Vec<StatusLineElement>,
    pub center: Vec<StatusLineElement>,
    pub right: Vec<StatusLineElement>,
    pub separator: String,
    pub mode: ModeConfig,
    pub diagnostics: Vec<Severity>,
    pub workspace_diagnostics: Vec<Severity>,
}

impl Default for StatusLineConfig {
    fn default() -> Self {
        use StatusLineElement as E;

        Self {
            left: vec![
                E::Mode,
                E::Spinner,
                E::FileName,
                E::ReadOnlyIndicator,
                E::FileModificationIndicator,
            ],
            center: vec![],
            right: vec![
                E::DebugSession,
                E::LanguageServers,
                E::Diagnostics,
                E::Selections,
                E::Register,
                E::Position,
                E::FileEncoding,
            ],
            separator: String::from("│"),
            mode: ModeConfig::default(),
            diagnostics: vec![Severity::Warning, Severity::Error],
            workspace_diagnostics: vec![Severity::Warning, Severity::Error],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ModeConfig {
    pub normal: String,
    pub insert: String,
    pub select: String,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            normal: String::from("NOR"),
            insert: String::from("INS"),
            select: String::from("SEL"),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusLineElement {
    /// The editor mode (Normal, Insert, Visual/Selection)
    Mode,

    /// The LSP activity spinner
    Spinner,

    /// The file basename (the leaf of the open file's path)
    FileBaseName,

    /// The relative file path
    FileName,

    /// The file absolute path
    FileAbsolutePath,

    // The file modification indicator
    FileModificationIndicator,

    /// An indicator that shows `"[readonly]"` when a file cannot be written
    ReadOnlyIndicator,

    /// The file encoding
    FileEncoding,

    /// The file line endings (CRLF or LF)
    FileLineEnding,

    /// The file indentation style
    FileIndentStyle,

    /// The file type (language ID or "text")
    FileType,

    /// A summary of the number of errors and warnings
    Diagnostics,

    /// A summary of the number of errors and warnings on file and workspace
    WorkspaceDiagnostics,

    /// The number of selections (cursors)
    Selections,

    /// The number of characters currently in primary selection
    PrimarySelectionLength,

    /// The cursor position
    Position,

    /// The separator string
    Separator,

    /// The cursor position as a percent of the total file
    PositionPercentage,

    /// The total line numbers of the current file
    TotalLineNumbers,

    /// A single space
    Spacer,

    /// Current version control information
    VersionControl,

    /// Indicator for selected register
    Register,

    /// The base of current working directory
    CurrentWorkingDirectory,

    /// The current function name (from tree-sitter)
    FunctionName,
    /// Indicator for when code actions are available
    CodeActionHint,
    /// The selected debug session: its name, whether it is stopped and how many sessions there are
    DebugSession,
    /// The language server of the buffer (the first of its language's `language-servers`), and
    /// `+N` for the N others
    #[serde(alias = "lsp")]
    LanguageServers,
}

// Cursor shape is read and used on every rendered frame and so needs
// to be fast. Therefore we avoid a hashmap and use an enum indexed array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShapeConfig([CursorKind; 3]);

impl CursorShapeConfig {
    pub fn from_mode(&self, mode: Mode) -> CursorKind {
        self.get(mode as usize).copied().unwrap_or_default()
    }
}

impl<'de> Deserialize<'de> for CursorShapeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let m = HashMap::<Mode, CursorKind>::deserialize(deserializer)?;
        let into_cursor = |mode: Mode| m.get(&mode).copied().unwrap_or_default();
        Ok(CursorShapeConfig([
            into_cursor(Mode::Normal),
            into_cursor(Mode::Select),
            into_cursor(Mode::Insert),
        ]))
    }
}

impl Serialize for CursorShapeConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        let modes = [Mode::Normal, Mode::Select, Mode::Insert];
        for mode in modes {
            map.serialize_entry(&mode, &self.from_mode(mode))?;
        }
        map.end()
    }
}

impl std::ops::Deref for CursorShapeConfig {
    type Target = [CursorKind; 3];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for CursorShapeConfig {
    fn default() -> Self {
        Self([CursorKind::Block; 3])
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineNumber {
    /// Show absolute line number
    Absolute,

    /// If focused and in normal/select mode, show relative line number to the primary cursor.
    /// If unfocused or in insert mode, show absolute line number.
    Relative,
}

impl std::str::FromStr for LineNumber {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "absolute" | "abs" => Ok(Self::Absolute),
            "relative" | "rel" => Ok(Self::Relative),
            _ => anyhow::bail!("Line number can only be `absolute` or `relative`."),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GutterType {
    /// Show diagnostics and other features like breakpoints
    Diagnostics,
    /// Show line numbers
    LineNumbers,
    /// Show one blank space
    Spacer,
    /// Highlight local changes
    Diff,
    /// Indicator for when code actions are available
    CodeActionHint,
    /// The connector of a side-by-side diff, in front of the right side (added by the diff
    /// view, not configured)
    DiffConnector,
    /// The folds: `▸` on a closed fold, `▾` where a block could be folded; a click opens or
    /// closes it
    Folds,
}

impl std::str::FromStr for GutterType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "diagnostics" => Ok(Self::Diagnostics),
            "spacer" => Ok(Self::Spacer),
            "line-numbers" => Ok(Self::LineNumbers),
            "diff" => Ok(Self::Diff),
            "code-action-hint" => Ok(Self::CodeActionHint),
            "folds" => Ok(Self::Folds),
            _ => anyhow::bail!(
                "Gutter type can only be `diagnostics`, `spacer`, `line-numbers`, `diff`, \
                 `code-action-hint` or `folds`."
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhitespaceConfig {
    pub render: WhitespaceRender,
    pub characters: WhitespaceCharacters,
}

impl Default for WhitespaceConfig {
    fn default() -> Self {
        Self {
            render: WhitespaceRender::Basic(WhitespaceRenderValue::None),
            characters: WhitespaceCharacters::default(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "kebab-case")]
pub enum WhitespaceRender {
    Basic(WhitespaceRenderValue),
    Specific {
        default: Option<WhitespaceRenderValue>,
        space: Option<WhitespaceRenderValue>,
        nbsp: Option<WhitespaceRenderValue>,
        nnbsp: Option<WhitespaceRenderValue>,
        tab: Option<WhitespaceRenderValue>,
        newline: Option<WhitespaceRenderValue>,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhitespaceRenderValue {
    None,
    // TODO
    // Selection,
    All,
}

impl WhitespaceRender {
    pub fn space(&self) -> WhitespaceRenderValue {
        match *self {
            Self::Basic(val) => val,
            Self::Specific { default, space, .. } => {
                space.or(default).unwrap_or(WhitespaceRenderValue::None)
            }
        }
    }
    pub fn nbsp(&self) -> WhitespaceRenderValue {
        match *self {
            Self::Basic(val) => val,
            Self::Specific { default, nbsp, .. } => {
                nbsp.or(default).unwrap_or(WhitespaceRenderValue::None)
            }
        }
    }
    pub fn nnbsp(&self) -> WhitespaceRenderValue {
        match *self {
            Self::Basic(val) => val,
            Self::Specific { default, nnbsp, .. } => {
                nnbsp.or(default).unwrap_or(WhitespaceRenderValue::None)
            }
        }
    }
    pub fn tab(&self) -> WhitespaceRenderValue {
        match *self {
            Self::Basic(val) => val,
            Self::Specific { default, tab, .. } => {
                tab.or(default).unwrap_or(WhitespaceRenderValue::None)
            }
        }
    }
    pub fn newline(&self) -> WhitespaceRenderValue {
        match *self {
            Self::Basic(val) => val,
            Self::Specific {
                default, newline, ..
            } => newline.or(default).unwrap_or(WhitespaceRenderValue::None),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AutoSave {
    /// Auto save after a delay in milliseconds. Defaults to disabled.
    #[serde(default)]
    pub after_delay: AutoSaveAfterDelay,
    /// Auto save on focus lost. Defaults to false.
    #[serde(default)]
    pub focus_lost: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutoSaveAfterDelay {
    #[serde(default)]
    /// Enable auto save after delay. Defaults to false.
    pub enable: bool,
    #[serde(default = "default_auto_save_delay")]
    /// Time delay in milliseconds. Defaults to [DEFAULT_AUTO_SAVE_DELAY].
    pub timeout: u64,
}

impl Default for AutoSaveAfterDelay {
    fn default() -> Self {
        Self {
            enable: false,
            timeout: DEFAULT_AUTO_SAVE_DELAY,
        }
    }
}

fn default_auto_save_delay() -> u64 {
    DEFAULT_AUTO_SAVE_DELAY
}

fn deserialize_auto_save<'de, D>(deserializer: D) -> Result<AutoSave, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize, Serialize)]
    #[serde(untagged, deny_unknown_fields, rename_all = "kebab-case")]
    enum AutoSaveToml {
        EnableFocusLost(bool),
        AutoSave(AutoSave),
    }

    match AutoSaveToml::deserialize(deserializer)? {
        AutoSaveToml::EnableFocusLost(focus_lost) => Ok(AutoSave {
            focus_lost,
            ..Default::default()
        }),
        AutoSaveToml::AutoSave(auto_save) => Ok(auto_save),
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AutoReload {
    /// Whether to check for file changes when the editor is focused. Defaults to false.
    #[serde(default)]
    pub focus_gained: bool,
    /// Autosave periodically at some interval. Defaults to disabled.
    #[serde(default)]
    pub periodic: AutoReloadPeriodic,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutoReloadPeriodic {
    #[serde(default)]
    /// Enable auto reload periodically. Defaults to false.
    pub enable: bool,
    #[serde(default = "default_auto_reload_interval")]
    /// Time interval in milliseconds. Defaults to [DEFAULT_AUTO_RELOAD_INTERVAL].
    pub interval: u64,
}

pub fn default_auto_reload_interval() -> u64 {
    DEFAULT_AUTO_RELOAD_INTERVAL
}

fn deserialize_auto_reload<'de, D>(deserializer: D) -> Result<AutoReload, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize, Serialize)]
    #[serde(untagged, deny_unknown_fields, rename_all = "kebab-case")]
    enum AutoReloadToml {
        FocusGained(bool),
        AutoReload(AutoReload),
    }

    match AutoReloadToml::deserialize(deserializer)? {
        AutoReloadToml::FocusGained(focus_gained) => Ok(AutoReload {
            focus_gained,
            ..Default::default()
        }),
        AutoReloadToml::AutoReload(auto_reload) => Ok(auto_reload),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhitespaceCharacters {
    pub space: char,
    pub nbsp: char,
    pub nnbsp: char,
    pub tab: char,
    pub tabpad: char,
    pub newline: char,
}

impl Default for WhitespaceCharacters {
    fn default() -> Self {
        Self {
            space: '·',   // U+00B7
            nbsp: '⍽',    // U+237D
            nnbsp: '␣',   // U+2423
            tab: '→',     // U+2192
            newline: '⏎', // U+23CE
            tabpad: ' ',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct IndentGuidesConfig {
    pub render: bool,
    pub character: char,
    pub skip_levels: u8,
}

impl Default for IndentGuidesConfig {
    fn default() -> Self {
        Self {
            skip_levels: 0,
            render: false,
            character: '│',
        }
    }
}

/// Line ending configuration.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEndingConfig {
    /// The platform's native line ending.
    ///
    /// `crlf` on Windows, otherwise `lf`.
    #[default]
    Native,
    /// Line feed.
    LF,
    /// Carriage return followed by line feed.
    Crlf,
    /// Form feed.
    #[cfg(feature = "unicode-lines")]
    FF,
    /// Carriage return.
    #[cfg(feature = "unicode-lines")]
    CR,
    /// Next line.
    #[cfg(feature = "unicode-lines")]
    Nel,
}

impl From<LineEndingConfig> for LineEnding {
    fn from(line_ending: LineEndingConfig) -> Self {
        match line_ending {
            LineEndingConfig::Native => NATIVE_LINE_ENDING,
            LineEndingConfig::LF => LineEnding::LF,
            LineEndingConfig::Crlf => LineEnding::Crlf,
            #[cfg(feature = "unicode-lines")]
            LineEndingConfig::FF => LineEnding::FF,
            #[cfg(feature = "unicode-lines")]
            LineEndingConfig::CR => LineEnding::CR,
            #[cfg(feature = "unicode-lines")]
            LineEndingConfig::Nel => LineEnding::Nel,
        }
    }
}

/// The glyphs used for borders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BorderStyle {
    /// Draw no border glyphs (the space of the border is kept).
    None,
    Plain,
    Rounded,
    Double,
    Thick,
    QuadrantInside,
    QuadrantOutside,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PopupBorderConfig {
    None,
    All,
    Popup,
    Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WordCompletion {
    pub enable: bool,
    pub trigger_length: NonZeroU8,
}

impl Default for WordCompletion {
    fn default() -> Self {
        Self {
            enable: true,
            trigger_length: NonZeroU8::new(7).unwrap(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionHighlightType {
    Default,
    ThemeColors,
    Vibrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct CompletionHighlight {
    /// What kind of highlight type: "default", "theme-colors", "vibrant"
    pub highlight_type: CompletionHighlightType,
}

impl Default for CompletionHighlight {
    fn default() -> Self {
        Self {
            highlight_type: CompletionHighlightType::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct PersistenceConfig {
    pub old_files: bool,
    pub commands: bool,
    pub search: bool,
    pub clipboard: bool,
    pub old_files_exclusions: Vec<EqRegex>,
    pub old_files_trim: usize,
    pub commands_trim: usize,
    pub search_trim: usize,
    /// Keep each file's undo history when it is saved, and bring it back when the file is
    /// opened again with the same contents (persistent undo).
    pub undo: bool,
    /// Where the undo histories are kept; the `undo` folder of the state folder when unset.
    pub undo_dir: Option<PathBuf>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            // files open where the cursor was last, after a restart too
            old_files: true,
            commands: false,
            search: false,
            clipboard: false,
            // TODO: any more defaults we should add here?
            old_files_exclusions: [r".*/\.git/.*", r".*/COMMIT_EDITMSG"]
                .iter()
                .map(|s| Regex::new(s).unwrap().into())
                .collect(),
            old_files_trim: 100,
            commands_trim: 100,
            search_trim: 100,
            undo: false,
            undo_dir: None,
        }
    }
}

impl Config {
    /// The border style in effect: `border-style`, or else what the older
    /// `rounded-corners` option asks for.
    pub fn effective_border_style(&self) -> BorderStyle {
        self.border_style.unwrap_or(if self.rounded_corners {
            BorderStyle::Rounded
        } else {
            BorderStyle::Plain
        })
    }

    /// Whether borders have rounded corners (for the custom-drawn gradient borders).
    pub fn border_rounded(&self) -> bool {
        self.effective_border_style() == BorderStyle::Rounded
    }

    /// Whether border glyphs are hidden.
    pub fn borders_hidden(&self) -> bool {
        self.effective_border_style() == BorderStyle::None
    }
}

/// The default `title-format`: `hx main.rs:12:5`.
pub const DEFAULT_TITLE_FORMAT: &str = "hx %{buffer_name}:%{cursor_line}:%{cursor_column}";

impl Default for Config {
    fn default() -> Self {
        Self {
            welcome_screen: true,
            remote_control: true,
            set_title: false,
            title_format: DEFAULT_TITLE_FORMAT.to_string(),
            scrolloff: ScrolloffConfig::default(),
            scroll_lines: 3,
            mouse: true,
            mouse_yank_register: '*',
            shell: if cfg!(windows) {
                vec!["cmd".to_owned(), "/C".to_owned()]
            } else {
                vec!["sh".to_owned(), "-c".to_owned()]
            },
            line_number: LineNumber::Absolute,
            picker_symbol: ">".to_string(),
            picker_stacked_preview: StackedPreview::default(),
            cursorline: false,
            cursorcolumn: false,
            cursor_roundness: 0,
            gutters: GutterConfig::default(),
            middle_click_paste: true,
            auto_pairs: AutoPairConfig::default(),
            auto_completion: true,
            path_completion: true,
            word_completion: WordCompletion::default(),
            snippet_completion: true,
            auto_format: true,
            default_yank_register: '"',
            yank_highlight: 200,
            smooth_scroll: SmoothScrollConfig::default(),
            subword_motions: false,
            remember_cursor_position: true,
            auto_save: AutoSave::default(),
            auto_reload: AutoReload::default(),
            idle_timeout: Duration::from_millis(250),
            completion_timeout: Duration::from_millis(250),
            preview_completion_insert: true,
            completion_trigger_len: 2,
            auto_info: true,
            file_picker: FilePickerConfig::default(),
            bufferline: BufferLineConfig::default(),
            breadcrumb: BreadcrumbConfig::default(),
            scrollbar: ScrollbarConfig::default(),
            inline_completion: InlineCompletionConfig::default(),
            file_explorer: FileExplorerConfig::default(),
            explorer: ExplorerConfig::default(),
            statusline: StatusLineConfig::default(),
            cursor_shape: CursorShapeConfig::default(),
            true_color: false,
            undercurl: false,
            search: SearchConfig::default(),
            lsp: LspConfig::default(),
            debugger: DebuggerConfig::default(),
            image_preview: Default::default(),
            component_keys: Default::default(),
            terminal: get_terminal_provider(),
            rulers: Vec::new(),
            ruler_char: "".to_string(),
            whitespace: WhitespaceConfig::default(),
            indent_guides: IndentGuidesConfig::default(),
            color_modes: false,
            soft_wrap: SoftWrap {
                enable: Some(false),
                ..SoftWrap::default()
            },
            text_width: 80,
            completion_replace: false,
            continue_comments: true,
            workspace_lsp_roots: Vec::new(),
            default_line_ending: LineEndingConfig::default(),
            insert_final_newline: true,
            atomic_save: true,
            trim_final_newlines: false,
            trim_trailing_whitespace: false,
            smart_tab: Some(SmartTabConfig::default()),
            popup_border: PopupBorderConfig::None,
            popup_max_width_percent: 60,
            rounded_corners: false,
            border_style: None,
            commands: Default::default(),
            indent_heuristic: IndentationHeuristic::default(),
            jump_label_alphabet: ('a'..='z').collect(),
            inline_diagnostics: InlineDiagnosticsConfig::default(),
            end_of_line_diagnostics: DiagnosticFilter::Enable(Severity::Hint),
            clipboard_provider: ClipboardProvider::default(),
            inline_blame: InlineBlameConfig::default(),
            diff: crate::diff_view::DiffConfig::default(),
            editor_config: true,
            max_panel_width: 50,
            max_panel_height: 50,
            max_panel_width_percent: 0.8,
            max_panel_height_percent: 0.8,
            rainbow_brackets: false,
            kitty_keyboard_protocol: Default::default(),
            cmdline: CmdlineConfig::default(),
            quickfix: QuickfixConfig::default(),
            recent_picker: RecentPickerConfig::default(),
            gradient_borders: GradientBorderConfig::default(),
            notifications: NotificationConfig::default(),
            completion_highlight: CompletionHighlight::default(),
            fullscreen_overlay: false,
            buffer_picker: BufferPickerConfig::default(),
            fold_textobjects: Vec::new(),
            insecure: false,
            workspace_trust: WorkspaceTrustConfig::default(),
            persistence: PersistenceConfig::default(),
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            wrap_around: true,
            smart_case: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Breakpoint {
    pub id: Option<usize>,
    pub verified: bool,
    pub message: Option<String>,

    pub line: usize,
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

use futures_util::stream::{Flatten, Once};

type Diagnostics = BTreeMap<Uri, Vec<(lsp::Diagnostic, DiagnosticProvider)>>;

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: usize,
    pub message: Cow<'static, str>,
    pub severity: Severity,
    pub timestamp: Instant,
    /// The local time of `timestamp`, `HH:MM:SS`, shown in the notification's header.
    pub clock: String,
    pub timeout: Option<Duration>,
    pub dismissed: bool,
    /// Optional corner radius override for this notification
    pub corner_radius: Option<u8>,
}

/// The local time now, `HH:MM:SS`.
fn clock_now() -> String {
    let now = crate::snippets::variables::DateTime::now();
    format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second)
}

impl Notification {
    pub fn new(message: impl Into<Cow<'static, str>>, severity: Severity) -> Self {
        Self {
            id: 0, // Will be set by NotificationManager
            message: message.into(),
            severity,
            timestamp: Instant::now(),
            clock: clock_now(),
            timeout: None,
            dismissed: false,
            corner_radius: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn is_expired(&self) -> bool {
        if let Some(timeout) = self.timeout {
            self.timestamp.elapsed() >= timeout
        } else {
            false
        }
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// Whether the notification is still shown (neither dismissed nor expired).
    pub fn is_active(&self) -> bool {
        !self.dismissed && !self.is_expired()
    }
}

/// Upper bound on live (undismissed, unexpired) notifications kept when history is
/// smaller, so notifications without a timeout can't grow the list without bound.
const MAX_ACTIVE_NOTIFICATIONS: usize = 64;

#[derive(Debug, Default)]
pub struct NotificationManager {
    notifications: Vec<Notification>,
    next_id: usize,
    max_history: usize,
}

impl NotificationManager {
    pub fn new(max_history: usize) -> Self {
        Self {
            notifications: Vec::new(),
            next_id: 1,
            max_history,
        }
    }

    pub fn add(&mut self, mut notification: Notification) -> usize {
        // Deduplicate: if an active notification with the same message and
        // severity already exists, refresh its timestamp and return its id.
        if let Some(existing) = self.notifications.iter_mut().find(|n| {
            n.message == notification.message
                && n.severity == notification.severity
                && !n.dismissed
                && !n.is_expired()
        }) {
            existing.timestamp = Instant::now();
            existing.clock = clock_now();
            return existing.id;
        }

        notification.id = self.next_id;
        self.next_id += 1;

        let id = notification.id; // Store the ID before moving
        self.notifications.push(notification);
        self.trim_history();

        id
    }

    /// Keep at most `max_history` notifications. Past (dismissed or expired)
    /// notifications are evicted oldest first; live ones are only evicted when
    /// there are more than [`MAX_ACTIVE_NOTIFICATIONS`] of them, so a small
    /// history (even 0) never hides a notification that is still on screen.
    fn trim_history(&mut self) {
        let limit = self.max_history;
        let mut excess = self.notifications.len().saturating_sub(limit);
        if excess > 0 {
            self.notifications.retain(|n| {
                if excess > 0 && !n.is_active() {
                    excess -= 1;
                    false
                } else {
                    true
                }
            });
        }

        let hard_limit = limit.max(MAX_ACTIVE_NOTIFICATIONS);
        if self.notifications.len() > hard_limit {
            let excess = self.notifications.len() - hard_limit;
            self.notifications.drain(0..excess);
        }
    }

    /// Dismiss `id` only if its timeout has elapsed. Used by scheduled dismiss
    /// tasks: a duplicate notification refreshes the timestamp, which must
    /// extend its lifetime rather than be cut short by the original task.
    pub fn dismiss_if_expired(&mut self, id: usize) {
        if let Some(notification) = self
            .notifications
            .iter_mut()
            .find(|n| n.id == id && n.is_expired())
        {
            notification.dismiss();
        }
    }

    pub fn dismiss(&mut self, id: usize) {
        if let Some(notification) = self.notifications.iter_mut().find(|n| n.id == id) {
            notification.dismiss();
        }
    }

    pub fn dismiss_all(&mut self) {
        for notification in &mut self.notifications {
            notification.dismiss();
        }
    }

    pub fn get_active(&self) -> Vec<&Notification> {
        self.notifications
            .iter()
            .filter(|n| n.is_active())
            .collect()
    }

    pub fn get_all(&self) -> &[Notification] {
        &self.notifications
    }

    /// The newest notification still shown. Asked for every frame (the statusline), so it does
    /// not collect the active ones like [`Self::get_active`].
    pub fn last_active(&self) -> Option<&Notification> {
        self.notifications.iter().rev().find(|n| n.is_active())
    }

    /// Mark expired notifications as dismissed and trim the history to
    /// `max_history`. Past notifications stay in the history; only
    /// [`Self::get_active`] filters them out.
    pub fn cleanup_expired(&mut self) {
        for notification in &mut self.notifications {
            if !notification.dismissed && notification.is_expired() {
                notification.dismiss();
            }
        }
        self.trim_history();
    }

    pub fn clear_history(&mut self) {
        self.notifications.clear();
    }
}

#[cfg(test)]
mod notification_manager_tests {
    use super::*;

    fn expired(message: &'static str) -> Notification {
        let mut n = Notification::new(message, Severity::Info).with_timeout(Duration::ZERO);
        n.timestamp = Instant::now();
        n
    }

    #[test]
    fn zero_history_keeps_live_notifications() {
        let mut manager = NotificationManager::new(0);
        let id = manager.add(Notification::new("hello", Severity::Info));
        assert_eq!(manager.get_active().len(), 1);
        assert_eq!(manager.get_active()[0].id, id);

        manager.dismiss(id);
        manager.cleanup_expired();
        assert!(manager.get_all().is_empty());
    }

    #[test]
    fn history_keeps_past_notifications() {
        let mut manager = NotificationManager::new(3);
        for msg in ["a", "b", "c", "d"] {
            let id = manager.add(Notification::new(msg, Severity::Info));
            manager.dismiss(id);
        }
        manager.add(expired("e"));
        manager.cleanup_expired();

        assert!(manager.get_active().is_empty());
        let history: Vec<_> = manager.get_all().iter().map(|n| &*n.message).collect();
        assert_eq!(history, ["c", "d", "e"]);
        assert!(manager.get_all().iter().all(|n| n.dismissed));
    }

    #[test]
    fn duplicate_is_merged_and_extends_lifetime() {
        let mut manager = NotificationManager::new(10);
        let timeout = Duration::from_secs(60);
        let first = manager.add(Notification::new("dup", Severity::Info).with_timeout(timeout));
        manager.add(Notification::new("other", Severity::Info));
        let second = manager.add(Notification::new("dup", Severity::Info).with_timeout(timeout));
        assert_eq!(first, second);
        assert_eq!(manager.get_all().len(), 2);

        // A scheduled dismiss for a refreshed (unexpired) notification is a no-op.
        manager.dismiss_if_expired(first);
        assert!(manager.get_active().iter().any(|n| n.id == first));
    }
}

/// A quit requested together with a save (`:wq`, `:x`, `:wqa`, ...). The editor keeps
/// running while the saves finish and the application quits afterwards. A failed
/// save cancels it, so unsaved edits are never lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingQuit {
    /// Close one view.
    View { view: ViewId, force: bool },
    /// Close every view.
    All { force: bool },
}

pub struct Editor {
    /// Current editing mode.
    pub mode: Mode,
    pub tree: Tree,
    /// The buffers of each tab page (by its root container): the ones opened or shown in
    /// it. What the bufferline lists with `[editor.bufferline] scope = "tab"`.
    tab_buffers: HashMap<ViewId, HashSet<DocumentId>>,
    pub next_document_id: DocumentId,
    pub documents: BTreeMap<DocumentId, Document>,

    // We Flatten<> to resolve the inner DocumentSavedEventFuture. For that we need a stream of streams, hence the Once<>.
    // https://stackoverflow.com/a/66875668
    pub saves: HashMap<DocumentId, UnboundedSender<Once<DocumentSavedEventFuture>>>,
    pub save_queue: SelectAll<Flatten<UnboundedReceiverStream<Once<DocumentSavedEventFuture>>>>,
    pub write_count: usize,
    /// A quit that waits for the saves in flight to finish, see [`PendingQuit`].
    pub pending_quit: Option<PendingQuit>,
    /// The arguments of the custom command being run, for `%arg{..}`.
    pub command_args: Option<Vec<String>>,
    /// User-defined snippets, loaded from `snippets.toml` and the `snippets/` directories.
    pub snippets: Arc<Snippets>,
    /// Locations collected to revisit, see [`Quicklist`].
    pub quicklist: Quicklist,
    /// The quickfix panel (see `[editor.quickfix]`): shown, and with the focus.
    pub quickfix_panel: crate::quicklist::QuickfixPanel,
    /// The recently used files (see `[editor.recent-picker]`).
    pub recent_files: crate::recent_files::RecentFiles,

    pub count: Option<std::num::NonZeroUsize>,
    pub selected_register: Option<char>,
    pub registers: Registers,
    pub macro_recording: Option<(char, Vec<KeyEvent>)>,
    pub macro_replaying: Vec<char>,
    pub language_servers: helix_lsp::Registry,
    pub diagnostics: Diagnostics,
    pub diff_providers: DiffProviderRegistry,

    pub debug_adapters: dap::registry::Registry,
    /// Bumped whenever the debugger's inline values are cleared (and so by every refresh):
    /// a fetch only applies its values if no newer refresh or clear happened since it
    /// started. Per editor, so separate editors (tests) don't drop each other's values.
    pub dap_inline_values_generation: u64,
    pub breakpoints: HashMap<PathBuf, Vec<Breakpoint>>,

    pub syn_loader: Arc<ArcSwap<syntax::Loader>>,
    pub theme_loader: Arc<theme::Loader>,
    /// last_theme is used for theme previews. We store the current theme here,
    /// and if previewing is cancelled, we can return to it.
    pub last_theme: Option<Theme>,
    /// The currently applied editor theme. While previewing a theme, the previewed theme
    /// is set here.
    pub theme: Theme,
    /// The size of a cell of the terminal in pixels, when it reports one. The application
    /// updates it before every frame.
    pub cell_size: Option<(u16, u16)>,
    /// Whether the terminal answered that it shows images with the kitty graphics protocol
    /// (asked at startup where the environment doesn't tell). The application sets it.
    pub kitty_graphics: bool,

    /// The primary Selection prior to starting a goto_line_number preview. This is
    /// restored when the preview is aborted, or added to the jumplist when it is
    /// confirmed.
    pub last_selection: Option<Selection>,

    pub status_msg: Option<(Cow<'static, str>, Severity)>,
    pub notifications: NotificationManager,
    pub autoinfo: Option<Info>,

    pub config: Arc<dyn DynAccess<Config>>,
    pub auto_pairs: Option<AutoPairs>,

    pub idle_timer: Pin<Box<Sleep>>,
    redraw_timer: Pin<Box<Sleep>>,
    last_motion: Option<Motion>,
    pub last_completion: Option<CompleteAction>,
    pub last_cwd: Option<PathBuf>,
    pub dir_stack: VecDeque<PathBuf>,

    /// Pending key sequence (the keys typed so far of a multi-key
    /// mapping, e.g. `[<space>]` after pressing space in normal mode).
    /// Published by the editor view's keymap handling so components can
    /// defer to in-progress sequences.
    pub keymap_pending: Vec<KeyEvent>,

    /// State of the file tree explorer panel.
    pub explorer_panel: crate::explorer::ExplorerState,
    /// What plugins see of the explorer and ask of it.
    pub plugin_explorer: crate::explorer::PluginExplorer,
    /// The keys plugins added to the diff viewer, the explorer and the quickfix panel.
    pub plugin_actions: crate::plugin_ui::PluginActions,
    /// The completion sources of plugins (`helix.completion.register_source`).
    pub plugin_completion_sources: Vec<crate::plugin_ui::PluginCompletionSource>,
    /// Asks the plugins' completion sources (set by the application when plugins load).
    pub plugin_completion_hook: Option<crate::plugin_ui::PluginCompletionHook>,
    /// The text just yanked, highlighted for a moment (see [`Config::yank_highlight`]).
    pub yank_flash: Option<YankFlash>,
    /// The smooth scroll under way (`[editor.smooth-scroll]`).
    pub scroll_animation: Option<ScrollAnimation>,
    /// The selections saved under a register name (`A-Z`), to be restored (`A-z`).
    pub saved_selections: HashMap<char, SavedSelections>,
    /// The floating windows of plugins (`helix.ui.float`), the last one on top.
    pub plugin_floats: Vec<crate::plugin_ui::PluginFloat>,
    /// The marks plugins show on the quickfix panel's rows.
    pub plugin_quickfix_marks: crate::plugin_ui::QuickfixMarks,
    /// The side-by-side diffs shown (see [`crate::diff_view`]).
    pub diff_sessions: Vec<crate::diff_view::DiffSession>,
    /// The changed files being reviewed in a diff, one after the other (`]f`, `[f`).
    pub diff_review: Option<crate::diff_view::ReviewQueue>,
    /// The commit message being written: saving it commits.
    pub commit_message: Option<crate::diff_view::CommitMessage>,
    /// The merge conflict resolver shown (see [`crate::merge_view`]).
    pub merge: Option<crate::merge_view::MergeSession>,
    /// Explorer internal clipboard for cut/copy-paste entries.
    pub explorer_clipboard: Option<ExplorerClipboard>,

    pub exit_code: i32,

    pub config_events: (UnboundedSender<ConfigEvent>, UnboundedReceiver<ConfigEvent>),
    pub needs_redraw: bool,
    /// Cached position of the cursor calculated during rendering.
    /// The content of `cursor_cache` is returned by `Editor::cursor` if
    /// set to `Some(_)`. The value will be cleared after it's used.
    /// If `cursor_cache` is `None` then the `Editor::cursor` function will
    /// calculate the cursor position.
    ///
    /// `Some(None)` represents a cursor position outside of the visible area.
    /// This will just cause `Editor::cursor` to return `None`.
    ///
    /// This cache is only a performance optimization to
    /// avoid calculating the cursor position multiple
    /// times during rendering and should not be set by other functions.
    pub handlers: Handlers,

    pub mouse_down_range: Option<Range>,
    pub cursor_cache: CursorCache,
    /// Mouse pointer shape the terminal should show, set while handling mouse
    /// events (resize handles) and applied by the application.
    pub pointer_shape: PointerShape,
    pub workspace_trust: WorkspaceTrust,
    /// Where the cursor was last in each file closed (and the scroll of its window): the file
    /// opens there again. Read from the file history at startup with `[editor.persistence]
    /// old-files`, and written to it as files close.
    pub old_file_locs: HashMap<PathBuf, (ViewPosition, Selection)>,
    /// Run by [`Editor::save`] before a document is written (the application runs the plugins'
    /// `buffer_pre_save` handlers with it). It may edit the document: what it leaves is
    /// written. When it returns `false` the document is not written.
    pub pre_save_hook: Option<PreSaveHook>,
    /// Asked before a command runs (the application runs the plugins' `command_pre`
    /// handlers with it): `false` keeps it from running.
    pub command_hook: Option<CommandHook>,
    /// Key bindings set by plugins (`helix.keymap`), over the configured keys.
    pub plugin_keymaps: PluginKeymaps,
    /// Status line segments of plugins (`helix.statusline`).
    pub plugin_statusline: Vec<StatusSegment>,
    /// Segments of plugins in the bar at the top of windows (`helix.winbar`).
    pub plugin_window_bar: Vec<BarSegment>,
    /// Theme styles plugins set (`helix.ui.set_style`), kept over theme changes.
    pub plugin_theme_overrides: Vec<(String, Style)>,
    /// The newest settings sent to the application (`:set`) and not applied yet: the next
    /// change builds on them, or two changes in a row would lose the first.
    pub pending_config: Option<Box<Config>>,
    /// Whether the terminal can draw the secondary cursors itself (kitty's multiple cursors
    /// protocol), detected when the terminal is set up.
    pub kitty_multi_cursor_support: bool,
}

pub type Motion = Box<dyn Fn(&mut Editor)>;

/// See [`Editor::pre_save_hook`].
pub type PreSaveHook = Box<dyn FnMut(&mut Editor, DocumentId) -> bool>;

/// See [`Editor::command_hook`]: the command's name and its arguments (empty for a command
/// that takes none).
pub type CommandHook = Box<dyn FnMut(&mut Editor, &str, &str) -> bool>;

#[derive(Debug)]
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),
    ConfigEvent(ConfigEvent),
    LanguageServerMessage((LanguageServerId, Call)),
    DebuggerEvent((DebugAdapterId, dap::Payload)),
    IdleTimer,
    Redraw,
}

#[derive(Debug, Clone)]
pub enum ConfigEvent {
    Refresh,
    Update(Box<Config>),
    ThemeChanged,
}

enum ThemeAction {
    Set,
    Preview,
}

#[derive(Debug, Clone)]
pub enum CompleteAction {
    Triggered,
    /// A savepoint of the currently selected completion. The savepoint
    /// MUST be restored before sending any event to the LSP
    Selected {
        savepoint: Arc<SavePoint>,
    },
    Applied {
        trigger_offset: usize,
        changes: Vec<Change>,
        placeholder: bool,
    },
}

#[derive(Debug, Copy, Clone)]
pub enum Action {
    Load,
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

impl Action {
    /// Whether to align the view to the cursor after executing this action
    pub fn align_view(&self, view: &View, new_doc: DocumentId) -> bool {
        !matches!((self, view.doc == new_doc), (Action::Load, false))
    }
}

/// Where a plugin's status line segment goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentPosition {
    /// After the configured left elements.
    Left,
    /// After the configured center elements.
    Center,
    /// Before the configured right elements.
    Right,
}

/// A text a plugin shows in the status line (`helix.statusline.set`).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSegment {
    pub plugin: String,
    /// The plugin's name for it (set again to change it).
    pub id: String,
    pub text: String,
    /// A theme key, under `style`.
    pub scope: Option<String>,
    pub style: Style,
    pub position: SegmentPosition,
    /// Only in the status lines of windows showing this buffer.
    pub document: Option<DocumentId>,
    /// Lower first.
    pub priority: i64,
}

/// Text just yanked, highlighted in its window until `until` (and while the buffer is
/// unchanged).
#[derive(Debug, Clone, PartialEq)]
pub struct YankFlash {
    pub view: ViewId,
    pub document: DocumentId,
    /// The buffer's version when it was yanked: an edit ends the highlight.
    pub version: i32,
    /// Chars, `end` exclusive.
    pub ranges: Vec<std::ops::Range<usize>>,
    pub until: Instant,
}

/// A plugin's text in the bar at the top of windows (`helix.winbar`).
#[derive(Debug, Clone, PartialEq)]
pub struct BarSegment {
    pub plugin: String,
    /// The plugin's name for it (set again to change it).
    pub id: String,
    /// With `{name}`, `{path}`, `{modified}` and `{language}` filled in for each window.
    pub text: String,
    /// A theme key, under `style`.
    pub scope: Option<String>,
    pub style: Style,
    pub position: SegmentPosition,
    /// Only in windows showing this buffer.
    pub document: Option<DocumentId>,
    /// Only in this window.
    pub window: Option<ViewId>,
    /// Lower first.
    pub priority: i64,
}

impl BarSegment {
    /// Whether it shows in `view`.
    pub fn shows_in(&self, view: &View) -> bool {
        self.window.is_none_or(|window| window == view.id)
            && self.document.is_none_or(|document| document == view.doc)
    }
}

/// Key bindings set by plugins. The application merges them over the configured keys each
/// time `version` changes (and again after `:config-reload`).
#[derive(Debug, Default, Clone)]
pub struct PluginKeymaps {
    /// Changes with every change of `bindings`.
    pub version: u64,
    pub bindings: Vec<PluginBinding>,
}

/// A key binding of a plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginBinding {
    /// The plugin that set it.
    pub plugin: String,
    pub mode: Mode,
    pub keys: Vec<KeyEvent>,
    /// As in `[keys]`: `"move_line_down"`, `":write"`, `"@ihi<esc>"`; `"no_op"` unbinds.
    pub command: String,
    /// What the key help shows for it (`None`: the command's own).
    pub description: Option<String>,
    /// The languages of the buffers it is for (`plaintasks`, ...): the key keeps its meaning
    /// in the others. `None`: all.
    pub languages: Option<Vec<String>>,
    /// The name the key help shows for the menu the key is in (`<space>t` of `<space>tn`).
    pub menu: Option<String>,
    /// The buffer it is for (a plugin's own buffer in a docked window): the key keeps its
    /// meaning in the others. `None`: all.
    pub document: Option<DocumentId>,
}

impl PluginBinding {
    /// Whether it applies in the buffer `document` of `language`.
    pub fn applies_to(&self, language: Option<&str>, document: Option<DocumentId>) -> bool {
        let language_matches = match &self.languages {
            None => true,
            Some(languages) => {
                language.is_some_and(|language| languages.iter().any(|l| l == language))
            }
        };
        language_matches && self.document.is_none_or(|wanted| document == Some(wanted))
    }
}

impl PluginKeymaps {
    /// Binds (replacing the binding of the same keys in the same mode for the same languages
    /// and buffer, whoever set it).
    pub fn set(&mut self, binding: PluginBinding) {
        self.bindings.retain(|b| {
            !(b.mode == binding.mode
                && b.keys == binding.keys
                && b.languages == binding.languages
                && b.document == binding.document)
        });
        self.bindings.push(binding);
        self.version += 1;
    }

    /// Whether some bindings are for some languages only: the keymap then depends on the
    /// focused buffer's language.
    pub fn has_language_bindings(&self) -> bool {
        self.bindings.iter().any(|b| b.languages.is_some())
    }

    /// Whether some bindings are for one buffer only: the keymap then depends on the focused
    /// buffer.
    pub fn has_buffer_bindings(&self) -> bool {
        self.bindings.iter().any(|b| b.document.is_some())
    }

    /// Removes the binding `plugin` set for `keys` in `mode`; whether there was one.
    pub fn remove(&mut self, plugin: &str, mode: Mode, keys: &[KeyEvent]) -> bool {
        let before = self.bindings.len();
        self.bindings
            .retain(|b| !(b.plugin == plugin && b.mode == mode && b.keys == keys));
        let removed = self.bindings.len() != before;
        if removed {
            self.version += 1;
        }
        removed
    }

    /// Removes every binding (the plugins are reloaded).
    pub fn clear(&mut self) {
        if !self.bindings.is_empty() {
            self.bindings.clear();
            self.version += 1;
        }
    }
}

/// Error thrown on failed document closed
pub enum CloseError {
    /// Document doesn't exist
    DoesNotExist,
    /// Buffer is modified
    BufferModified(String),
    /// Document failed to save
    SaveError(anyhow::Error),
}

impl Editor {
    pub fn new(
        mut area: Rect,
        theme_loader: Arc<theme::Loader>,
        syn_loader: Arc<ArcSwap<syntax::Loader>>,
        config: Arc<dyn DynAccess<Config>>,
        handlers: Handlers,
        workspace_trust: WorkspaceTrust,
        old_file_locs: HashMap<PathBuf, (ViewPosition, Selection)>,
    ) -> Self {
        let language_servers = helix_lsp::Registry::new(syn_loader.clone());
        let conf = config.load();
        let auto_pairs = (&conf.auto_pairs).into();

        // HAXX: offset the render area height by 1 to account for prompt/commandline
        area.height -= 1;

        Self {
            mode: Mode::Normal,
            tree: Tree::new(area),
            tab_buffers: HashMap::new(),
            next_document_id: DocumentId::default(),
            documents: BTreeMap::new(),
            saves: HashMap::new(),
            save_queue: SelectAll::new(),
            write_count: 0,
            pending_quit: None,
            command_args: None,
            snippets: Arc::new(Snippets::default()),
            quicklist: Quicklist::default(),
            quickfix_panel: crate::quicklist::QuickfixPanel::default(),
            recent_files: crate::recent_files::RecentFiles::default(),
            count: None,
            selected_register: None,
            macro_recording: None,
            macro_replaying: Vec::new(),
            theme: theme_loader.default(),
            cell_size: None,
            kitty_graphics: false,
            language_servers,
            diagnostics: Diagnostics::new(),
            diff_providers: DiffProviderRegistry::default(),
            debug_adapters: dap::registry::Registry::new(),
            dap_inline_values_generation: 0,
            breakpoints: HashMap::new(),
            syn_loader,
            theme_loader,
            last_theme: None,
            last_selection: None,
            registers: Registers::new(Box::new(arc_swap::access::Map::new(
                Arc::clone(&config),
                |config: &Config| &config.clipboard_provider,
            ))),
            status_msg: None,
            notifications: NotificationManager::new(conf.notifications.max_history),
            autoinfo: None,
            idle_timer: Box::pin(sleep(conf.idle_timeout)),
            redraw_timer: Box::pin(sleep(Duration::MAX)),
            last_motion: None,
            last_completion: None,
            last_cwd: None,
            keymap_pending: Vec::new(),
            explorer_panel: crate::explorer::ExplorerState::default(),
            plugin_explorer: crate::explorer::PluginExplorer::default(),
            plugin_actions: Default::default(),
            plugin_completion_sources: Vec::new(),
            plugin_completion_hook: None,
            plugin_floats: Vec::new(),
            yank_flash: None,
            scroll_animation: None,
            saved_selections: HashMap::new(),
            plugin_quickfix_marks: Default::default(),
            diff_sessions: Vec::new(),
            diff_review: None,
            commit_message: None,
            merge: None,
            explorer_clipboard: None,
            config,
            auto_pairs,
            exit_code: 0,
            config_events: unbounded_channel(),
            needs_redraw: false,
            handlers,
            mouse_down_range: None,
            cursor_cache: CursorCache::default(),
            pointer_shape: PointerShape::default(),
            dir_stack: VecDeque::with_capacity(DIR_STACK_CAP),
            workspace_trust,
            old_file_locs,
            pre_save_hook: None,
            command_hook: None,
            plugin_keymaps: PluginKeymaps::default(),
            pending_config: None,
            plugin_statusline: Vec::new(),
            plugin_window_bar: Vec::new(),
            plugin_theme_overrides: Vec::new(),
            kitty_multi_cursor_support: false,
        }
    }

    pub fn popup_border(&self) -> bool {
        self.config().popup_border == PopupBorderConfig::All
            || self.config().popup_border == PopupBorderConfig::Popup
    }

    pub fn menu_border(&self) -> bool {
        self.config().popup_border == PopupBorderConfig::All
            || self.config().popup_border == PopupBorderConfig::Menu
    }

    pub fn apply_motion<F: Fn(&mut Self) + 'static>(&mut self, motion: F) {
        motion(self);
        self.last_motion = Some(Box::new(motion));
    }

    pub fn repeat_last_motion(&mut self, count: usize) {
        if let Some(motion) = self.last_motion.take() {
            for _ in 0..count {
                motion(self);
            }
            self.last_motion = Some(motion);
        }
    }
    /// Current editing mode for the [`Editor`].
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn config(&self) -> DynGuard<Config> {
        self.config.load()
    }

    /// The active explorer side, honoring the runtime override.
    pub fn explorer_side(&self) -> crate::explorer::ExplorerSide {
        let config = self.config.load();
        self.explorer_panel.side(&config.explorer)
    }

    /// The active explorer width, honoring the runtime override.
    pub fn explorer_width(&self) -> u16 {
        let config = self.config.load();
        self.explorer_panel.width(&config.explorer)
    }

    /// Whether hidden files are shown in the explorer, honoring the runtime
    /// override.
    pub fn explorer_show_hidden(&self) -> bool {
        let config = self.config.load();
        self.explorer_panel.show_hidden(&config.explorer)
    }

    /// Call if the config has changed to let the editor update all
    /// relevant members.
    /// Sets the style of a theme scope for plugins (kept over theme changes).
    pub fn set_plugin_style(&mut self, scope: &str, style: Style) {
        self.plugin_theme_overrides.retain(|(s, _)| s != scope);
        self.plugin_theme_overrides.push((scope.to_string(), style));
        if self.theme.set_style(scope, style) {
            (*self.syn_loader)
                .load()
                .set_scopes(self.theme.scopes().to_vec());
        }
    }

    /// Removes a plugin's style of `scope`: the theme's own comes back (the theme is loaded
    /// again with the remaining overrides).
    pub fn reset_plugin_style(&mut self, scope: &str) -> anyhow::Result<()> {
        let before = self.plugin_theme_overrides.len();
        self.plugin_theme_overrides.retain(|(s, _)| s != scope);
        if before == self.plugin_theme_overrides.len() {
            return Ok(());
        }
        let theme = self.theme_loader.load(self.theme.name())?;
        self.set_theme(theme)
    }

    /// The settings a change of settings (`:set`) builds on: the newest ones sent and not
    /// applied yet, or the current ones.
    pub fn config_to_change(&self) -> Config {
        match &self.pending_config {
            Some(pending) => (**pending).clone(),
            None => (*self.config()).clone(),
        }
    }

    /// Sends changed settings to the application (which applies them with the next event).
    pub fn send_config(&mut self, config: Config) -> anyhow::Result<()> {
        self.pending_config = Some(Box::new(config.clone()));
        self.config_events
            .0
            .send(ConfigEvent::Update(Box::new(config)))?;
        Ok(())
    }

    pub fn refresh_config(&mut self, old_config: &Config) {
        let config = self.config();
        self.auto_pairs = (&config.auto_pairs).into();
        self.reset_idle_timer();
        self._refresh();
        helix_event::dispatch(crate::events::ConfigDidChange {
            editor: self,
            old: old_config,
            new: &config,
        })
    }

    pub fn clear_idle_timer(&mut self) {
        // equivalent to internal Instant::far_future() (30 years)
        self.idle_timer
            .as_mut()
            .reset(Instant::now() + Duration::from_secs(86400 * 365 * 30));
    }

    pub fn reset_idle_timer(&mut self) {
        let config = self.config();
        self.idle_timer
            .as_mut()
            .reset(Instant::now() + config.idle_timeout);

        // Cleanup expired notifications periodically
        self.cleanup_notifications();
    }

    pub fn clear_status(&mut self) {
        self.status_msg = None;
        self.notifications.dismiss_all();
    }

    #[inline]
    pub fn set_status<T: Into<Cow<'static, str>>>(&mut self, status: T) {
        let status = status.into();
        log::debug!("editor status: {}", status);

        let config = self.config();
        if config.notifications.enable && config.notifications.style == NotificationStyle::Popup {
            // Only create notification, don't set status_msg for popup style
            self.notify_info(status);
        } else {
            // Traditional behavior: set status_msg and optionally create notification
            self.status_msg = Some((status.clone(), Severity::Info));
            if config.notifications.enable {
                self.notify_info(status);
            }
        }
    }

    #[inline]
    /// Reload the user-defined snippets from the config directory and, when the workspace is
    /// trusted, from its `.helix/` directory.
    ///
    /// On failure the previous snippets stay active and the error is returned.
    pub fn reload_snippets(&mut self) -> anyhow::Result<usize> {
        let (workspace, _) = helix_loader::find_workspace();
        let trusted = self
            .workspace_trust
            .query(&workspace, TrustQuery::LocalConfig)
            .is_trusted();
        let snippets = Snippets::load(
            &helix_loader::config_dir(),
            trusted.then_some(workspace.as_path()),
        )?;
        let count = snippets.len();
        self.snippets = Arc::new(snippets);
        Ok(count)
    }

    pub fn set_error<T: Into<Cow<'static, str>>>(&mut self, error: T) {
        let error = error.into();
        log::debug!("editor error: {}", error);

        let config = self.config();
        if config.notifications.enable && config.notifications.style == NotificationStyle::Popup {
            // Only create notification, don't set status_msg for popup style
            self.notify_error(error);
        } else {
            // Traditional behavior: set status_msg and optionally create notification
            self.status_msg = Some((error.clone(), Severity::Error));
            if config.notifications.enable {
                self.notify_error(error);
            }
        }
    }

    #[inline]
    pub fn set_result<T: Into<Cow<'static, str>>>(&mut self, result: Result<T, T>) {
        match result {
            Ok(ok) => self.set_status(ok),
            Err(err) => self.set_error(err),
        }
    }

    #[inline]
    pub fn set_warning<T: Into<Cow<'static, str>>>(&mut self, warning: T) {
        let warning = warning.into();
        log::warn!("editor warning: {}", warning);

        let config = self.config();
        if config.notifications.enable && config.notifications.style == NotificationStyle::Popup {
            // Only create notification, don't set status_msg for popup style
            self.notify_warning(warning);
        } else {
            // Traditional behavior: set status_msg and optionally create notification
            self.status_msg = Some((warning.clone(), Severity::Warning));
            if config.notifications.enable {
                self.notify_warning(warning);
            }
        }
    }

    #[inline]
    pub fn get_status(&self) -> Option<(&Cow<'static, str>, &Severity)> {
        self.status_msg
            .as_ref()
            .map(|(status, sev)| (status, sev))
            .or_else(|| {
                self.notifications
                    .last_active()
                    .map(|n| (&n.message, &n.severity))
            })
    }

    /// Returns true if the current status is an error
    #[inline]
    pub fn is_err(&self) -> bool {
        self.get_status()
            .map(|(_, sev)| *sev == Severity::Error)
            .unwrap_or(false)
    }

    // Notification methods
    pub fn notify<T: Into<Cow<'static, str>>>(&mut self, message: T) -> usize {
        self.notify_with_severity(message, Severity::Info)
    }

    pub fn notify_info<T: Into<Cow<'static, str>>>(&mut self, message: T) -> usize {
        self.notify_with_severity(message, Severity::Info)
    }

    pub fn notify_warning<T: Into<Cow<'static, str>>>(&mut self, message: T) -> usize {
        self.notify_with_severity(message, Severity::Warning)
    }

    pub fn notify_error<T: Into<Cow<'static, str>>>(&mut self, message: T) -> usize {
        self.notify_with_severity(message, Severity::Error)
    }

    pub fn notify_with_severity<T: Into<Cow<'static, str>>>(
        &mut self,
        message: T,
        severity: Severity,
    ) -> usize {
        let config = self.config();
        if !config.notifications.enable {
            // Fall back to traditional status messages if notifications are disabled
            match severity {
                Severity::Error => self.set_error(message),
                Severity::Warning => self.set_warning(message),
                _ => self.set_status(message),
            }
            return 0;
        }

        let message = message.into();
        let mut notification = Notification::new(message.clone(), severity);

        // Set default timeout if configured
        if config.notifications.default_timeout > Duration::ZERO {
            let timeout = config.notifications.default_timeout;
            notification = notification.with_timeout(timeout);
            // Schedule a redraw at the timeout moment so the UI can expire/fade it immediately
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                helix_event::request_redraw();
            });
        }

        // `add` may merge into an existing notification or (with a small
        // `max-history`) evict entries, so use the message we were given rather
        // than whatever happens to be last in the list.
        let id = self.notifications.add(notification);

        // Also set status message for compatibility if using statusline style
        if config.notifications.style == NotificationStyle::Statusline {
            let severity = match severity {
                Severity::Error | Severity::Warning => severity,
                _ => Severity::Info,
            };
            self.status_msg = Some((message, severity));
        }

        id
    }

    pub fn dismiss_notification(&mut self, id: usize) {
        self.notifications.dismiss(id);
    }

    /// Dismiss notification `id` only if its timeout has elapsed.
    pub fn dismiss_notification_if_expired(&mut self, id: usize) {
        self.notifications.dismiss_if_expired(id);
    }

    pub fn dismiss_all_notifications(&mut self) {
        self.notifications.dismiss_all();
    }

    pub fn get_active_notifications(&self) -> Vec<&Notification> {
        self.notifications.get_active()
    }

    pub fn get_notification_history(&self) -> &[Notification] {
        self.notifications.get_all()
    }

    pub fn clear_notification_history(&mut self) {
        self.notifications.clear_history();
    }

    pub fn cleanup_notifications(&mut self) {
        self.notifications.cleanup_expired();
    }

    pub fn unset_theme_preview(&mut self) -> anyhow::Result<()> {
        if let Some(last_theme) = self.last_theme.take() {
            self.set_theme(last_theme)?;
        }
        // None likely occurs when the user types ":theme" and then exits before previewing
        Ok(())
    }

    pub fn set_theme_preview(&mut self, theme: Theme) -> anyhow::Result<()> {
        self.set_theme_impl(theme, ThemeAction::Preview)
    }

    pub fn set_theme(&mut self, theme: Theme) -> anyhow::Result<()> {
        self.set_theme_impl(theme, ThemeAction::Set)
    }

    fn set_theme_impl(&mut self, mut theme: Theme, preview: ThemeAction) -> anyhow::Result<()> {
        // `ui.selection` is the only scope required to be able to render a theme.
        if theme.find_highlight_exact("ui.selection").is_none() {
            bail!("Invalid theme: `ui.selection` required");
        }
        // Plugins' styles stay over any theme.
        for (scope, style) in &self.plugin_theme_overrides {
            theme.set_style(scope, *style);
        }

        let scopes = theme.scopes();
        (*self.syn_loader).load().set_scopes(scopes.to_vec());

        match preview {
            ThemeAction::Preview => {
                let last_theme = std::mem::replace(&mut self.theme, theme);
                // only insert on first preview: this will be the last theme the user has saved
                self.last_theme.get_or_insert(last_theme);
            }
            ThemeAction::Set => {
                self.last_theme = None;
                self.theme = theme;
            }
        }

        self._refresh();
        self.config_events.0.send(ConfigEvent::ThemeChanged)?;

        Ok(())
    }

    #[inline]
    pub fn language_server_by_id(
        &self,
        language_server_id: LanguageServerId,
    ) -> Option<&helix_lsp::Client> {
        self.language_servers
            .get_by_id(language_server_id)
            .map(|client| &**client)
    }

    /// Refreshes the language server for a given document
    pub fn refresh_language_servers(&mut self, doc_id: DocumentId) {
        if self.launch_language_servers(doc_id) {
            self.language_servers_did_change(doc_id);
        }
    }

    /// Tells the handlers that `doc_id`'s language servers changed and what the previous ones
    /// computed was dropped, so they ask the current ones (already running servers send no
    /// "initialized" event that would make them ask).
    fn language_servers_did_change(&mut self, doc_id: DocumentId) {
        helix_event::dispatch(crate::events::DocumentLanguageServersDidChange {
            editor: self,
            doc: doc_id,
        });
    }

    /// Applies the edits the language servers ask for before a file operation
    /// (`workspace/willRenameFiles` and the like).
    ///
    /// The protocol has the edits applied before the files are touched, so this waits for the
    /// answers on the main thread. The wait is bounded: the requests are sent to all servers
    /// at once and each times out after a few seconds (see `Client::will_rename`), so the
    /// editor stalls for that long at most, however many servers there are, instead of one
    /// timeout after the other.
    fn apply_file_operation_edits<F>(
        &mut self,
        request: &str,
        requests: Vec<(helix_lsp::OffsetEncoding, F)>,
    ) where
        F: std::future::Future<Output = helix_lsp::Result<Option<lsp::WorkspaceEdit>>>,
    {
        if requests.is_empty() {
            return;
        }
        let responses =
            helix_lsp::block_on(futures_util::future::join_all(requests.into_iter().map(
                |(offset_encoding, request)| async move { (offset_encoding, request.await) },
            )));
        for (offset_encoding, response) in responses {
            let edit = match response {
                Ok(edit) => edit.unwrap_or_default(),
                Err(err) => {
                    log::error!("invalid {request} response: {err:?}");
                    continue;
                }
            };
            if let Err(err) = self.apply_workspace_edit(offset_encoding, &edit) {
                log::error!("failed to apply workspace edit: {err:?}")
            }
        }
    }

    /// moves/renames a path, invoking any event handlers (currently only lsp)
    /// and calling `set_doc_path` if the file is open in the editor
    pub fn move_path(&mut self, old_path: &Path, new_path: &Path) -> io::Result<()> {
        let new_path = canonicalize(new_path);
        // sanity check
        if old_path == new_path {
            return Ok(());
        }
        let is_dir = old_path.is_dir();
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        let requests = language_servers
            .iter()
            .filter_map(|ls| {
                Some((
                    ls.offset_encoding(),
                    ls.will_rename(old_path, &new_path, is_dir)?,
                ))
            })
            .collect();
        self.apply_file_operation_edits("willRename", requests);

        if old_path.exists() {
            fs::rename(old_path, &new_path)?;
        }

        if let Some(doc) = self.document_by_path(old_path) {
            self.set_doc_path(doc.id(), &new_path);
        }
        let is_dir = new_path.is_dir();
        for ls in self.language_servers.iter_clients() {
            // A new language server might have been started in `set_doc_path` and won't
            // be initialized yet. Skip the `did_rename` notification for this server.
            if !ls.is_initialized() {
                continue;
            }
            ls.did_rename(old_path, &new_path, is_dir);
        }
        self.language_servers
            .file_event_handler
            .file_changed(old_path.to_owned());
        self.language_servers
            .file_event_handler
            .file_changed(new_path);
        Ok(())
    }

    pub fn create_path(&mut self, path: &Path, is_dir: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        let requests = language_servers
            .iter()
            .filter_map(|ls| Some((ls.offset_encoding(), ls.will_create(&path, is_dir)?)))
            .collect();
        self.apply_file_operation_edits("willCreate", requests);

        if let Some(dir) = path.parent() {
            if !dir.is_dir() {
                fs::create_dir_all(dir)?;
            }
        }
        if is_dir {
            fs::create_dir(&path)?;
        } else {
            fs::write(&path, [])?;
        }

        for ls in self.language_servers.iter_clients() {
            if !ls.is_initialized() {
                continue;
            }
            ls.did_create(&path, is_dir);
        }
        self.language_servers.file_event_handler.file_changed(path);
        Ok(())
    }

    pub fn delete_path(&mut self, path: &Path, recursive: bool) -> io::Result<()> {
        let path = canonicalize(path);
        let is_dir = path.is_dir();
        let language_servers: Vec<_> = self
            .language_servers
            .iter_clients()
            .filter(|client| client.is_initialized())
            .cloned()
            .collect();
        let requests = language_servers
            .iter()
            .filter_map(|ls| Some((ls.offset_encoding(), ls.will_delete(&path, is_dir)?)))
            .collect();
        self.apply_file_operation_edits("willDelete", requests);

        if is_dir {
            if recursive {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_dir(&path)?;
            }
        } else {
            fs::remove_file(&path)?;
        }

        for ls in self.language_servers.iter_clients() {
            if !ls.is_initialized() {
                continue;
            }
            ls.did_delete(&path, is_dir);
        }
        self.language_servers.file_event_handler.file_changed(path);
        Ok(())
    }

    pub fn set_doc_path(&mut self, doc_id: DocumentId, path: &Path) {
        let doc = doc_mut!(self, &doc_id);
        let old_path = doc.path();

        if let Some(old_path) = old_path {
            // sanity check, should not occur but some callers (like an LSP) may
            // create bogus calls
            if old_path == path {
                return;
            }
            // if we are open in LSPs send did_close notification
            for language_server in doc.language_servers() {
                language_server.text_document_did_close(doc.identifier());
            }
        }
        // we need to clear the list of language servers here so that
        // refresh_doc_language/refresh_language_servers doesn't resend
        // text_document_did_close. Since we called `text_document_did_close`
        // we have fully unregistered this document from its LS
        doc.language_servers.clear();
        doc.set_path(Some(path));
        doc.detect_editor_config();
        self.refresh_doc_language(doc_id)
    }

    pub fn refresh_doc_language(&mut self, doc_id: DocumentId) {
        let loader = self.syn_loader.load();
        let doc = doc_mut!(self, &doc_id);
        doc.detect_language(&loader);
        doc.detect_editor_config();
        doc.detect_indent_and_line_ending();
        // The document's language servers were detached before (a new path) or may change with
        // the language: what they computed goes, and the servers now serving it are asked again.
        doc.clear_language_server_data();
        self.launch_language_servers(doc_id);
        let doc = doc_mut!(self, &doc_id);
        let diagnostics = Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, doc);
        doc.replace_diagnostics(diagnostics, &[], None);
        self.language_servers_did_change(doc_id);
    }

    /// Launch a language server for a given document. Returns whether the document's language
    /// servers changed (what the previous ones computed for it is dropped then).
    pub fn launch_language_servers(&mut self, doc_id: DocumentId) -> bool {
        if !self.config().lsp.enable {
            return false;
        }
        // if doc doesn't have a URL it's a scratch buffer, ignore it
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return false;
        };
        let Some(doc_url) = doc.url() else {
            return false;
        };
        let (lang, path) = (doc.language.clone(), doc.path());
        let config = doc.config.load();
        let root_dirs = &config.workspace_lsp_roots;

        let workspace = doc.workspace_root();
        let trust = self.workspace_trust.query(workspace, TrustQuery::Lsp);
        if !trust.is_trusted() {
            return false;
        }

        // store only successfully started language servers
        let language_servers = lang.as_ref().map_or_else(HashMap::default, |language| {
            self.language_servers
                .get(language, path, root_dirs, config.lsp.snippets)
                .filter_map(|(lang, client)| match client {
                    Ok(client) => Some((lang, client)),
                    Err(err) => {
                        if let helix_lsp::Error::ExecutableNotFound(err) = err {
                            // Silence by default since some language servers might just not be installed
                            log::debug!(
                                "Language server not found for `{}` {} {}", language.scope, lang, err,
                            );
                        } else {
                            log::error!(
                                "Failed to initialize the language servers for `{}` - `{}` {{ {} }}",
                                language.scope,
                                lang,
                                err
                            );
                        }
                        None
                    }
                })
                .collect::<HashMap<_, _>>()
        });

        if language_servers.is_empty() && doc.language_servers.is_empty() {
            return false;
        }

        let language_id = doc.language_id().map(ToOwned::to_owned).unwrap_or_default();

        // A server leaving or joining (`:lang`, `:lsp-restart`, ...) makes what the previous
        // ones computed for the document stale.
        let servers_changed = doc.language_servers.len() != language_servers.len()
            || doc.language_servers.iter().any(|(name, doc_ls)| {
                language_servers
                    .get(name)
                    .is_none_or(|ls| ls.id() != doc_ls.id())
            });
        if servers_changed {
            doc.clear_language_server_data();
        }

        // only spawn new language servers if the servers aren't the same
        let doc_language_servers_not_in_registry =
            doc.language_servers.iter().filter(|(name, doc_ls)| {
                language_servers
                    .get(*name)
                    .is_none_or(|ls| ls.id() != doc_ls.id())
            });

        for (_, language_server) in doc_language_servers_not_in_registry {
            language_server.text_document_did_close(doc.identifier());
        }

        let language_servers_not_in_doc = language_servers.iter().filter(|(name, ls)| {
            doc.language_servers
                .get(*name)
                .is_none_or(|doc_ls| ls.id() != doc_ls.id())
        });

        for (_, language_server) in language_servers_not_in_doc {
            // TODO: this now races with on_init code if the init happens too quickly
            language_server.text_document_did_open(
                doc_url.clone(),
                doc.version(),
                doc.text(),
                language_id.clone(),
            );
        }

        doc.language_servers = language_servers;
        servers_changed
    }

    fn _refresh(&mut self) {
        let config = self.config();

        // Reset the inlay hints annotations *before* updating the views, that way we ensure they
        // will disappear during the `.sync_change(doc)` call below.
        //
        // We can't simply check this config when rendering because inlay hints are only parts of
        // the possible annotations, and others could still be active, so we need to selectively
        // drop the inlay hints.
        if !config.lsp.display_inlay_hints {
            for doc in self.documents_mut() {
                doc.reset_all_inlay_hints();
            }
        }

        for (view, _) in self.tree.all_views_mut() {
            let doc = doc_mut!(self, &view.doc);
            view.gutters = config.gutters.clone();
            view.ensure_cursor_in_view(doc, config.scrolloff)
        }
        // the right sides of the diffs keep their connector in front of the new gutters
        self.diff_gutters_changed(&config.gutters);
        self.sync_tab_buffers();
    }

    /// Adds `doc` to the buffers of the tab page of `view` (the tab shown for `None`).
    fn add_to_tab_of(&mut self, view: Option<ViewId>, doc: DocumentId) {
        let tab = match view {
            Some(view) => self.tree.tab_of(view),
            None => Some(self.tree.active_tab()),
        };
        if let Some(root) = tab.and_then(|tab| self.tree.tab_root(tab)) {
            self.tab_buffers.entry(root).or_default().insert(doc);
        }
    }

    /// Takes `doc` out of the buffers of tab page `index` (unless a window of the tab shows
    /// it): `:tab-new file` opens a file for the new tab, not for the one it was run in.
    pub fn remove_from_tab(&mut self, index: usize, doc: DocumentId) {
        let shown = self
            .tree
            .tab_views(index)
            .into_iter()
            .any(|view| self.tree.try_get(view).is_some_and(|view| view.doc == doc));
        if shown {
            return;
        }
        if let Some(root) = self.tree.tab_root(index) {
            if let Some(buffers) = self.tab_buffers.get_mut(&root) {
                buffers.remove(&doc);
            }
        }
    }

    /// Keeps the buffers of the tab pages in step with the tabs and buffers: the closed
    /// tabs and buffers go, every buffer a window shows is in its tab, and the buffers no
    /// tab has any more (their tab closed) join the tab shown, so that none goes missing.
    fn sync_tab_buffers(&mut self) {
        let roots: Vec<ViewId> = (0..self.tree.tab_count())
            .filter_map(|tab| self.tree.tab_root(tab))
            .collect();
        let documents = &self.documents;
        self.tab_buffers.retain(|root, buffers| {
            buffers.retain(|doc| documents.contains_key(doc));
            roots.contains(root)
        });
        for (tab, root) in roots.iter().enumerate() {
            for view in self.tree.tab_views(tab) {
                if let Some(view) = self.tree.try_get(view) {
                    self.tab_buffers.entry(*root).or_default().insert(view.doc);
                }
            }
        }
        let orphans: Vec<DocumentId> = self
            .documents
            .keys()
            .filter(|doc| {
                !self
                    .tab_buffers
                    .values()
                    .any(|buffers| buffers.contains(doc))
            })
            .copied()
            .collect();
        if let Some(root) = self.tree.tab_root(self.tree.active_tab()) {
            self.tab_buffers.entry(root).or_default().extend(orphans);
        }
    }

    /// Whether the bufferline of the tab page shown lists `doc` (see
    /// [`Self::bufferline_documents`]).
    pub fn in_bufferline(&self, doc: DocumentId) -> bool {
        if self.config().bufferline.scope == BufferLineScope::Global || self.tree.tab_count() < 2 {
            return true;
        }
        // the buffer of the focused window always is (whatever the bookkeeping says)
        self.tree
            .try_get(self.tree.focus)
            .is_some_and(|view| view.doc == doc)
            || self
                .tree
                .tab_root(self.tree.active_tab())
                .and_then(|root| self.tab_buffers.get(&root))
                .is_some_and(|buffers| buffers.contains(&doc))
    }

    /// The buffers the bufferline lists, in order: every open buffer, or with
    /// `[editor.bufferline] scope = "tab"` (the default) and several tab pages, the ones of
    /// the tab shown. `gn` / `gp` and the buffer picker go through these.
    pub fn bufferline_documents(&self) -> impl Iterator<Item = &Document> {
        self.documents().filter(|doc| self.in_bufferline(doc.id()))
    }

    fn replace_document_in_view(&mut self, current_view: ViewId, doc_id: DocumentId) {
        let scrolloff = self.config().scrolloff;
        self.add_to_tab_of(Some(current_view), doc_id);
        let view = self.tree.get_mut(current_view);

        view.doc = doc_id;
        let doc = doc_mut!(self, &doc_id);

        doc.ensure_view_init(view.id);
        doc.mark_as_focused();

        view.ensure_cursor_in_view(doc, scrolloff)
    }

    pub fn switch(&mut self, id: DocumentId, action: Action) {
        use crate::tree::Layout;

        if !self.documents.contains_key(&id) {
            log::error!("cannot switch to document that does not exist (anymore)");
            return;
        }

        if !matches!(action, Action::Load) {
            self.enter_normal_mode();
        }

        let focust_lost = match action {
            Action::Replace => {
                let (view, doc) = current_ref!(self);
                // If the current view is an empty scratch buffer and is not displayed in any other views, delete it.
                // Boolean value is determined before the call to `view_mut` because the operation requires a borrow
                // of `self.tree`, which is mutably borrowed when `view_mut` is called.
                let remove_empty_scratch = !doc.is_modified()
                    // If the buffer has no path and is not modified, it is an empty scratch buffer.
                    && doc.path().is_none()
                    // If the buffer we are changing to is not this buffer
                    && id != doc.id
                    // Ensure the buffer is not displayed in any other splits.
                    && !self
                        .tree
                        .traverse()
                        .any(|(_, v)| v.doc == doc.id && v.id != view.id);

                let (view, doc) = current!(self);
                let view_id = view.id;
                // The document that loses the focus (the window shows `id` next).
                let left = doc.id;

                // Append any outstanding changes to history in the old document.
                doc.append_changes_to_history(view);

                if remove_empty_scratch {
                    // Copy `doc.id` into a variable before calling `self.documents.remove`, which requires a mutable
                    // borrow, invalidating direct access to `doc.id`.
                    let id = doc.id;
                    self.documents.remove(&id);

                    // Remove the scratch buffer from any jumplists, in every tab
                    for (view, _) in self.tree.all_views_mut() {
                        view.remove_document(&id);
                    }
                } else {
                    view.push_jump(doc);
                    // Set last accessed doc if it is a different document
                    if doc.id != id {
                        view.add_to_history(view.doc);
                        // Set last modified doc if modified and last modified doc is different
                        if std::mem::take(&mut doc.modified_since_accessed)
                            && view.last_modified_docs[0] != Some(view.doc)
                        {
                            view.last_modified_docs = [Some(view.doc), view.last_modified_docs[0]];
                        }
                    }
                }

                self.replace_document_in_view(view_id, id);

                // The document left, not the one shown now (a removed scratch buffer is
                // closed, not left).
                if left != id && !remove_empty_scratch {
                    dispatch(DocumentFocusLost {
                        editor: self,
                        doc: left,
                    });
                }
                return;
            }
            Action::Load => {
                let view_id = view!(self).id;
                self.add_to_tab_of(Some(view_id), id);
                let doc = doc_mut!(self, &id);
                doc.ensure_view_init(view_id);
                doc.mark_as_focused();
                return;
            }
            Action::HorizontalSplit | Action::VerticalSplit => {
                let focus_lost = self.tree.try_get(self.tree.focus).map(|view| view.doc);
                // copy the current view, unless there is no view yet
                let view = self
                    .tree
                    .try_get(self.tree.focus)
                    .filter(|v| id == v.doc) // Different Document
                    .cloned()
                    .unwrap_or_else(|| View::new(id, self.config().gutters.clone()));
                let view_id = self.tree.split(
                    view,
                    match action {
                        Action::HorizontalSplit => Layout::Horizontal,
                        Action::VerticalSplit => Layout::Vertical,
                        _ => unreachable!(),
                    },
                );
                self.add_to_tab_of(Some(view_id), id);
                // initialize selection for view
                let scrolloff = self.config().scrolloff;
                let doc = doc_mut!(self, &id);
                doc.ensure_view_init(view_id);
                doc.mark_as_focused();
                // where the cursor was last, seen in a window of another size
                if let Some(view) = self.tree.try_get(view_id) {
                    view.ensure_cursor_in_view(doc, scrolloff);
                }
                focus_lost
            }
        };

        self._refresh();
        if let Some(focus_lost) = focust_lost {
            dispatch(DocumentFocusLost {
                editor: self,
                doc: focus_lost,
            });
        }
    }

    /// Shows the document `id` in a new window along an edge of the screen (see
    /// [`crate::tree::Tree::dock`]), focused when `focus` is set.
    pub fn dock(
        &mut self,
        id: DocumentId,
        side: crate::tree::DockSide,
        size: u16,
        focus: bool,
    ) -> ViewId {
        let view = View::new(id, self.config().gutters.clone());
        let view_id = self.tree.dock(view, side, size);
        self.add_to_tab_of(Some(view_id), id);
        let doc = doc_mut!(self, &id);
        doc.ensure_view_init(view_id);
        if focus {
            self.focus(view_id);
        }
        self._refresh();
        view_id
    }

    /// Generate an id for a new document and register it.
    fn new_document(&mut self, mut doc: Document) -> DocumentId {
        let id = self.next_document_id;
        // Safety: adding 1 from 1 is fine, practically impossible to reach usize max
        self.next_document_id =
            DocumentId(unsafe { NonZeroUsize::new_unchecked(self.next_document_id.0.get() + 1) });
        doc.id = id;
        self.documents.insert(id, doc);
        // opened in the tab page shown
        self.add_to_tab_of(None, id);

        let (save_sender, save_receiver) = tokio::sync::mpsc::unbounded_channel();
        self.saves.insert(id, save_sender);

        let stream = UnboundedReceiverStream::new(save_receiver).flatten();
        self.save_queue.push(stream);

        id
    }

    pub fn new_file_from_document(&mut self, action: Action, doc: Document) -> DocumentId {
        let id = self.new_document(doc);
        self.switch(id, action);
        id
    }

    pub fn new_file(&mut self, action: Action) -> DocumentId {
        self.new_file_from_document(
            action,
            Document::default(self.config.clone(), self.syn_loader.clone()),
        )
    }

    /// Use when Helix is opened with no arguments passed
    pub fn new_file_welcome(&mut self) -> DocumentId {
        self.new_file_from_document(
            Action::VerticalSplit,
            Document::default(self.config.clone(), self.syn_loader.clone()).with_welcome(),
        )
    }

    pub fn new_file_from_stdin(&mut self, action: Action) -> Result<DocumentId, Error> {
        let (stdin, encoding, has_bom) = crate::document::read_to_string(&mut stdin(), None)?;
        let doc = Document::from(
            helix_core::Rope::default(),
            Some((encoding, has_bom)),
            self.config.clone(),
            self.syn_loader.clone(),
        );
        let doc_id = self.new_file_from_document(action, doc);
        let doc = doc_mut!(self, &doc_id);
        let view = view_mut!(self);
        doc.ensure_view_init(view.id);
        let transaction =
            helix_core::Transaction::insert(doc.text(), doc.selection(view.id), stdin.into())
                .with_selection(Selection::point(0));
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
        Ok(doc_id)
    }

    pub fn document_id_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_by_path(path).map(|doc| doc.id)
    }

    /// Remove the diagnostics contributed by `provider` for `uri` from the workspace
    /// diagnostics map, dropping the entry entirely when the file has no diagnostics
    /// from any provider left.
    ///
    /// This is for pull responses which are authoritative, i.e. a language server
    /// reporting that a file has no diagnostics anymore. Pruning pushed
    /// (`publishDiagnostics`) empty reports for files without an open document is
    /// handled by [`Self::handle_lsp_diagnostics`] instead, which retains the
    /// diagnostics of such files.
    pub fn clear_diagnostics(&mut self, provider: &DiagnosticProvider, uri: &Uri) {
        let mut entry_is_empty = false;
        if let Some(diagnostics) = self.diagnostics.get_mut(uri) {
            diagnostics.retain(|(_, existing)| existing != provider);
            entry_is_empty = diagnostics.is_empty();
        }
        if entry_is_empty {
            self.diagnostics.remove(uri);
        }
    }

    // ??? possible use for integration tests
    /// Notes `path` (absolute) as the most recently used file, for the recent file picker
    /// (`[editor.recent-picker]`); written in the background.
    pub fn record_recent_file(&self, path: &Path) {
        let config = self.config();
        let recent = &config.recent_picker;
        if recent.enable && recent.max_save_entries > 0 {
            self.recent_files
                .record(path, &recent.recents_path, recent.max_save_entries);
        }
    }

    /// A file never remembered where its cursor was (`[editor.persistence]
    /// old-files-exclusions`: a commit message opens at its start).
    fn excluded_from_old_files(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        self.config()
            .persistence
            .old_files_exclusions
            .iter()
            .any(|regex| regex.is_match(&path))
    }

    /// Notes where the cursor was last in a document, for when its file is opened again: in
    /// this session, and in later ones with `[editor.persistence] old-files` (written to the
    /// file history then). Nothing without `remember-cursor-position`.
    pub fn note_last_position(&mut self, doc_id: DocumentId) {
        if !self.config().remember_cursor_position {
            return;
        }
        let Some(doc) = self.documents.get(&doc_id) else {
            return;
        };
        let (Some(path), Some((selection, view_position))) = (doc.path(), doc.last_position())
        else {
            return;
        };
        if self.excluded_from_old_files(path) {
            return;
        }
        // noted already (the window closed, then the buffer)
        if self
            .old_file_locs
            .get(path)
            .is_some_and(|(noted_position, noted)| {
                *noted_position == view_position && *noted == selection
            })
        {
            return;
        }
        let path = path.to_path_buf();
        if self.config().persistence.old_files {
            persistence::push_file_history(&FileHistoryEntry::new(
                path.clone(),
                view_position,
                selection.clone(),
            ));
        }
        self.old_file_locs.insert(path, (view_position, selection));
    }

    /// [`Editor::note_last_position`] for every document (on exit: the windows still open
    /// were not closed).
    pub fn note_last_positions(&mut self) {
        let ids: Vec<DocumentId> = self.documents.keys().copied().collect();
        for doc_id in ids {
            self.note_last_position(doc_id);
        }
    }

    /// Writes the recently used files still to write (on exit).
    pub fn flush_recent_files(&self) {
        let config = self.config();
        let recent = &config.recent_picker;
        self.recent_files
            .flush(&recent.recents_path, recent.max_save_entries.max(1));
    }

    pub fn open(&mut self, path: &Path, action: Action) -> Result<DocumentId, DocumentOpenError> {
        let path = helix_stdx::path::canonicalize(path);
        let id = self.document_id_by_path(&path);

        let id = if let Some(id) = id {
            id
        } else {
            let mut doc = Document::open(
                &path,
                None,
                true,
                self.config.clone(),
                self.syn_loader.clone(),
            )?;

            let diagnostics =
                Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, &doc);
            doc.replace_diagnostics(diagnostics, &[], None);

            let trust_full = self
                .workspace_trust
                .query(doc.workspace_root(), TrustQuery::Git)
                .is_trusted();
            if let Some(diff_base) = self.diff_providers.get_diff_base(&path, trust_full) {
                doc.set_diff_base(diff_base);
            }
            doc.set_version_control_head(
                self.diff_providers.get_current_head_name(&path, trust_full),
            );

            // The file was open before (in this session, or in an earlier one with
            // `[editor.persistence] old-files`): it opens where the cursor was last.
            if self.config().remember_cursor_position && !self.excluded_from_old_files(&path) {
                if let Some((view_position, selection)) = self.old_file_locs.get(&path) {
                    doc.remember_position(selection.clone(), *view_position);
                }
            }

            let id = self.new_document(doc);

            self.launch_language_servers(id);

            helix_event::dispatch(DocumentDidOpen {
                editor: self,
                doc: id,
                path: &path,
            });

            id
        };

        self.switch(id, action);
        self.record_recent_file(&path);

        Ok(id)
    }

    /// Opens a new tab page showing `doc_id` in a window of its own, and shows it.
    pub fn new_tab(&mut self, doc_id: DocumentId) {
        if !self.documents.contains_key(&doc_id) {
            return;
        }
        self.enter_normal_mode();
        self.tree.new_tab();
        let view = View::new(doc_id, self.config().gutters.clone());
        let view_id = self.tree.insert(view);
        self.add_to_tab_of(Some(view_id), doc_id);
        let doc = doc_mut!(self, &doc_id);
        doc.ensure_view_init(view_id);
        doc.mark_as_focused();
        self._refresh();
    }

    /// Shows tab page `index` (0-based); nothing for one that doesn't exist.
    pub fn switch_tab(&mut self, index: usize) {
        if index == self.tree.active_tab() || index >= self.tree.tab_count() {
            return;
        }
        self.enter_normal_mode();
        self.tree.switch_tab(index);
        let scrolloff = self.config().scrolloff;
        let (view, doc) = current!(self);
        doc.mark_as_focused();
        view.ensure_cursor_in_view(doc, scrolloff);
    }

    /// Closes the windows of the tab page shown, which closes it; the next one is shown.
    /// Not the last tab.
    pub fn close_tab(&mut self) -> Result<(), &'static str> {
        if self.tree.tab_count() < 2 {
            return Err("This is the only tab page");
        }
        let views = self.tree.tab_views(self.tree.active_tab());
        for view in views {
            self.close(view);
        }
        Ok(())
    }

    /// The label of tab page `index`: the file name of the buffer of its focused window (or
    /// `[scratch]`).
    pub fn tab_label(&self, index: usize) -> String {
        let Some(view) = self
            .tree
            .tab_focus(index)
            .and_then(|focus| self.tree.try_get(focus))
        else {
            return String::new();
        };
        self.documents
            .get(&view.doc)
            .map(|doc| match doc.path().and_then(|path| path.file_name()) {
                Some(name) => name.to_string_lossy().into_owned(),
                None => doc.display_name().to_string(),
            })
            .unwrap_or_default()
    }

    pub fn close(&mut self, id: ViewId) {
        // Closed already: closing a window of a diff or of a merge closes the others with it,
        // and a command closing several windows (`wonly`, `:quit-all`) gets to them after.
        if !self.tree.contains(id) {
            return;
        }
        // the documents whose cursor was last in this window
        let mut left = Vec::new();
        for doc in self.documents_mut() {
            if doc.last_view() == Some(id) {
                left.push(doc.id());
            }
            // Remove selections for the closed view on all documents.
            doc.remove_view(id);
        }
        for doc_id in left {
            self.note_last_position(doc_id);
        }

        self.tree.remove(id);
        self._refresh();

        // A window of a diff closed: the diff ends now (its other window too when it is the
        // old version's), so that closing the last real window quits (`hx --diff`, `:q`).
        if let Some(index) = self
            .diff_sessions
            .iter()
            .position(|session| session.side_of(id).is_some())
        {
            let session = self.diff_sessions.remove(index);
            self.end_diff_session(session);
        }
        // a window of the merge closed: the merge ends (the result stays)
        if self
            .merge
            .as_ref()
            .is_some_and(|merge| merge.pane_of(id).is_some())
        {
            self.close_merge();
        }
    }

    pub fn close_document(&mut self, doc_id: DocumentId, force: bool) -> Result<(), CloseError> {
        let doc = match self.documents.get(&doc_id) {
            Some(doc) => doc,
            None => return Err(CloseError::DoesNotExist),
        };
        if !force && doc.is_modified() {
            return Err(CloseError::BufferModified(doc.display_name().into_owned()));
        }

        // This will also disallow any follow-up writes
        self.saves.remove(&doc_id);
        // the selections saved in it go with it
        self.saved_selections
            .retain(|_, saved| saved.document != doc_id);

        enum Action {
            Close(ViewId),
            ReplaceDoc(ViewId, DocumentId),
        }

        // where the cursor was last, for when the file is opened again
        self.note_last_position(doc_id);

        // the windows of every tab: one in a tab not shown must not keep a closed buffer
        let actions: Vec<Action> = self
            .tree
            .all_views_mut()
            .filter_map(|(view, _focus)| {
                view.remove_document(&doc_id);

                if view.doc == doc_id {
                    // something was previously open in the view, switch to previous doc
                    if let Some(prev_doc) = view.docs_access_history.pop() {
                        Some(Action::ReplaceDoc(view.id, prev_doc))
                    } else {
                        // only the document that is being closed was in the view, close it
                        Some(Action::Close(view.id))
                    }
                } else {
                    None
                }
            })
            .collect();

        for action in actions {
            match action {
                Action::Close(view_id) => {
                    self.close(view_id);
                }
                Action::ReplaceDoc(view_id, doc_id) => {
                    self.replace_document_in_view(view_id, doc_id);
                }
            }
        }

        let doc = self.documents.remove(&doc_id).unwrap();

        // If the document we removed was visible in all views, we will have no more views. We don't
        // want to close the editor just for a simple buffer close, so we need to create a new view
        // containing either an existing document, or a brand new document.
        if self.tree.views().next().is_none() {
            let doc_id = self
                .documents
                .iter()
                .map(|(&doc_id, _)| doc_id)
                .next()
                .unwrap_or_else(|| {
                    self.new_document(Document::default(
                        self.config.clone(),
                        self.syn_loader.clone(),
                    ))
                });
            let view = View::new(doc_id, self.config().gutters.clone());
            let view_id = self.tree.insert(view);
            self.add_to_tab_of(Some(view_id), doc_id);
            let doc = doc_mut!(self, &doc_id);
            doc.ensure_view_init(view_id);
            doc.mark_as_focused();
        }

        self._refresh();

        helix_event::dispatch(DocumentDidClose { editor: self, doc });

        Ok(())
    }

    /// Commits a document's uncommitted changes (e.g. edits made by the pre-save hook) to its
    /// history, through a view showing it (the focused one when it does), so that a following
    /// save records them as part of the saved revision.
    fn commit_pending_changes(&mut self, doc_id: DocumentId) {
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        if doc.changes().is_empty() {
            return;
        }
        let focus = self.tree.focus;
        let view_id = if self.tree.contains(focus) && self.tree.get(focus).doc == doc_id {
            Some(focus)
        } else {
            self.tree
                .views()
                .find(|(view, _)| view.doc == doc_id)
                .map(|(view, _)| view.id)
        };
        if let Some(view_id) = view_id {
            let view = self.tree.get_mut(view_id);
            doc.append_changes_to_history(view);
        }
    }

    /// Gives each window (of every tab) the top row for the plugins' bar when a segment
    /// shows in it, or takes it back. Run when the segments change and before each frame
    /// (a window opened since gets its bar).
    pub fn sync_window_bars(&mut self) {
        let segments = &self.plugin_window_bar;
        for (view, _) in self.tree.all_views_mut() {
            view.bar = segments.iter().any(|segment| segment.shows_in(view));
        }
    }

    /// The plugins' segments showing in `view`, ordered by priority.
    pub fn window_bar_segments(&self, view: &View) -> Vec<&BarSegment> {
        let mut segments: Vec<&BarSegment> = self
            .plugin_window_bar
            .iter()
            .filter(|segment| segment.shows_in(view))
            .collect();
        segments.sort_by_key(|segment| segment.priority);
        segments
    }

    /// Starts a smooth scroll of `view` from `from` to where it is now (after a scroll
    /// command), when `[editor.smooth-scroll]` is enabled. A scroll started while another
    /// is under way starts from what is on the screen.
    pub fn animate_scroll(&mut self, view_id: ViewId, from: ViewPosition) {
        let config = self.config().smooth_scroll.clone();
        if !config.enable || config.duration == 0 {
            self.stop_scroll_animation();
            return;
        }
        let Some(view) = self.tree.try_get(view_id) else {
            return;
        };
        let Some(doc) = self.documents.get(&view.doc) else {
            return;
        };
        let now = Instant::now();
        // what the screen shows of a scroll under way
        let from = match &self.scroll_animation {
            Some(running)
                if running.view == view_id
                    && running.document == doc.id()
                    && running.version == doc.version() =>
            {
                Self::scroll_position(running, view, doc, now).unwrap_or(from)
            }
            _ => from,
        };
        let to = doc.view_offset(view_id);
        self.scroll_animation = Self::scroll_rows(view, doc, from, to)
            .filter(|rows| rows.abs() > 1)
            .map(|rows| ScrollAnimation {
                view: view_id,
                document: doc.id(),
                version: doc.version(),
                from,
                to,
                rows,
                start: now,
                duration: Duration::from_millis(config.duration),
                easing: config.easing,
                drawn: false,
            });
        use std::sync::atomic::Ordering;
        let generation = SMOOTH_SCROLL_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        if self.scroll_animation.is_none() {
            return;
        }
        // the frames of the animation (about 60 a second), until it is over (one more then)
        // or no longer the latest
        let until = now + Duration::from_millis(config.duration);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                loop {
                    sleep(Duration::from_millis(16)).await;
                    if SMOOTH_SCROLL_GENERATION.load(Ordering::Relaxed) != generation {
                        break;
                    }
                    helix_event::request_redraw();
                    if Instant::now() >= until {
                        break;
                    }
                }
            });
        }
    }

    /// Ends the smooth scroll under way, if any (the view is already where it goes).
    pub fn stop_scroll_animation(&mut self) {
        if self.scroll_animation.take().is_some() {
            SMOOTH_SCROLL_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The visual rows from `from` to `to` in `view` (negative upwards), when `to` is
    /// `from` moved down or up by them (and both are within reach).
    fn scroll_rows(
        view: &View,
        doc: &Document,
        from: ViewPosition,
        to: ViewPosition,
    ) -> Option<isize> {
        if from.horizontal_offset != to.horizontal_offset || from == to {
            return None;
        }
        let text = doc.text().slice(..);
        let len = text.len_chars();
        if from.anchor > len || to.anchor > len {
            return None;
        }
        let text_fmt = doc.text_format(view.inner_area(doc).width, None);
        let annotations = view.text_annotations(doc, None);
        let block = from.anchor.min(to.anchor);
        let row = |pos: usize| {
            helix_core::visual_offset_from_block(text, block, pos, &text_fmt, &annotations)
                .0
                .row as isize
        };
        let rows = (row(to.anchor) + to.vertical_offset as isize)
            - (row(from.anchor) + from.vertical_offset as isize);
        if rows == 0 || rows.abs() > SMOOTH_SCROLL_MAX_ROWS {
            return None;
        }
        // only when moving `from` by the rows lands on `to`
        let landed = helix_core::char_idx_at_visual_offset(
            text,
            from.anchor,
            from.vertical_offset as isize + rows,
            0,
            &text_fmt,
            &annotations,
        );
        (landed == (to.anchor, to.vertical_offset)).then_some(rows)
    }

    /// Where the screen shows the view of `animation` at `now`.
    fn scroll_position(
        animation: &ScrollAnimation,
        view: &View,
        doc: &Document,
        now: Instant,
    ) -> Option<ViewPosition> {
        let elapsed = now.saturating_duration_since(animation.start);
        if elapsed >= animation.duration {
            return None;
        }
        let progress = elapsed.as_secs_f64() / animation.duration.as_secs_f64();
        let rows = (animation.rows as f64 * animation.easing.apply(progress)).round() as isize;
        let text_fmt = doc.text_format(view.inner_area(doc).width, None);
        let (anchor, vertical_offset) = helix_core::char_idx_at_visual_offset(
            doc.text().slice(..),
            animation.from.anchor,
            animation.from.vertical_offset as isize + rows,
            0,
            &text_fmt,
            &view.text_annotations(doc, None),
        );
        Some(ViewPosition {
            anchor,
            horizontal_offset: animation.to.horizontal_offset,
            vertical_offset,
        })
    }

    /// Puts the view of the smooth scroll under way where this frame shows it, and returns
    /// what [`Self::end_scroll_frame`] puts back after drawing. Ends the animation when it
    /// is over, or when something else moved the view or changed the document.
    pub fn begin_scroll_frame(&mut self) -> Option<(DocumentId, ViewId, ViewPosition)> {
        let animation = self.scroll_animation.as_mut()?;
        let alive = self.tree.try_get(animation.view).and_then(|view| {
            let doc = self.documents.get(&animation.document)?;
            (view.doc == animation.document && doc.version() == animation.version)
                .then_some((view, doc))
        });
        let Some((view, doc)) = alive else {
            self.stop_scroll_animation();
            return None;
        };
        let actual = doc.view_offset(animation.view);
        if actual != animation.to {
            // the cursor kept in view after the command: the target, if it is in reach
            let rows = (!animation.drawn)
                .then(|| Self::scroll_rows(view, doc, animation.from, actual))
                .flatten();
            match rows {
                Some(rows) => {
                    animation.to = actual;
                    animation.rows = rows;
                }
                None => {
                    self.stop_scroll_animation();
                    return None;
                }
            }
        }
        animation.drawn = true;
        let shown = Self::scroll_position(animation, view, doc, Instant::now());
        let Some(shown) = shown else {
            self.stop_scroll_animation();
            return None;
        };
        let (document, view_id) = (animation.document, animation.view);
        doc_mut!(self, &document).set_view_offset(view_id, shown);
        Some((document, view_id, actual))
    }

    /// Puts back the view's position after a frame of a smooth scroll (see
    /// [`Self::begin_scroll_frame`]).
    pub fn end_scroll_frame(&mut self, frame: Option<(DocumentId, ViewId, ViewPosition)>) {
        if let Some((document, view, position)) = frame {
            if let Some(doc) = self.documents.get_mut(&document) {
                doc.set_view_offset(view, position);
            }
        }
    }

    /// Highlights `ranges` (chars) of the focused buffer for [`Config::yank_highlight`]
    /// milliseconds: they were just yanked.
    pub fn flash_yank(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        let millis = self.config().yank_highlight;
        let ranges: Vec<_> = ranges.into_iter().filter(|r| r.start < r.end).collect();
        if millis == 0 || ranges.is_empty() {
            self.yank_flash = None;
            return;
        }
        let Some(view) = self.tree.try_get(self.tree.focus) else {
            return;
        };
        let Some(doc) = self.documents.get(&view.doc) else {
            return;
        };
        let duration = Duration::from_millis(millis);
        self.yank_flash = Some(YankFlash {
            view: view.id,
            document: doc.id(),
            version: doc.version(),
            ranges,
            until: Instant::now() + duration,
        });
        // drawn once more when it is over, to take it away
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                sleep(duration).await;
                helix_event::request_redraw();
            });
        }
    }

    /// The ranges just yanked in `view`, if their highlight is not over.
    pub fn yank_flash_in(&self, view: &View, doc: &Document) -> Option<&[std::ops::Range<usize>]> {
        let flash = self.yank_flash.as_ref()?;
        (flash.view == view.id
            && flash.document == doc.id()
            && flash.version == doc.version()
            && Instant::now() < flash.until)
            .then_some(flash.ranges.as_slice())
    }

    /// Whether the command `name` may run (see [`Editor::command_hook`]).
    pub fn command_allowed(&mut self, name: &str, args: &str) -> bool {
        // Taken out while it runs: a command it runs itself is not asked about.
        let Some(mut hook) = self.command_hook.take() else {
            return true;
        };
        let allowed = hook(self, name, args);
        self.command_hook.get_or_insert(hook);
        allowed
    }

    pub fn save<P: Into<PathBuf>>(
        &mut self,
        doc_id: DocumentId,
        path: Option<P>,
        force: bool,
    ) -> anyhow::Result<()> {
        // convert a channel of futures to pipe into main queue one by one
        // via stream.then() ? then push into main future

        // Taken out while it runs, so a save it starts itself doesn't run it again.
        if let Some(mut hook) = self.pre_save_hook.take() {
            let allowed = hook(self, doc_id);
            self.pre_save_hook.get_or_insert(hook);
            self.commit_pending_changes(doc_id);
            if !allowed {
                bail!("the save was cancelled by a plugin");
            }
        }

        let path = path.map(|path| path.into());
        let doc = self
            .documents
            .get_mut(&doc_id)
            .ok_or_else(|| anyhow!("the document was closed before it was saved"))?;
        let doc_save_future = doc.save(path, force)?;

        // When a file is written to, notify the file event handler.
        // Note: This can be removed once proper file watching is implemented.
        let handler = self.language_servers.file_event_handler.clone();
        let future = async move {
            let res = doc_save_future.await;
            if let Ok(event) = &res {
                handler.file_changed(event.path.clone());
            }
            res
        };

        use futures_util::stream;

        self.saves
            .get(&doc_id)
            .ok_or_else(|| anyhow::format_err!("saves are closed for this document!"))?
            .send(stream::once(Box::pin(future)))
            .map_err(|err| anyhow!("failed to send save event: {}", err))?;

        self.write_count += 1;

        Ok(())
    }

    pub fn resize(&mut self, area: Rect) {
        if self.tree.resize(area) {
            self._refresh();
        };
    }

    pub fn focus(&mut self, view_id: ViewId) {
        if self.tree.focus == view_id {
            return;
        }

        // Reset mode to normal and ensure any pending changes are committed in the old document.
        self.enter_normal_mode();
        let (view, doc) = current!(self);
        doc.append_changes_to_history(view);
        self.ensure_cursor_in_view(view_id);

        let prev_id = std::mem::replace(&mut self.tree.focus, view_id);
        doc_mut!(self).mark_as_focused();

        let focus_lost = self.tree.get(prev_id).doc;
        dispatch(DocumentFocusLost {
            editor: self,
            doc: focus_lost,
        });
    }

    pub fn focus_next(&mut self) {
        self.focus(self.tree.next());
    }

    pub fn focus_prev(&mut self) {
        self.focus(self.tree.prev());
    }

    pub fn focus_direction(&mut self, direction: tree::Direction) {
        let current_view = self.tree.focus;
        if let Some(id) = self.tree.find_split_in_direction(current_view, direction) {
            self.focus(id)
        }
    }

    pub fn swap_split_in_direction(&mut self, direction: tree::Direction) {
        self.tree.swap_split_in_direction(direction);
    }

    pub fn transpose_view(&mut self) {
        self.tree.transpose();
    }

    pub fn resize_buffer(&mut self, resize_type: Resize, dimension: Dimension) {
        self.tree
            .resize_buffer(resize_type, dimension, &self.config());
    }

    pub fn toggle_focus_window(&mut self) {
        self.tree.toggle_focus_window();
    }

    pub fn should_close(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn ensure_cursor_in_view(&mut self, id: ViewId) {
        let config = self.config();
        let view = self.tree.get(id);
        let doc = doc_mut!(self, &view.doc);
        view.ensure_cursor_in_view(doc, config.scrolloff)
    }

    #[inline]
    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    #[inline]
    pub fn document_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    #[inline]
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    #[inline]
    /// Remove the variable values shown at the end of lines (see [`DebuggerConfig`]).
    pub fn clear_dap_inline_values(&mut self) {
        // Values still being fetched belong to what was cleared: drop them on arrival.
        self.dap_inline_values_generation = self.dap_inline_values_generation.wrapping_add(1);
        for doc in self.documents_mut() {
            if !doc.dap_inline_values.is_empty() {
                doc.dap_inline_values.clear();
            }
        }
    }

    pub fn documents_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.documents.values_mut()
    }

    pub fn document_by_path<P: AsRef<Path>>(&self, path: P) -> Option<&Document> {
        self.documents()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }

    pub fn document_by_path_mut<P: AsRef<Path>>(&mut self, path: P) -> Option<&mut Document> {
        self.documents_mut()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }

    /// Returns all supported diagnostics for the document
    pub fn doc_diagnostics<'a>(
        language_servers: &'a helix_lsp::Registry,
        diagnostics: &'a Diagnostics,
        document: &Document,
    ) -> impl Iterator<Item = helix_core::Diagnostic> + 'a {
        Editor::doc_diagnostics_with_filter(language_servers, diagnostics, document, |_, _| true)
    }

    /// Returns all supported diagnostics for the document
    /// filtered by `filter` which is invocated with the raw `lsp::Diagnostic` and the language server id it came from
    pub fn doc_diagnostics_with_filter<'a>(
        language_servers: &'a helix_lsp::Registry,
        diagnostics: &'a Diagnostics,
        document: &Document,
        filter: impl Fn(&lsp::Diagnostic, &DiagnosticProvider) -> bool + 'a,
    ) -> impl Iterator<Item = helix_core::Diagnostic> + 'a {
        let text = document.text().clone();
        let language_config = document.language.clone();
        document
            .uri()
            .and_then(|uri| diagnostics.get(&uri))
            .map(|diags| {
                diags.iter().filter_map(move |(diagnostic, provider)| {
                    let server_id = provider.language_server_id()?;
                    let ls = language_servers.get_by_id(server_id)?;
                    language_config
                        .as_ref()
                        .and_then(|c| {
                            c.language_servers.iter().find(|features| {
                                features.name == ls.name()
                                    && features.has_feature(LanguageServerFeature::Diagnostics)
                            })
                        })
                        .and_then(|_| {
                            if filter(diagnostic, provider) {
                                Document::lsp_diagnostic_to_diagnostic(
                                    &text,
                                    language_config.as_deref(),
                                    diagnostic,
                                    provider.clone(),
                                    ls.offset_encoding(),
                                )
                            } else {
                                None
                            }
                        })
                })
            })
            .into_iter()
            .flatten()
    }

    /// Gets the primary cursor position in screen coordinates,
    /// or `None` if the primary cursor is not visible on screen.
    pub fn cursor(&self) -> (Option<Position>, CursorKind) {
        let config = self.config();
        let (view, doc) = current_ref!(self);
        if let Some(mut pos) = self.cursor_cache.get(view, doc) {
            let inner = view.inner_area(doc);
            pos.col += inner.x as usize;
            pos.row += inner.y as usize;
            let cursorkind = config.cursor_shape.from_mode(self.mode);
            (Some(pos), cursorkind)
        } else {
            (None, CursorKind::default())
        }
    }

    /// Closes language servers with timeout. The default timeout is 10000 ms, use
    /// `timeout` parameter to override this.
    pub async fn close_language_servers(&self, timeout: Option<u64>) {
        // Remove all language servers from the file event handler.
        // Note: this is non-blocking.
        for client in self.language_servers.iter_clients() {
            self.language_servers
                .file_event_handler
                .remove_client(client.id());
        }

        // Enqueue shutdown+exit for every server (non-blocking fire-and-forget).
        for client in self.language_servers.iter_clients() {
            client.force_shutdown();
        }

        // Wait until shutdown+exit have actually been written to each server's stdin
        // before the runtime (and the pipes) are torn down, so well-behaved servers
        // can act on `exit` before kill_on_drop reaps them. This waits only on our
        // own outbound write -- not on any server response -- so a slow server (e.g.
        // gopls flushing logs) doesn't delay it. Capped so a wedged write can't hang
        // the quit.
        let cap = Duration::from_millis(timeout.unwrap_or(1000));
        let _ = tokio::time::timeout(cap, async {
            for client in self.language_servers.iter_clients() {
                client.wait_shutdown_flushed().await;
            }
        })
        .await;
    }

    pub async fn wait_event(&mut self) -> EditorEvent {
        // the loop only runs once or twice and would be better implemented with a recursion + const generic
        // however due to limitations with async functions that can not be implemented right now
        loop {
            tokio::select! {
                biased;

                Some(event) = self.save_queue.next() => {
                    self.write_count -= 1;
                    return EditorEvent::DocumentSaved(event)
                }
                Some(config_event) = self.config_events.1.recv() => {
                    return EditorEvent::ConfigEvent(config_event)
                }
                Some(message) = self.language_servers.incoming.next() => {
                    return EditorEvent::LanguageServerMessage(message)
                }
                Some(event) = self.debug_adapters.incoming.next() => {
                    return EditorEvent::DebuggerEvent(event)
                }

                _ = helix_event::redraw_requested() => {
                    if  !self.needs_redraw{
                        self.needs_redraw = true;
                        let timeout = Instant::now() + self.redraw_delay();
                        if timeout < self.idle_timer.deadline() && timeout < self.redraw_timer.deadline(){
                            self.redraw_timer.as_mut().reset(timeout)
                        }
                    }
                }

                _ = &mut self.redraw_timer  => {
                    self.redraw_timer.as_mut().reset(Instant::now() + Duration::from_secs(86400 * 365 * 30));
                    return EditorEvent::Redraw
                }
                _ = &mut self.idle_timer  => {
                    return EditorEvent::IdleTimer
                }
            }
        }
    }

    /// How long a requested redraw waits, so that the requests made meanwhile share its
    /// frame. The frames of a smooth scroll are paced by their own timer (about 60 a
    /// second) and drawn at once: waiting would leave a frame every 50 ms, two in a
    /// 150 ms scroll.
    pub fn redraw_delay(&self) -> Duration {
        if self.scroll_animation.is_some() {
            Duration::ZERO
        } else {
            Duration::from_millis(33)
        }
    }

    pub async fn flush_writes(&mut self) -> anyhow::Result<()> {
        while self.write_count > 0 {
            if let Some(save_event) = self.save_queue.next().await {
                self.write_count -= 1;

                let save_event = match save_event {
                    Ok(event) => event,
                    Err(err) => {
                        self.set_error(err.to_string());
                        bail!(err);
                    }
                };

                let doc = doc_mut!(self, &save_event.doc_id);
                doc.set_last_saved_revision(save_event.revision, save_event.save_time);
            }
        }

        Ok(())
    }

    /// Switches the editor into normal mode.
    pub fn enter_normal_mode(&mut self) {
        use helix_core::graphemes;

        if self.mode == Mode::Normal {
            return;
        }

        self.mode = Mode::Normal;
        let (view, doc) = current!(self);

        try_restore_indent(doc, view);

        // if leaving append mode, move cursor back by 1
        if doc.restore_cursor {
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| {
                let mut head = range.to();
                if range.head > range.anchor {
                    head = graphemes::prev_grapheme_boundary(text, head);
                }

                Range::new(range.from(), head)
            });

            doc.set_selection(view.id, selection);
            doc.restore_cursor = false;
        }
    }

    pub fn current_stack_frame(&self) -> Option<&dap::StackFrame> {
        self.debug_adapters.current_stack_frame()
    }

    /// Returns the id of a view that this doc contains a selection for, preferring the current
    /// view. If there are no selections the current view is initialized for the doc.
    pub fn get_synced_view_id(&mut self, id: DocumentId) -> ViewId {
        let current_view = view_mut!(self);
        let doc = self.documents.get_mut(&id).unwrap();
        if doc.selections().contains_key(&current_view.id) {
            current_view.id
        } else if let Some(view_id) = doc.selections().keys().next() {
            *view_id
        } else {
            doc.ensure_view_init(current_view.id);
            current_view.id
        }
    }

    pub fn set_cwd(&mut self, path: &Path) -> std::io::Result<()> {
        self.last_cwd = helix_stdx::env::set_current_working_dir(path)?;
        self.clear_doc_relative_paths();
        Ok(())
    }

    pub fn get_last_cwd(&mut self) -> Option<&Path> {
        self.last_cwd.as_deref()
    }

    pub fn jump_forward(&mut self, view_id: ViewId, count: usize) {
        if let Some(jump) = view_mut!(self, view_id).jumps.forward(count).cloned() {
            self.jump_to(view_id, jump);
        }
    }

    pub fn jump_backward(&mut self, view_id: ViewId, count: usize) {
        let view = view_mut!(self, view_id);
        let doc = doc_mut!(self, &view.doc);
        // `backward` may add the current selection, which refers to the current revision.
        doc.append_changes_to_history(view);
        if let Some(jump) = view.jumps.backward(view_id, doc, count).cloned() {
            self.jump_to(view_id, jump);
        }
    }

    fn jump_to(&mut self, view_id: ViewId, jump: Jump) {
        let selection = jump.selection(doc!(self, &jump.doc_id));
        let old_doc_id = view!(self, view_id).doc;
        if old_doc_id != jump.doc_id {
            self.replace_document_in_view(view_id, jump.doc_id);
            dispatch(DocumentFocusLost {
                editor: self,
                doc: old_doc_id,
            });
        }
        let (view, doc) = current!(self);
        doc.set_selection(view_id, selection);
        view.ensure_cursor_in_view_center(doc, self.config.load().scrolloff);
    }
}

fn try_restore_indent(doc: &mut Document, view: &mut View) {
    use helix_core::{
        chars::char_is_whitespace,
        line_ending::{line_end_char_index, str_is_line_ending},
        unicode::segmentation::UnicodeSegmentation,
        Operation, Transaction,
    };

    fn inserted_a_new_blank_line(changes: &[Operation], pos: usize, line_end_pos: usize) -> bool {
        if let [Operation::Retain(move_pos), Operation::Insert(ref inserted_str), Operation::Retain(_)] =
            changes
        {
            let mut graphemes = inserted_str.graphemes(true);
            move_pos + inserted_str.len() == pos
                && graphemes.next().is_some_and(str_is_line_ending)
                && graphemes.all(|g| g.chars().all(char_is_whitespace))
                && pos == line_end_pos // ensure no characters exists after current position
        } else {
            false
        }
    }

    let doc_changes = doc.changes().changes();
    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();
    let pos = range.cursor(text);
    let line_end_pos = line_end_char_index(&text, range.cursor_line(text));

    if inserted_a_new_blank_line(doc_changes, pos, line_end_pos) {
        // Removes tailing whitespaces for the primary selection only, preserving existing behavior
        let line_start_pos = text.line_to_char(range.cursor_line(text));
        let transaction =
            Transaction::change(doc.text(), [(line_start_pos, pos, None)].into_iter());
        doc.apply(&transaction, view.id);
    }
}

#[derive(Default)]
pub struct CursorCache(Cell<Option<Option<Position>>>);

impl CursorCache {
    pub fn get(&self, view: &View, doc: &Document) -> Option<Position> {
        if let Some(pos) = self.0.get() {
            return pos;
        }

        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let res = view.screen_coords_at_pos(doc, text, cursor);
        self.set(res);
        res
    }

    pub fn set(&self, cursor_pos: Option<Position>) {
        self.0.set(Some(cursor_pos))
    }

    pub fn reset(&self) {
        self.0.set(None)
    }
}

#[cfg(test)]
mod border_style_tests {
    use super::*;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn border_style_parses_every_value() {
        for (name, expected) in [
            ("none", BorderStyle::None),
            ("plain", BorderStyle::Plain),
            ("rounded", BorderStyle::Rounded),
            ("double", BorderStyle::Double),
            ("thick", BorderStyle::Thick),
            ("quadrant-inside", BorderStyle::QuadrantInside),
            ("quadrant-outside", BorderStyle::QuadrantOutside),
        ] {
            let config = config(&format!("border-style = \"{name}\""));
            assert_eq!(config.border_style, Some(expected), "{name}");
            assert_eq!(config.effective_border_style(), expected);
        }
        assert!(toml::from_str::<Config>("border-style = \"dotted\"").is_err());
    }

    #[test]
    fn rounded_corners_still_works_and_border_style_wins() {
        assert_eq!(
            Config::default().effective_border_style(),
            BorderStyle::Plain
        );
        assert!(!Config::default().border_rounded());

        let legacy = config("rounded-corners = true");
        assert_eq!(legacy.effective_border_style(), BorderStyle::Rounded);
        assert!(legacy.border_rounded());

        let both = config("rounded-corners = true\nborder-style = \"double\"");
        assert_eq!(both.effective_border_style(), BorderStyle::Double);
        assert!(!both.border_rounded());

        let none = config("border-style = \"none\"");
        assert!(none.borders_hidden());
        assert!(!config("border-style = \"plain\"").borders_hidden());
    }
}

#[cfg(test)]
mod bufferline_config_tests {
    use super::*;

    fn bufferline(toml: &str) -> Result<BufferLineConfig, toml::de::Error> {
        toml::from_str::<Config>(toml).map(|config| config.bufferline)
    }

    #[test]
    fn bufferline_accepts_the_string_and_the_table_forms() {
        // the upstream string form: the mode, the other fields at their defaults
        for (name, mode) in [
            ("always", BufferLineRenderMode::Always),
            ("never", BufferLineRenderMode::Never),
            ("multiple", BufferLineRenderMode::Multiple),
        ] {
            assert_eq!(
                bufferline(&format!("bufferline = \"{name}\"")).unwrap(),
                BufferLineConfig {
                    render_mode: mode,
                    ..BufferLineConfig::default()
                }
            );
        }

        // the table form
        assert_eq!(
            bufferline("[bufferline]\nrender-mode = \"multiple\"\nseparator = \"|\"").unwrap(),
            BufferLineConfig {
                render_mode: BufferLineRenderMode::Multiple,
                separator: "|".to_owned(),
                scope: BufferLineScope::Tab,
            }
        );
        // the scope: the tab page's buffers by default, or all of them
        assert_eq!(BufferLineConfig::default().scope, BufferLineScope::Tab);
        assert_eq!(
            bufferline("[bufferline]\nscope = \"global\"")
                .unwrap()
                .scope,
            BufferLineScope::Global
        );
        assert_eq!(
            bufferline("[bufferline]\nscope = \"tab\"").unwrap().scope,
            BufferLineScope::Tab
        );
        assert!(bufferline("[bufferline]\nscope = \"window\"").is_err());
        assert_eq!(
            bufferline("[bufferline]\nseparator = \"|\"").unwrap(),
            BufferLineConfig {
                separator: "|".to_owned(),
                ..BufferLineConfig::default()
            }
        );
        assert_eq!(bufferline("").unwrap(), BufferLineConfig::default());

        // still rejected: unknown modes, unknown keys, other types
        assert!(bufferline("bufferline = \"sometimes\"").is_err());
        assert!(bufferline("[bufferline]\nrender-mode = \"sometimes\"").is_err());
        assert!(bufferline("[bufferline]\nseprator = \"|\"").is_err());
        assert!(bufferline("bufferline = 3").is_err());
    }
}

#[cfg(test)]
mod notification_border_tests {
    use super::*;

    #[test]
    fn notification_border_keys_parse() {
        let config: Config = toml::from_str(
            r##"
            [notifications.border]
            gradient = true
            gradient-start = "#112233"
            gradient-end = "#445566"
            style = "double"
            "##,
        )
        .unwrap();
        let border = &config.notifications.border;
        assert!(border.gradient);
        assert_eq!(border.gradient_start, "#112233");
        assert_eq!(border.gradient_end, "#445566");
        assert_eq!(border.style, NotificationBorderStyle::Double);
        assert_eq!(border.style.border_style(), Some(BorderStyle::Double));

        for (name, style) in [
            ("solid", NotificationBorderStyle::Solid),
            ("dashed", NotificationBorderStyle::Dashed),
            ("dotted", NotificationBorderStyle::Dotted),
            ("none", NotificationBorderStyle::None),
            ("plain", NotificationBorderStyle::Plain),
            ("rounded", NotificationBorderStyle::Rounded),
            ("thick", NotificationBorderStyle::Thick),
            ("quadrant-inside", NotificationBorderStyle::QuadrantInside),
            ("quadrant-outside", NotificationBorderStyle::QuadrantOutside),
        ] {
            let config: Config =
                toml::from_str(&format!("notifications.border.style = \"{name}\"")).unwrap();
            assert_eq!(config.notifications.border.style, style, "{name}");
        }
        assert_eq!(NotificationBorderStyle::Solid.border_style(), Option::None);
        assert!(toml::from_str::<Config>("notifications.border.style = \"wavy\"").is_err());
    }
}

#[cfg(test)]
mod cursor_roundness_tests {
    use super::Config;

    fn roundness(value: &str) -> u8 {
        let config: Config = toml::from_str(&format!("cursor-roundness = {value}")).unwrap();
        config.cursor_roundness
    }

    #[test]
    fn the_popup_width_is_a_percentage_within_range() {
        assert_eq!(Config::default().popup_max_width_percent, 60u8);
        for (value, expected) in [
            ("10", 10u8),
            ("45", 45),
            ("100", 100),
            ("0", 10),
            ("-5", 10),
            ("250", 100),
            ("72.6", 73),
        ] {
            let config: Config =
                toml::from_str(&format!("popup-max-width-percent = {value}")).unwrap();
            assert_eq!(config.popup_max_width_percent, expected, "{value}");
        }
    }

    #[test]
    fn any_number_is_accepted_and_brought_into_range() {
        assert_eq!(Config::default().cursor_roundness, 0u8);
        for (value, expected) in [
            ("0", 0u8),
            ("1", 1),
            ("25", 25),
            ("50", 50),
            ("51", 50),
            ("255", 50),
            ("300", 50),
            ("9223372036854775807", 50),
            ("-1", 0),
            ("-9000", 0),
            ("12.4", 12),
            ("49.6", 50),
            ("1e9", 50),
            ("-0.5", 0),
            ("nan", 0),
            ("inf", 50),
        ] {
            assert_eq!(roundness(value), expected, "cursor-roundness = {value}");
        }
        // not a number: an error, as for any other option
        assert!(toml::from_str::<Config>("cursor-roundness = \"round\"").is_err());
    }

    #[test]
    fn smooth_scroll_easings() {
        use crate::editor::ScrollEasing;
        for easing in [
            ScrollEasing::Linear,
            ScrollEasing::EaseOut,
            ScrollEasing::EaseInOut,
        ] {
            // from nothing to the whole distance, never back
            let start: f64 = easing.apply(0.0);
            let end: f64 = easing.apply(1.0);
            assert_eq!(start, 0.0_f64, "{easing:?}");
            assert_eq!(end, 1.0_f64, "{easing:?}");
            let mut last = 0.0_f64;
            for step in 1..=100 {
                let covered = easing.apply(step as f64 / 100.0);
                assert!(covered >= last, "{easing:?} at {step}");
                last = covered;
            }
            // out of range is clamped
            assert_eq!(easing.apply(-1.0), 0.0_f64);
            assert_eq!(easing.apply(2.0), 1.0_f64);
        }
        assert_eq!(ScrollEasing::Linear.apply(0.5), 0.5_f64);
        // ease-out is ahead halfway; ease-in-out is halfway at half the time
        assert_eq!(ScrollEasing::EaseOut.apply(0.5), 0.875_f64);
        assert_eq!(ScrollEasing::EaseInOut.apply(0.5), 0.5_f64);
        assert!(ScrollEasing::EaseInOut.apply(0.25) < 0.25_f64);
    }
}

#[cfg(test)]
mod statusline_tests {
    use super::*;

    /// `language-servers` (or `lsp`) is a status line element, on the right by default.
    #[test]
    fn the_language_servers_element_parses_and_is_on_by_default() {
        let config: StatusLineConfig = toml::from_str(
            r#"left = ["language-servers"]
right = ["lsp", "position"]"#,
        )
        .unwrap();
        assert_eq!(config.left, vec![StatusLineElement::LanguageServers]);
        assert_eq!(
            config.right,
            vec![
                StatusLineElement::LanguageServers,
                StatusLineElement::Position
            ]
        );

        let default: Vec<StatusLineElement> = StatusLineConfig::default().right;
        assert!(
            default.contains(&StatusLineElement::LanguageServers),
            "{default:?}"
        );
        assert!(toml::from_str::<StatusLineConfig>(r#"right = ["language-server"]"#).is_err());
    }
}

#[cfg(test)]
mod remember_cursor_tests {
    use super::*;

    /// `remember-cursor-position` is on by default, and can be turned off.
    #[test]
    fn remember_cursor_position_is_on_by_default() {
        let default: bool = Config::default().remember_cursor_position;
        assert!(default);
        let config: Config = toml::from_str("remember-cursor-position = false").unwrap();
        assert!(!config.remember_cursor_position);
        let config: Config = toml::from_str("").unwrap();
        assert!(config.remember_cursor_position);
    }
}
