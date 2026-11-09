//! The explorer file tree panel (`[editor.explorer]`, commands
//! `explorer-*`, default key `<space>x`).
//!
//! This is the UI half of the feature; the tree model, configuration, and
//! runtime state live in `helix-view` (`helix-view/src/explorer.rs`).
//!
//! The [`Explorer`] component is installed once and then never removed:
//! toggling the panel only flips `editor.explorer_panel.visible`, so the
//! loaded tree survives hide/show cycles without any fs work. File
//! mutations (create/rename/delete/cut/copy/paste) are driven from the
//! panel: names are asked for in a cmdline popup, destructive steps are
//! confirmed in a [`ConfirmBox`].
//!
//! Key routing: while the panel is visible *and* focused, keys drive the
//! tree; unbound keys (and all keys while an editor keymap sequence like
//! `<space>` is pending) fall through to the editor, so multi-key editor
//! bindings keep working even with the panel focused. `/` moves the keys to
//! the filter box at the top of the panel until `Enter`/`Esc`.

use std::error::Error as _;
use std::io;
use std::path::{Component as PathComponent, Path, PathBuf};

use helix_core::unicode::segmentation::UnicodeSegmentation;
use helix_core::unicode::width::UnicodeWidthStr;
use helix_core::Position;
use helix_view::graphics::CursorKind;
use helix_view::icons::ICONS;
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use helix_view::{
    document::Mode,
    editor::{Action, CmdlineStyle, GradientBorderConfig, PointerShape},
    explorer::{
        copy_recursively, new_matcher, Entry, ExplorerClipboard, ExplorerSide, FloatDrag,
        FloatGeometry, ScanOptions, Tree, TreeParams, FILTER_RESULT_LIMIT, FILTER_SCAN_LIMIT,
    },
    graphics::{Color, Rect, Style},
    Editor,
};
use tui::buffer::Buffer as Surface;
use tui::text::Span;
use tui::widgets::{Block, Widget};

use crate::{
    commands::git::{change_marker, change_style, ChangeKind, StatusMap},
    compositor::{Component, Compositor, Context, Event, EventResult},
    job::Callback,
    ui::{completers, gradient_border::GradientBorder, CmdlinePopup, ConfirmBox, PromptEvent},
};

pub const EXPLORER_ID: &str = "explorer";

/// A movement of the editor's keymap that moves the tree's cursor while the tree has the
/// keyboard. The editor resolves the keys (sequences such as `gg` and `ge`, counts, the
/// user's own bindings) and hands the command to the tree instead of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    /// `gg`: the first row, or row N with a count.
    Top,
    /// `ge`: the last row.
    Bottom,
    /// `G` with a count: row N.
    Row,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    /// `gt`, `gc`, `gb`: the top, middle and bottom of the visible rows.
    WindowTop,
    WindowCenter,
    WindowBottom,
}

impl Motion {
    /// The motion of a static command of the keymap, by its name.
    pub fn from_command(name: &str) -> Option<Self> {
        Some(match name {
            "move_line_down"
            | "move_visual_line_down"
            | "extend_line_down"
            | "extend_visual_line_down" => Motion::Down,
            "move_line_up" | "move_visual_line_up" | "extend_line_up" | "extend_visual_line_up" => {
                Motion::Up
            }
            "goto_file_start" => Motion::Top,
            "goto_last_line" | "goto_file_end" => Motion::Bottom,
            "goto_line" => Motion::Row,
            "page_down" | "page_cursor_down" => Motion::PageDown,
            "page_up" | "page_cursor_up" => Motion::PageUp,
            "half_page_down" | "page_cursor_half_down" => Motion::HalfPageDown,
            "half_page_up" | "page_cursor_half_up" => Motion::HalfPageUp,
            "goto_window_top" => Motion::WindowTop,
            "goto_window_center" => Motion::WindowCenter,
            "goto_window_bottom" => Motion::WindowBottom,
            _ => return None,
        })
    }
}

/// Minimum editor width the panel always leaves for views, even on narrow
/// terminals.
const MIN_EDITOR_WIDTH: u16 = 8;
/// A one-column separator line between the panel and the editor.
const SEPARATOR_WIDTH: u16 = 1;
/// Narrowest the panel can be dragged to.
const MIN_PANEL_WIDTH: u16 = 4;
/// Drawn in front of the filter input (it is focused with `/`).
const FILTER_PREFIX: &str = "> ";
/// The title of the filter box at the top of the panel.
const FILTER_BOX_TITLE: &str = "Explorer";
/// The panel height from which the filter is a bordered box (three rows) rather than a line.
const FILTER_BOX_MIN_HEIGHT: u16 = 8;
/// How long a git status is kept before the panel asks for a new one (it is asked for while the
/// panel is drawn, so a hidden panel costs nothing).
const GIT_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
/// The same when `git status` failed, e.g. because the folder is not in a repository.
const GIT_STATUS_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// How long to wait before the next `git status`, after one that took `took`: in a huge
/// repository, where it takes seconds, git must not be running most of the time.
fn git_status_interval(took: std::time::Duration) -> std::time::Duration {
    took.saturating_mul(10)
        .clamp(GIT_STATUS_INTERVAL, GIT_STATUS_RETRY)
}

/// `TreeParams` as seen from the current editor state; what the tree ops and
/// the renderer both use.
pub fn tree_params(editor: &Editor) -> TreeParams {
    let config = editor.config();
    TreeParams::new(&config.explorer, &editor.explorer_panel)
        .with_scan(ScanOptions::from(&config.file_picker))
}

/// Shrinks the editor's framing area around the pinned explorer column so
/// that view layout never allocates columns under the panel. A pure
/// computation over the area `EditorView::render` derives (i.e. the area
/// already excludes the command line and buffer line rows), which keeps the
/// result stable pass over pass: the panel column is subtracted from the
/// current edges rather than remembered. Returns the area unchanged when the
/// panel is hidden or floating.
pub fn layout(editor: &Editor, editor_area: Rect) -> Rect {
    if !editor.explorer_panel.visible {
        return editor_area;
    }

    let side = editor.explorer_side();
    if side == ExplorerSide::Floating {
        // Floating windows overlay the editor; nothing is reserved.
        return editor_area;
    }

    let width = editor
        .explorer_width()
        .min(editor_area.width.saturating_sub(MIN_EDITOR_WIDTH));
    let reserved = width
        .saturating_add(SEPARATOR_WIDTH)
        .min(editor_area.width.saturating_sub(MIN_EDITOR_WIDTH));

    match side {
        ExplorerSide::Left => {
            let x = editor_area.x.saturating_add(reserved);
            let right = editor_area.x + editor_area.width;
            Rect::new(
                x,
                editor_area.y,
                right.saturating_sub(x),
                editor_area.height,
            )
        }
        ExplorerSide::Right => Rect::new(
            editor_area.x,
            editor_area.y,
            editor_area.width.saturating_sub(reserved),
            editor_area.height,
        ),
        ExplorerSide::Floating => editor_area,
    }
}

/// Checks a name typed into the create prompt: a path below the target
/// directory, so no `..`, no absolute path.
fn validate_relative_path(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    let valid = path.components().count() > 0
        && path
            .components()
            .all(|component| matches!(component, PathComponent::Normal(_)));
    if valid {
        Ok(path.to_path_buf())
    } else {
        Err(format!(
            "Invalid name \"{name}\": use a path below the selected folder"
        ))
    }
}

/// Checks a name typed into the rename prompt: exactly one path component.
fn validate_file_name(name: &str) -> Result<&str, String> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(PathComponent::Normal(_)), None) => Ok(name),
        _ => Err(format!(
            "Invalid name \"{name}\": a new name cannot contain a path"
        )),
    }
}

/// Removes a file, a symlink, or a directory tree (symlinks are unlinked, never
/// followed).
fn remove_path(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Whether `a` and `b` name the same directory entry (not following symlinks), as two
/// spellings of one name do on a case-insensitive file system.
fn same_entry(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows file systems are case-insensitive.
        let lower = |path: &Path| path.as_os_str().to_string_lossy().to_lowercase();
        a.parent() == b.parent() && lower(a) == lower(b)
    }
}

/// A popup asking for one line of text, in the configured cmdline style.
fn ask(
    editor: &Editor,
    title: String,
    initial: Option<String>,
    on_event: impl FnMut(&mut Context, &str, PromptEvent) + 'static,
) -> CmdlinePopup {
    let style = editor.config().cmdline.style;
    // The bottom cmdline has no frame to carry the title, so the title becomes
    // the label in front of the input.
    let label = if style == CmdlineStyle::Bottom {
        format!("{title}: ")
    } else {
        title
    };
    let popup = CmdlinePopup::new(label.into(), None, completers::none, on_event, style);
    match initial {
        Some(line) => popup.with_line(line, editor),
        None => popup,
    }
}

/// Pushes `layer` on top of the compositor once the current event has been
/// handled.
fn push_layer(layer: impl Component + 'static) -> EventResult {
    EventResult::Consumed(Some(Box::new(move |compositor, _cx| {
        compositor.push(Box::new(layer));
    })))
}

pub struct Explorer {
    root: PathBuf,
    tree: Tree,
    /// Index into the visible row list.
    cursor: usize,
    /// First rendered row (scroll offset), adjusted during render.
    scroll: usize,
    /// The tree rows (below the filter box) of the last render, used to
    /// hit-test mouse events.
    area: Rect,
    /// The filter box row of the last render (empty when the panel is too
    /// short to have one).
    input_area: Rect,
    /// The whole panel content (filter box and rows) of the last render.
    panel: Rect,
    /// Column of the separator line of a pinned panel, the drag handle for
    /// resizing it.
    separator: Option<u16>,
    /// Terminal width of the last render.
    screen_width: u16,
    /// A drag of the separator is in progress.
    resizing: bool,
    /// The floating window's gradient border and the config it was built from.
    gradient_border: Option<(GradientBorderConfig, GradientBorder)>,
    /// Frame (border included) of the floating window, `None` when pinned.
    frame: Option<Rect>,
    /// Area the floating window moves in.
    float_bounds: Rect,
    /// A move/resize of the floating window in progress: what is dragged,
    /// where the pointer was grabbed and the geometry at that moment.
    float_drag: Option<(FloatDrag, (u16, u16), FloatGeometry)>,
    /// Text of the filter box.
    filter: String,
    /// Byte offset of the filter box's cursor.
    filter_cursor: usize,
    /// Keys edit the filter text instead of driving the tree.
    filter_editing: bool,
    /// The arrows moved the selection since the filter text last changed.
    filter_navigated: bool,
    /// Git status of the tree (changed files, folders with changes).
    git: StatusMap,
    /// When the status was last asked for: when the answer came, or `None` to ask again now.
    git_asked: Option<std::time::Instant>,
    /// A `git status` is running.
    git_in_flight: bool,
    /// How long to wait before asking again: longer when there is no repository to ask.
    git_interval: std::time::Duration,
    /// Highlights the characters a fuzzy filter matched, kept: a matcher is large to make.
    matcher: nucleo::Matcher,
}

impl Explorer {
    /// Creates the panel rooted at `root`, loading the root's children only.
    pub fn new(root: &Path, editor: &Editor) -> io::Result<Self> {
        let mut tree = Tree::new(root.to_path_buf(), tree_params(editor))?;
        tree.set_show_parent(true);
        // Start on the root row, below the `...` row.
        let cursor = usize::from(tree.visible().first().is_some_and(|e| e.is_parent));
        Ok(Self {
            root: root.to_path_buf(),
            tree,
            cursor,
            scroll: 0,
            area: Rect::default(),
            input_area: Rect::default(),
            panel: Rect::default(),
            separator: None,
            screen_width: 0,
            resizing: false,
            gradient_border: None,
            frame: None,
            float_bounds: Rect::default(),
            float_drag: None,
            filter: String::new(),
            filter_cursor: 0,
            filter_editing: false,
            filter_navigated: false,
            git: StatusMap::default(),
            git_asked: None,
            git_in_flight: false,
            git_interval: GIT_STATUS_INTERVAL,
            matcher: new_matcher(),
        })
    }

    /// The row the cursor is on, counted from 0.
    pub fn cursor_row(&self) -> usize {
        self.cursor
    }

    /// The number of rows of the tree.
    pub fn row_count(&self) -> usize {
        self.len()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn len(&self) -> usize {
        self.tree.visible().len()
    }

    /// The row under the cursor (if the tree is non-empty).
    fn entry(&self) -> Option<&Entry> {
        self.tree
            .visible()
            .get(self.cursor)
            .filter(|entry| !entry.is_parent)
    }

    /// Whether the cursor is on a directory that is not expanded.
    fn on_collapsed_dir(&self) -> bool {
        self.entry()
            .is_some_and(|entry| entry.is_dir && !self.tree.is_expanded(&entry.path))
    }

    /// Whether the cursor is on the `...` row.
    fn on_parent_row(&self) -> bool {
        self.tree
            .visible()
            .get(self.cursor)
            .is_some_and(|entry| entry.is_parent)
    }

    /// Makes the parent of the current root the new root, selecting the
    /// directory we came from. Does nothing at the filesystem root.
    fn go_up(&mut self, cx: &mut Context) -> EventResult {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else {
            return EventResult::Consumed(None);
        };
        match Tree::new(parent.clone(), tree_params(cx.editor)) {
            Ok(mut tree) => {
                tree.set_show_parent(true);
                let previous = std::mem::replace(&mut self.root, parent);
                self.tree = tree;
                self.scroll = 0;
                self.cursor = 0;
                self.select_path(&previous);
                cx.editor
                    .set_status(format!("Explorer root: {}", self.root.display()));
            }
            Err(err) => cx
                .editor
                .set_error(format!("Failed to open {}: {err}", parent.display())),
        }
        EventResult::Consumed(None)
    }

    /// Moves the cursor for a movement command of the editor's keymap (see [`Motion`]).
    /// `count` is the count typed before it, if any.
    pub fn apply_motion(&mut self, motion: Motion, count: Option<usize>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let last = len - 1;
        let times = count.unwrap_or(1).max(1);
        // The rows of the last render; before the first one, a page is one row.
        let page = (self.area.height as usize).max(1);
        let half = (page / 2).max(1);
        let window_top = self.scroll.min(last);
        let window_bottom = (self.scroll + page).min(len) - 1;
        let row = |n: usize| n.saturating_sub(1).min(last);

        self.cursor = match motion {
            Motion::Down => self.cursor.saturating_add(times).min(last),
            Motion::Up => self.cursor.saturating_sub(times),
            // `gg` goes to the top, `5gg` to the fifth row (like a line number).
            Motion::Top => count.map_or(0, row),
            Motion::Bottom => last,
            // `5G`; without a count there is nothing to go to.
            Motion::Row => count.map_or(self.cursor, row),
            Motion::PageDown => self
                .cursor
                .saturating_add(page.saturating_mul(times))
                .min(last),
            Motion::PageUp => self.cursor.saturating_sub(page.saturating_mul(times)),
            Motion::HalfPageDown => self
                .cursor
                .saturating_add(half.saturating_mul(times))
                .min(last),
            Motion::HalfPageUp => self.cursor.saturating_sub(half.saturating_mul(times)),
            Motion::WindowTop => window_top,
            Motion::WindowCenter => window_top + (window_bottom - window_top) / 2,
            Motion::WindowBottom => window_bottom,
        };
    }

    /// Reveals and selects the focused document's file. Used by
    /// `:explorer-reveal` and by opening the panel with `auto-reveal` set.
    /// Returns whether the file lives inside the tree root.
    pub fn reveal_document(&mut self, editor: &Editor) -> bool {
        let path = helix_view::doc!(editor).path().map(Path::to_path_buf);
        let Some(path) = path else {
            return false;
        };
        self.reveal_path(&path, editor)
    }

    /// Reveals `path` in the whole tree (dropping the filter, which could hide
    /// it) and selects it.
    fn reveal_path(&mut self, path: &Path, editor: &Editor) -> bool {
        let found = self.tree.reveal(path, tree_params(editor));
        self.sync_filter_text();
        if found {
            self.select_path(path);
        }
        found
    }

    /// Empties the filter box when the tree dropped its filter on its own.
    fn sync_filter_text(&mut self) {
        if self.tree.filter().is_none() && !self.filter.is_empty() {
            self.filter.clear();
            self.filter_cursor = 0;
            self.filter_editing = false;
        }
    }

    fn entry_cloned(&self) -> Option<Entry> {
        self.entry().cloned()
    }

    fn clamp_cursor(&mut self) {
        let last = self.len().saturating_sub(1);
        if self.cursor > last {
            self.cursor = last;
        }
    }

    /// Moves the cursor to the row for `path`, or failing that to `path`'s
    /// closest visible ancestor (so deleting a deleted file's row still
    /// lands somewhere sensible).
    fn select_path(&mut self, path: &Path) {
        let find = |tree: &Tree, needle: &Path| {
            tree.visible().iter().position(|entry| entry.path == needle)
        };
        if let Some(i) = find(&self.tree, path) {
            self.cursor = i;
        } else {
            let mut cur = path.parent().map(Path::to_path_buf);
            while let Some(ancestor) = cur {
                if let Some(i) = find(&self.tree, &ancestor) {
                    self.cursor = i;
                    break;
                }
                cur = ancestor.parent().map(Path::to_path_buf);
            }
        }
        self.clamp_cursor();
    }

    /// Reloads every expanded directory from disk (after file mutations) and
    /// parks the cursor at `keep` (default: wherever the cursor points).
    pub fn refresh_keep(&mut self, editor: &Editor, keep: Option<PathBuf>) {
        let params = tree_params(editor);
        let keep = keep.or_else(|| self.entry().map(|entry| entry.path.clone()));
        self.tree.refresh(params);
        self.git_asked = None;
        self.clamp_cursor();
        if let Some(keep) = keep {
            self.select_path(&keep);
        }
    }

    /// Like [`Self::refresh_keep`] for a path that was just created or moved:
    /// its ancestors are expanded so the entry is actually on screen.
    pub fn refresh_reveal(&mut self, editor: &Editor, path: PathBuf) {
        self.tree.refresh(tree_params(editor));
        self.git_asked = None;
        if !self.reveal_path(&path, editor) {
            self.clamp_cursor();
            self.select_path(&path);
        }
    }

    // --------------------------------------------------------------------
    // Filter box
    // --------------------------------------------------------------------

    /// The row of the best-scoring match of a fuzzy filter, the one a picker
    /// would put on top. `files_only` skips folders.
    fn best_row(&self, files_only: bool) -> Option<usize> {
        if !self.tree.filter().is_some_and(|filter| filter.is_fuzzy()) {
            return None;
        }
        let best = if files_only {
            self.tree.best_file()
        } else {
            self.tree.best_match()
        }?;
        self.tree
            .visible()
            .iter()
            .position(|entry| entry.path == best)
    }

    /// Applies the filter box text to the tree. A fuzzy query selects its best
    /// match, like a picker's top result; a wildcard query keeps the selection
    /// on its entry when that is still listed, otherwise moves to the first
    /// match.
    fn filter_changed(&mut self, editor: &mut Editor) {
        let params = tree_params(editor);
        let keep = self.entry().map(|entry| entry.path.clone());
        self.tree.set_filter(&self.filter, params);
        self.report_filter_limits(editor);
        self.filter_navigated = false;

        if self.tree.filter().is_none() {
            // Back to the full tree: keep the selected entry on screen, even
            // if only the filter had been showing it.
            match keep {
                Some(path) => {
                    self.tree.reveal(&path, params);
                    self.select_path(&path);
                }
                None => self.cursor = 0,
            }
        } else if let Some(row) = self.best_row(false) {
            self.cursor = row;
        } else {
            let kept = keep.and_then(|path| {
                self.tree
                    .visible()
                    .iter()
                    .position(|entry| entry.path == path)
            });
            // Fall back to the first row below the root.
            self.cursor = kept.unwrap_or(1);
        }
        self.clamp_cursor();
        self.scroll = 0;
    }

    /// Tells the user when a filter result is incomplete.
    /// The gradient border of `[editor.gradient-borders]`, made again when its config changed.
    fn gradient(
        &mut self,
        config: &helix_view::editor::Config,
        theme: &helix_view::Theme,
    ) -> Option<&mut GradientBorder> {
        let stale = self
            .gradient_border
            .as_ref()
            .is_none_or(|(cached, _)| *cached != config.gradient_borders);
        if stale {
            self.gradient_border = Some((
                config.gradient_borders.clone(),
                GradientBorder::from_theme(theme, &config.gradient_borders),
            ));
        }
        self.gradient_border.as_mut().map(|(_, border)| border)
    }

    /// The count at the right of the filter box, like a picker's: while filtering, the entries
    /// matching the filter out of those searched (still growing while the search runs); else
    /// the entries listed.
    fn filter_count_label(&self) -> String {
        match self.tree.filter_counts() {
            Some((matches, searched)) => {
                let scanning = if self.tree.scanning() {
                    "(scanning) "
                } else {
                    ""
                };
                format!("{scanning}{matches}/{searched}")
            }
            None => {
                let listed = self
                    .tree
                    .visible()
                    .iter()
                    .filter(|entry| !entry.is_parent && entry.depth > 0)
                    .count();
                format!("{listed}/{listed}")
            }
        }
    }

    /// The filter box heading a panel tall enough: a bordered box titled
    /// [`FILTER_BOX_TITLE`] (in the middle of its top border) with the prompt, the filter and
    /// the count. Sets `input_area` to where the prompt and the filter go.
    fn render_filter_box(&mut self, area: Rect, surface: &mut Surface, editor: &Editor) {
        let theme = &editor.theme;
        let config = editor.config();
        let border_style = explorer_border_style(theme, crate::ui::border_style(theme));

        if config.gradient_borders.enable {
            if let Some(border) = self.gradient(&config, theme) {
                if !config.borders_hidden() {
                    border.render(area, surface, theme, config.border_rounded());
                }
            }
        } else {
            Widget::render(
                Block::bordered()
                    .border_type(crate::ui::border_type(&config))
                    .border_style(border_style),
                area,
                surface,
            );
        }
        let title = format!(" {FILTER_BOX_TITLE} ");
        let title_width = title.width() as u16;
        if area.width >= title_width + 4 {
            let title_style = theme.try_get("ui.popup.title").unwrap_or(border_style);
            surface.set_string(
                area.x + (area.width - title_width) / 2,
                area.y,
                &title,
                title_style,
            );
        }

        // One column of padding inside the border on either side.
        let line = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.min(1),
        );
        let count = self.filter_count_label();
        let count_width = count.width() as u16;
        self.input_area = if line.width >= count_width + 1 + FILTER_PREFIX.width() as u16 + 4 {
            surface.set_stringn(
                line.right() - count_width,
                line.y,
                &count,
                count_width as usize,
                theme.get("ui.text"),
            );
            line.clip_right(count_width + 1)
        } else {
            line
        };

        let input = self.input_area;
        surface.set_stringn(
            input.x,
            input.y,
            FILTER_PREFIX,
            input.width as usize,
            theme.get("ui.text.focus"),
        );
        let prefix_width = FILTER_PREFIX.width() as u16;
        let avail = input.width.saturating_sub(prefix_width) as usize;
        if avail > 0 && !self.filter.is_empty() {
            let start = self.filter_window_start(avail);
            surface.set_stringn(
                input.x + prefix_width,
                input.y,
                &self.filter[start..],
                avail,
                theme.get("ui.text"),
            );
        }
    }

    fn report_filter_limits(&self, editor: &mut Editor) {
        if self.tree.filter_truncated() {
            editor.set_status(format!(
                "Explorer: filter searched only the first {FILTER_SCAN_LIMIT} entries"
            ));
        } else if self.tree.filter_capped() {
            editor.set_status(format!(
                "Explorer: showing the first {FILTER_RESULT_LIMIT} matches, type more to narrow"
            ));
        }
    }

    fn start_filter_edit(&mut self) {
        self.filter_editing = true;
        self.filter_cursor = self.filter.len();
    }

    fn clear_filter(&mut self, editor: &mut Editor) {
        self.filter.clear();
        self.filter_cursor = 0;
        self.filter_editing = false;
        self.filter_changed(editor);
    }

    fn filter_prev_boundary(&self) -> usize {
        self.filter[..self.filter_cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn filter_next_boundary(&self) -> usize {
        self.filter[self.filter_cursor..]
            .graphemes(true)
            .next()
            .map_or(self.filter.len(), |g| self.filter_cursor + g.len())
    }

    fn filter_insert(&mut self, text: &str) {
        self.filter.insert_str(self.filter_cursor, text);
        self.filter_cursor += text.len();
    }

    /// Byte offset at which the visible part of the filter text starts, so
    /// that the cursor stays inside `avail` columns.
    fn filter_window_start(&self, avail: usize) -> usize {
        let mut start = 0;
        while start < self.filter_cursor && self.filter[start..self.filter_cursor].width() >= avail
        {
            start += self.filter[start..]
                .graphemes(true)
                .next()
                .map_or(1, str::len);
        }
        start
    }

    /// Keys while the filter box has the keyboard.
    fn on_filter_key(&mut self, key: KeyEvent, cx: &mut Context) -> EventResult {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let plain = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;

        match key.code {
            // Enter accepts the filter and opens its first file match, so one
            // keypress does what typing the query asked for.
            KeyCode::Enter if key.modifiers == KeyModifiers::NONE => {
                self.filter_editing = false;
                if self.tree.filter().is_some() {
                    // Once the user picked a row with the arrows, Enter acts
                    // on that row; otherwise on the first file match.
                    if !self.filter_navigated {
                        let first_file = self.best_row(true).or_else(|| {
                            self.tree
                                .visible()
                                .iter()
                                .position(|entry| !entry.is_dir && !entry.is_parent)
                        });
                        if let Some(i) = first_file {
                            self.cursor = i;
                        }
                    }
                    if self.entry().is_some_and(|entry| !entry.is_dir) {
                        return self.on_primary_open(cx);
                    }
                    if self.filter_navigated {
                        return self.on_primary_open(cx);
                    }
                }
            }
            // The arrows move through the results without leaving the box, so
            // the query can still be edited afterwards.
            KeyCode::Down if key.modifiers == KeyModifiers::NONE => {
                if self.cursor + 1 < self.len() {
                    self.cursor += 1;
                }
                self.filter_navigated = true;
            }
            KeyCode::Up if key.modifiers == KeyModifiers::NONE => {
                self.cursor = self.cursor.saturating_sub(1);
                self.filter_navigated = true;
            }
            // Tab leaves the box and keeps the filter applied, to browse the
            // results with the tree keys.
            KeyCode::Tab if key.modifiers == KeyModifiers::NONE => {
                self.filter_editing = false;
            }
            // Escape drops the filter altogether.
            KeyCode::Esc => self.clear_filter(cx.editor),
            KeyCode::Backspace if plain => {
                if self.filter_cursor > 0 {
                    let start = self.filter_prev_boundary();
                    self.filter.replace_range(start..self.filter_cursor, "");
                    self.filter_cursor = start;
                    self.filter_changed(cx.editor);
                }
            }
            KeyCode::Delete if plain => {
                if self.filter_cursor < self.filter.len() {
                    let end = self.filter_next_boundary();
                    self.filter.replace_range(self.filter_cursor..end, "");
                    self.filter_changed(cx.editor);
                }
            }
            // After picking a row with the up/down arrows, left and right
            // work on that row like in the tree: expand and collapse folders,
            // or step up to the parent. Typing (or Home/End) gives the arrows
            // back to the text cursor.
            KeyCode::Right if plain && self.filter_navigated && self.on_collapsed_dir() => {
                return self.on_primary_open(cx);
            }
            KeyCode::Left if plain && self.filter_navigated => {
                return self.on_collapse_or_ascend();
            }
            KeyCode::Left if plain => self.filter_cursor = self.filter_prev_boundary(),
            KeyCode::Right if plain => self.filter_cursor = self.filter_next_boundary(),
            KeyCode::Home if plain => {
                self.filter_cursor = 0;
                self.filter_navigated = false;
            }
            KeyCode::End if plain => {
                self.filter_cursor = self.filter.len();
                self.filter_navigated = false;
            }
            KeyCode::Char('u') if ctrl => {
                self.filter.replace_range(..self.filter_cursor, "");
                self.filter_cursor = 0;
                self.filter_changed(cx.editor);
            }
            KeyCode::Char('w') if ctrl => {
                // Delete back over trailing separators, then one word.
                let before = &self.filter[..self.filter_cursor];
                let trimmed = before.trim_end_matches([' ', '/', '*']);
                let start = trimmed.rfind([' ', '/', '*']).map_or(0, |i| i + 1);
                self.filter.replace_range(start..self.filter_cursor, "");
                self.filter_cursor = start;
                self.filter_changed(cx.editor);
            }
            KeyCode::Char(c) if plain => {
                let mut buf = [0u8; 4];
                self.filter_insert(c.encode_utf8(&mut buf));
                self.filter_changed(cx.editor);
            }
            // C-h and C-l move focus between the tree and the buffers, so the
            // box lets them through and stops editing (the filter stays applied).
            KeyCode::Char('h' | 'l') if ctrl => {
                self.filter_editing = false;
                return EventResult::Ignored(None);
            }
            // Everything else is swallowed: the box owns the keyboard.
            _ => {}
        }
        EventResult::Consumed(None)
    }

    // --------------------------------------------------------------------
    // Key handling
    // --------------------------------------------------------------------

    fn on_nav_key(&mut self, key: KeyEvent, cx: &mut Context) -> EventResult {
        if key.modifiers != KeyModifiers::NONE {
            return EventResult::Ignored(None);
        }

        // Moving up and down (`j`/`k`, `gg`, `ge`, counts, paging, ...) is not handled
        // here: those keys reach the editor's keymap, which hands the movement commands it
        // resolves to back to the tree (see `Motion`), so the user's bindings apply.
        match key.code {
            KeyCode::Home => {
                self.cursor = 0;
                EventResult::Consumed(None)
            }
            // With a count, `G` is the keymap's `goto_line` (`5G`, see `Motion::Row`).
            KeyCode::Char('G') if cx.editor.count.is_some() => EventResult::Ignored(None),
            KeyCode::Char('G') | KeyCode::End => {
                self.cursor = self.len().saturating_sub(1);
                EventResult::Consumed(None)
            }
            KeyCode::Char('h') | KeyCode::Left => self.on_collapse_or_ascend(),
            KeyCode::Char('-') => self.on_parent_dir(),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => self.on_primary_open(cx),
            KeyCode::Char('v') => self.on_open_split(cx, Action::VerticalSplit),
            KeyCode::Char('s') => self.on_open_split(cx, Action::HorizontalSplit),
            KeyCode::Char('.') => self.on_reveal(cx),
            KeyCode::Char('/') => {
                self.start_filter_edit();
                EventResult::Consumed(None)
            }
            KeyCode::Char('R') => {
                self.refresh_keep(cx.editor, None);
                cx.editor.set_status("Explorer refreshed");
                EventResult::Consumed(None)
            }
            KeyCode::Char('H') => {
                let hidden = cx.editor.explorer_show_hidden();
                cx.editor.explorer_panel.show_hidden = Some(!hidden);
                cx.editor.set_status(if !hidden {
                    "Explorer: showing hidden files"
                } else {
                    "Explorer: hiding hidden files"
                });
                self.refresh_keep(cx.editor, None);
                EventResult::Consumed(None)
            }
            KeyCode::Char('a') => self.on_create(cx, false),
            KeyCode::Char('A') => self.on_create(cx, true),
            KeyCode::Char('r') => self.on_rename(cx),
            KeyCode::Char('d') => self.on_delete(cx),
            KeyCode::Char('x') => self.on_cut(cx),
            KeyCode::Char('c') => self.on_copy(cx),
            KeyCode::Char('p') => self.on_paste(cx),
            KeyCode::Char('q') => {
                cx.editor.explorer_panel.hide();
                EventResult::Consumed(None)
            }
            KeyCode::Esc => {
                // An applied filter goes first; the next Esc leaves the panel.
                if self.tree.filter().is_some() {
                    self.clear_filter(cx.editor);
                    return EventResult::Consumed(None);
                }
                // Pinning keeps the tree visible and just hands focus back;
                // a leftover float would cover the editor, so it closes.
                if cx.editor.explorer_side() == ExplorerSide::Floating {
                    cx.editor.explorer_panel.hide();
                }
                cx.editor.explorer_panel.focused = false;
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }

    /// `h`/`left`: collapse the directory at the cursor; otherwise ascend to
    /// the parent directory (nvim-tree habit).
    fn on_collapse_or_ascend(&mut self) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if entry.is_dir && self.tree.is_expanded(&entry.path) {
            self.tree.collapse(&entry.path);
            return EventResult::Consumed(None);
        }
        if let Some(parent) = entry.path.parent() {
            if !self.tree.is_root(parent) {
                if let Some(i) = self.tree.visible().iter().position(|e| e.path == parent) {
                    self.cursor = i;
                }
            }
        }
        EventResult::Consumed(None)
    }

    /// `-`: move the cursor to the parent directory row of the entry under
    /// the cursor (nvim-tree "dir up"). The cursor stays at the root when
    /// there is no parent row in the tree.
    fn on_parent_dir(&mut self) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if let Some(parent) = entry.path.parent() {
            if let Some(i) = self
                .tree
                .visible()
                .iter()
                .position(|e| e.path == parent && e.is_dir)
            {
                self.cursor = i;
            }
        }
        EventResult::Consumed(None)
    }

    /// `l`/`right`/Enter: expand/collapse a directory, or open the file.
    fn on_primary_open(&mut self, cx: &mut Context) -> EventResult {
        if self.on_parent_row() {
            return self.go_up(cx);
        }
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if entry.is_dir {
            if let Err(err) = self.tree.toggle(&entry.path, tree_params(cx.editor)) {
                cx.editor
                    .set_error(format!("Failed to read {}: {err}", entry.path.display()));
            }
            self.clamp_cursor();
            EventResult::Consumed(None)
        } else {
            self.open_entry(cx, Action::Replace)
        }
    }

    /// `v`/`s`: open the file in a split (a directory at the cursor is just
    /// toggled instead).
    fn on_open_split(&mut self, cx: &mut Context, action: Action) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if entry.is_dir {
            return self.on_primary_open(cx);
        }
        self.open_entry(cx, action)
    }

    fn open_entry(&self, cx: &mut Context, action: Action) -> EventResult {
        let Some(entry) = self.entry() else {
            return EventResult::Consumed(None);
        };
        let path = entry.path.clone();

        // A floating tree covers the buffers it opens, so any successful open
        // also closes the float; a pinned panel just hands focus back and
        // stays visible. Directory expansion and open errors keep the tree up.
        let floating = cx.editor.explorer_side() == ExplorerSide::Floating;

        // A plain open (`l`/Enter) prefers focusing the view that already shows
        // this file, instead of replacing another view's buffer or opening a
        // duplicate. Split requests (`v`/`s`) always split, as asked.
        if matches!(action, Action::Replace) {
            let canonical = helix_stdx::path::canonicalize(&path);
            if let Some(doc_id) = cx.editor.document_id_by_path(&canonical) {
                let view_id = cx
                    .editor
                    .tree
                    .views()
                    .find(|(view, focused)| view.doc == doc_id && *focused)
                    .or_else(|| cx.editor.tree.views().find(|(view, _)| view.doc == doc_id))
                    .map(|(view, _)| view.id);
                if let Some(view_id) = view_id {
                    cx.editor.focus(view_id);
                    // Hand keyboard focus back to the editor; the tree stays open.
                    cx.editor.explorer_panel.focused = false;
                    if floating {
                        cx.editor.explorer_panel.hide();
                    }
                    return EventResult::Consumed(None);
                }
            }
        }

        match cx.editor.open(&path, action) {
            Ok(_) => {
                // Hand keyboard focus back to the editor; the tree stays open.
                cx.editor.explorer_panel.focused = false;
                if floating {
                    cx.editor.explorer_panel.hide();
                }
                cx.editor.set_status(format!("Opened {}", path.display()));
            }
            Err(err) => {
                let message = match err.source() {
                    Some(source) => format!("unable to open \"{}\": {source}", path.display()),
                    None => format!("unable to open \"{}\"", path.display()),
                };
                cx.editor.set_error(message);
            }
        }
        EventResult::Consumed(None)
    }

    /// Moving and resizing the floating window by its border. Returns `None`
    /// when the event is not for the border.
    fn handle_float_mouse(&mut self, mouse: &MouseEvent, cx: &mut Context) -> Option<EventResult> {
        if let Some((drag, origin, start)) = self.float_drag {
            cx.editor.pointer_shape = float_pointer(drag);
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    let dx = mouse.column as i32 - origin.0 as i32;
                    let dy = mouse.row as i32 - origin.1 as i32;
                    let bounds = self.float_bounds;
                    cx.editor.explorer_panel.floating_geometry =
                        Some(drag.apply(start, dx, dy, bounds.width, bounds.height));
                }
                MouseEventKind::Up(_) => {
                    // Kept while the window is open; it opens centered next time.
                    self.float_drag = None;
                }
                _ => {}
            }
            return Some(EventResult::Consumed(None));
        }

        let frame = self.frame?;
        let drag = FloatDrag::at(frame, mouse.column, mouse.row)?;
        cx.editor.pointer_shape = float_pointer(drag);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            cx.editor.explorer_panel.focused = true;
            let bounds = self.float_bounds;
            let start = FloatGeometry {
                x: frame.x.saturating_sub(bounds.x),
                y: frame.y.saturating_sub(bounds.y),
                width: frame.width,
                height: frame.height,
            };
            self.float_drag = Some((drag, (mouse.column, mouse.row), start));
        }
        Some(EventResult::Consumed(None))
    }

    /// Resizes the pinned panel so its separator follows the pointer.
    fn resize_to(&self, column: u16, cx: &mut Context) {
        let width = match cx.editor.explorer_side() {
            ExplorerSide::Left => column.saturating_sub(self.panel.x),
            ExplorerSide::Right => self.panel.right().saturating_sub(column.saturating_add(1)),
            ExplorerSide::Floating => return,
        };
        let max = self
            .screen_width
            .saturating_sub(MIN_EDITOR_WIDTH + SEPARATOR_WIDTH)
            .max(MIN_PANEL_WIDTH);
        cx.editor.explorer_panel.width = Some(width.clamp(MIN_PANEL_WIDTH, max));
    }

    /// Mouse handling inside the panel: left press selects the row under
    /// the pointer and manipulates it (expand a directory, open a file), a
    /// press on the filter box starts typing a filter, and the wheel scrolls
    /// the selection. Dragging the separator line of a pinned panel resizes
    /// it. Events outside the panel bubble down to the editor underneath.
    fn handle_mouse(&mut self, mouse: MouseEvent, cx: &mut Context) -> EventResult {
        // A separator drag owns the pointer until the button is released,
        // wherever it wanders.
        if self.resizing {
            cx.editor.pointer_shape = PointerShape::ColResize;
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_to(mouse.column, cx),
                MouseEventKind::Up(_) => self.resizing = false,
                _ => {}
            }
            return EventResult::Consumed(None);
        }

        if let Some(result) = self.handle_float_mouse(&mouse, cx) {
            return result;
        }

        let panel = self.panel;
        let on_separator = self.separator == Some(mouse.column)
            && mouse.row >= panel.y
            && mouse.row < panel.bottom();
        if on_separator {
            cx.editor.pointer_shape = PointerShape::ColResize;
            return match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    cx.editor.explorer_panel.focused = true;
                    self.resizing = true;
                    EventResult::Consumed(None)
                }
                // Hovering/other buttons on the handle stay inert too.
                _ => EventResult::Consumed(None),
            };
        }

        if mouse.column < panel.x
            || mouse.column >= panel.right()
            || mouse.row < panel.y
            || mouse.row >= panel.bottom()
        {
            return EventResult::Ignored(None);
        }

        // Clicking anywhere in the panel gives it the keyboard.
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            cx.editor.explorer_panel.focused = true;
        }

        let area = self.area;
        // The filter box (or line, and the blank row below it).
        if mouse.row < area.y {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) && self.input_area.width > 0 {
                cx.editor.explorer_panel.focused = true;
                self.start_filter_edit();
            }
            return EventResult::Consumed(None);
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Clicking the tree while typing a filter ends the typing.
                self.filter_editing = false;
                // Rows are drawn from `scroll` at `area.y` downwards, so a
                // click at `row` maps to visible index `scroll + row - y`.
                let i = self.scroll + (mouse.row - area.y) as usize;
                if i >= self.len() {
                    // Blank space below the last row: consumed but inert.
                    return EventResult::Consumed(None);
                }
                self.cursor = i;
                if mouse.modifiers != KeyModifiers::NONE {
                    return EventResult::Consumed(None);
                }
                if self.on_parent_row() {
                    return self.go_up(cx);
                }
                let Some(entry) = self.entry_cloned() else {
                    return EventResult::Consumed(None);
                };
                if entry.is_dir {
                    if let Err(err) = self.tree.toggle(&entry.path, tree_params(cx.editor)) {
                        cx.editor
                            .set_error(format!("Failed to read {}: {err}", entry.path.display()));
                    }
                    self.clamp_cursor();
                } else {
                    self.open_entry(cx, Action::Replace);
                }
                EventResult::Consumed(None)
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let lines = cx.editor.config().scroll_lines.unsigned_abs().max(1);
                if mouse.kind == MouseEventKind::ScrollUp {
                    self.cursor = self.cursor.saturating_sub(lines);
                } else {
                    self.cursor = (self.cursor + lines).min(self.len().saturating_sub(1));
                }
                EventResult::Consumed(None)
            }
            // Everything else over the panel (drag, right/middle click,
            // click press) stays inside the panel: inert, not leaked to the
            // editor underneath.
            _ => EventResult::Consumed(None),
        }
    }

    /// `.`: reveal and select the focused document's file.
    fn on_reveal(&mut self, cx: &mut Context) -> EventResult {
        let path = helix_view::doc!(cx.editor).path().map(Path::to_path_buf);
        let Some(path) = path else {
            cx.editor.set_error("Current buffer has no file");
            return EventResult::Consumed(None);
        };
        if !self.reveal_path(&path, cx.editor) {
            cx.editor.set_error(format!(
                "{} is not inside the explorer root",
                path.display()
            ));
        }
        EventResult::Consumed(None)
    }

    // --------------------------------------------------------------------
    // File operations
    // --------------------------------------------------------------------

    /// `path` as shown to the user: relative to the tree root (`.` for the
    /// root itself).
    fn display_rel(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }

    /// The directory that subtree mutations (create/paste) act on: the
    /// selected directory itself, the parent of the selected file, or the tree
    /// root when nothing is selected.
    fn mutation_target(&self) -> PathBuf {
        match self.entry() {
            Some(entry) if entry.is_dir => entry.path.clone(),
            Some(entry) => entry
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// `a` (file, or folder when the name ends in `/`) and `A` (folder):
    /// asks for a name in a cmdline popup and creates it below the selected
    /// directory (the tree root when nothing is selected).
    fn on_create(&self, cx: &mut Context, folder_only: bool) -> EventResult {
        let target = self.mutation_target();
        let where_ = self.display_rel(&target);
        let title = if folder_only {
            format!("New folder in {where_}")
        } else {
            format!("New file/folder in {where_}  (name/ = folder)")
        };

        let popup = ask(
            cx.editor,
            title,
            None,
            move |cx: &mut Context, input: &str, event: PromptEvent| {
                if event != PromptEvent::Validate {
                    return;
                }
                let input = input.trim();
                if input.is_empty() {
                    return;
                }
                let is_dir = folder_only || input.ends_with('/');
                let relative = match validate_relative_path(input.trim_end_matches('/')) {
                    Ok(relative) => relative,
                    Err(message) => {
                        cx.editor.set_error(message);
                        return;
                    }
                };
                let dest = target.join(&relative);
                if dest.exists() {
                    cx.editor
                        .set_error(format!("{} already exists", dest.display()));
                    return;
                }

                let created = if is_dir {
                    std::fs::create_dir_all(&dest)
                } else {
                    dest.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&dest)
                                .map(drop)
                        })
                };
                match created {
                    Ok(()) => cx.editor.set_status(format!(
                        "Created {} {}",
                        if is_dir { "folder" } else { "file" },
                        dest.display()
                    )),
                    Err(err) => {
                        cx.editor
                            .set_error(format!("Failed to create {}: {err}", dest.display()));
                    }
                }
                // Refresh either way: a half-done create (parents made) shows.
                refresh_after(cx, dest);
            },
        );
        push_layer(popup)
    }

    /// `r`: asks for the new name of the entry under the cursor.
    fn on_rename(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if self.tree.is_root(&entry.path) {
            cx.editor.set_error("Cannot rename the explorer root");
            return EventResult::Consumed(None);
        }
        let from = entry.path.clone();
        let parent_dir = from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let name = entry.name.clone();
        let kind = if entry.is_dir { "folder" } else { "file" };

        let popup = ask(
            cx.editor,
            format!("Rename {kind} {name}"),
            Some(name),
            move |cx: &mut Context, input: &str, event: PromptEvent| {
                if event != PromptEvent::Validate {
                    return;
                }
                let input = input.trim();
                if input.is_empty() {
                    return;
                }
                let new_name = match validate_file_name(input) {
                    Ok(new_name) => new_name,
                    Err(message) => {
                        cx.editor.set_error(message);
                        return;
                    }
                };
                let dest = parent_dir.join(new_name);
                if dest == from {
                    return;
                }
                // On a case-insensitive file system (macOS, Windows) a new case of the same
                // name finds the entry itself: that rename is allowed.
                if std::fs::symlink_metadata(&dest).is_ok() && !same_entry(&from, &dest) {
                    cx.editor
                        .set_error(format!("Cannot rename: {} already exists", dest.display()));
                    return;
                }
                match std::fs::rename(&from, &dest) {
                    Ok(()) => {
                        // Open buffers follow their file, or saving them
                        // would recreate it under the old name.
                        retarget_buffers(cx.editor, &from, &dest);
                        cx.editor.set_status(format!(
                            "Renamed {} to {}",
                            from.display(),
                            dest.display()
                        ));
                    }
                    Err(err) => cx.editor.set_error(format!(
                        "Failed to rename {} to {}: {err}",
                        from.display(),
                        dest.display()
                    )),
                }
                refresh_after(cx, dest);
            },
        );
        push_layer(popup)
    }

    /// `d`: asks for confirmation, then deletes the entry under the cursor.
    fn on_delete(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if self.tree.is_root(&entry.path) {
            cx.editor.set_error("Cannot delete the explorer root");
            return EventResult::Consumed(None);
        }
        let path = entry.path.clone();
        let keep = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let (kind, warning) = if entry.is_dir {
            (
                "folder",
                "The folder and everything inside it will be removed.",
            )
        } else {
            ("file", "The file will be removed.")
        };
        let message = format!(
            "Delete {kind} {}?\n{}\n\n{warning} This cannot be undone.",
            entry.name,
            self.display_rel(&path)
        );

        push_layer(ConfirmBox::new(
            format!("Delete {kind}"),
            message,
            "Delete",
            move |_compositor, cx| {
                cx.editor
                    .set_status(format!("Deleting {}…", path.display()));
                let target = path.clone();
                // A big tree can take a while to remove: keep the UI alive.
                run_blocking(
                    cx,
                    move || remove_path(&target),
                    move |editor, compositor, result| {
                        match result {
                            Ok(()) => editor.set_status(format!("Deleted {}", path.display())),
                            Err(err) => editor
                                .set_error(format!("Failed to delete {}: {err}", path.display())),
                        }
                        if let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) {
                            explorer.refresh_keep(editor, Some(keep));
                        }
                    },
                );
            },
        ))
    }

    /// `x`: cut into the explorer's clipboard for the next `p`.
    fn on_cut(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        if self.tree.is_root(&entry.path) {
            cx.editor.set_error("Cannot cut the explorer root");
            return EventResult::Consumed(None);
        }
        cx.editor.explorer_clipboard = Some(ExplorerClipboard {
            path: entry.path,
            cut: true,
        });
        // Selection changes are not required for paste to work.
        cx.editor.set_status("Explorer: cut");
        EventResult::Consumed(None)
    }

    /// `c`: copy into the explorer's clipboard for the next `p`.
    fn on_copy(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        cx.editor.explorer_clipboard = Some(ExplorerClipboard {
            path: entry.path,
            cut: false,
        });
        cx.editor.set_status("Explorer: copied");
        EventResult::Consumed(None)
    }

    /// `p`: paste the clipboard entry in/beside the selection.
    fn on_paste(&self, cx: &mut Context) -> EventResult {
        let Some(clip) = cx.editor.explorer_clipboard.clone() else {
            cx.editor.set_error("Explorer clipboard is empty");
            return EventResult::Consumed(None);
        };

        let target_dir = self.mutation_target();
        let Some(source_name) = clip.path.file_name() else {
            cx.editor.set_error("Explorer clipboard has no file name");
            return EventResult::Consumed(None);
        };
        let dest = target_dir.join(source_name);

        if dest == clip.path {
            cx.editor.set_error("Pasting back onto the original path");
            return EventResult::Consumed(None);
        }
        if helix_view::explorer::is_within(&dest, &clip.path) {
            cx.editor
                .set_error("Cannot paste a path into its own subtree");
            return EventResult::Consumed(None);
        }

        // Not `exists`: a dangling link at `dest` is replaced as well.
        if std::fs::symlink_metadata(&dest).is_ok() {
            let message = format!(
                "{} already exists.\nIts contents will be overwritten.",
                self.display_rel(&dest)
            );
            push_layer(ConfirmBox::new(
                "Overwrite",
                message,
                "Overwrite",
                move |_compositor, cx| paste_now(cx, &clip, &dest),
            ))
        } else {
            paste_now(cx, &clip, &dest);
            EventResult::Consumed(None)
        }
    }

    // --------------------------------------------------------------------
    // Rendering
    // --------------------------------------------------------------------

    /// What comes before an entry. With `guides`, three columns per level: the lines of the
    /// folders that continue (`│  `) and the branch of the entry (`├─ `/`└─ `). Without,
    /// two blank columns per level.
    fn prefix(tree: &Tree, entry: &Entry, guides: bool) -> String {
        if entry.depth == 0 {
            return String::new();
        }
        if !guides {
            return "  ".repeat(entry.depth as usize);
        }
        // Every ancestor directory between root and entry (excluding the
        // root itself), nearest last.
        let mut ancestors: Vec<&Path> = Vec::with_capacity(entry.depth as usize);
        let mut cur = entry.path.parent();
        while let Some(parent) = cur {
            if tree.is_root(parent) {
                break;
            }
            ancestors.push(parent);
            cur = parent.parent();
        }
        let mut prefix = String::with_capacity((entry.depth as usize + 1) * 3 * 3);
        for ancestor in ancestors.iter().rev() {
            if tree.is_last_child(ancestor) {
                prefix.push_str("   "); // ancestor branch ended here
            } else {
                prefix.push_str("│  "); // vertical guide continues
            }
        }
        prefix.push_str(if entry.is_last { "└─ " } else { "├─ " });
        prefix
    }

    /// Asks for a new git status when the last one is old, and stores it when it arrives.
    fn request_git_status(&mut self, cx: &mut Context) {
        if !cx.editor.config().explorer.git_status {
            self.git = StatusMap::default();
            return;
        }
        if self.git_in_flight
            || self
                .git_asked
                .is_some_and(|asked| asked.elapsed() < self.git_interval)
        {
            return;
        }
        self.git_in_flight = true;
        let started = std::time::Instant::now();
        let root = self.root.clone();
        let trusted = crate::commands::git::workspace_trusted(cx.editor, &root);
        let providers = cx.editor.diff_providers.clone();
        cx.jobs.callback(async move {
            let result = crate::commands::git::changes_under(root.clone(), trusted, providers)
                .await
                .map(|changes| StatusMap::new(&changes, &root));
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) else {
                    return;
                };
                explorer.git_in_flight = false;
                explorer.git_asked = Some(std::time::Instant::now());
                // The panel may have been re-rooted while git was working.
                match result {
                    Ok(map) if explorer.root == root => {
                        explorer.git_interval = git_status_interval(started.elapsed());
                        if explorer.git != map {
                            explorer.git = map;
                            editor.needs_redraw = true;
                        }
                    }
                    Ok(_) => explorer.git_asked = None,
                    Err(err) => {
                        log::debug!("explorer git status: {err}");
                        explorer.git_interval = GIT_STATUS_RETRY;
                        explorer.git = StatusMap::default();
                    }
                }
            }));
            Ok(call)
        });
    }
}

/// Runs a slow file operation on a blocking thread, then `done` on the UI
/// thread with its result.
fn run_blocking<T: Send + 'static>(
    cx: &mut Context,
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
    done: impl FnOnce(&mut Editor, &mut Compositor, io::Result<T>) + Send + 'static,
) {
    cx.jobs.callback(async move {
        let result = tokio::task::spawn_blocking(work)
            .await
            .unwrap_or_else(|err| Err(io::Error::other(err.to_string())));
        let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
            done(editor, compositor, result)
        }));
        Ok(call)
    });
}

/// Points every open buffer under `from` at the same place under `to`, after
/// a rename or move on disk.
fn retarget_buffers(editor: &mut Editor, from: &Path, to: &Path) {
    let moved: Vec<_> = editor
        .documents
        .values()
        .filter_map(|doc| {
            let relative = doc.path()?.strip_prefix(from).ok()?;
            let new_path = if relative.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(relative)
            };
            Some((doc.id(), new_path))
        })
        .collect();
    for (id, path) in moved {
        editor.set_doc_path(id, &path);
    }
}

/// Moves or copies `from` to `to`. A move is a plain rename when it can be
/// (instant, even for huge trees) and a copy followed by removing the original
/// otherwise. `Ok(Some(err))` means the copy worked but the original could not
/// be removed.
fn transfer(from: &Path, to: &Path, cut: bool) -> io::Result<Option<io::Error>> {
    if cut && !to.exists() && std::fs::rename(from, to).is_ok() {
        return Ok(None);
    }
    copy_recursively(from, to)?;
    Ok(if cut { remove_path(from).err() } else { None })
}

/// Paste implementation shared by plain paste and overwrite confirmation. The
/// work happens off the UI thread.
fn paste_now(cx: &mut Context, clip: &ExplorerClipboard, dest: &Path) {
    let (from, to, cut) = (clip.path.clone(), dest.to_path_buf(), clip.cut);
    cx.editor.set_status(format!(
        "{} {}…",
        if cut { "Moving" } else { "Copying" },
        from.display()
    ));
    let (work_from, work_to) = (from.clone(), to.clone());
    run_blocking(
        cx,
        move || transfer(&work_from, &work_to, cut),
        move |editor, compositor, result| {
            match result {
                Err(err) => {
                    editor.set_error(format!(
                        "Failed to paste {} to {}: {err}",
                        from.display(),
                        to.display()
                    ));
                    return;
                }
                Ok(None) if !cut => {
                    editor.set_status(format!("Copied {} to {}", from.display(), to.display()))
                }
                Ok(None) => {
                    editor.explorer_clipboard = None;
                    retarget_buffers(editor, &from, &to);
                    editor.set_status(format!("Moved {} to {}", from.display(), to.display()));
                }
                Ok(Some(err)) => {
                    // The copy succeeded, removing the original did not: keep
                    // the clipboard entry so the cut can be retried.
                    editor.set_error(format!(
                        "Copied {} to {} but failed to remove the original: {err}",
                        from.display(),
                        to.display()
                    ));
                }
            }
            if let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) {
                explorer.refresh_reveal(editor, to);
            }
        },
    );
}

/// Runs a tree refresh after a mutation prompt's file operation finished,
/// revealing and selecting `show` (the entry that was created, renamed or
/// pasted). Callable from inside prompt callbacks, which have `jobs` but no
/// direct compositor.
fn refresh_after(cx: &mut Context, show: PathBuf) {
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
            if let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) {
                explorer.refresh_reveal(editor, show);
            }
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

impl Component for Explorer {
    fn id(&self) -> Option<&'static str> {
        Some(EXPLORER_ID)
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !cx.editor.explorer_panel.visible {
            return EventResult::Ignored(None);
        }

        // Mouse: clicks inside the panel operate the tree (select, expand,
        // open); anything else bubbles down to the editor underneath.
        if let Event::Mouse(mouse) = event {
            return self.handle_mouse(*mouse, cx);
        }

        // Pasted text goes into the filter box while it has the keyboard.
        if let Event::Paste(text) = event {
            if self.filter_editing && cx.editor.explorer_panel.focused {
                self.filter_insert(&text.replace(['\n', '\r'], " "));
                self.filter_changed(cx.editor);
                return EventResult::Consumed(None);
            }
            return EventResult::Ignored(None);
        }

        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };

        // While the editor's keymap has a partial sequence pending (e.g. the
        // `<space>` of `<space>x`), hand the key through so the editor's
        // multi-key mapping can complete.
        if !cx.editor.keymap_pending.is_empty() {
            return EventResult::Ignored(None);
        }

        if !cx.editor.explorer_panel.focused {
            return EventResult::Ignored(None);
        }

        // In insert (or other non-normal) modes the keys belong to the
        // document; the panel only drives the tree in normal mode.
        if cx.editor.mode != Mode::Normal {
            return EventResult::Ignored(None);
        }

        if self.filter_editing {
            return self.on_filter_key(*key, cx);
        }

        let result = self.on_nav_key(*key, cx);
        // A count typed before one of the tree's own keys (`3d`) applies to nothing:
        // drop it, so it doesn't carry over to the next key. Digits and movement keys
        // are not the tree's: the editor's keymap counts them (`5j`, `10gg`).
        if matches!(result, EventResult::Consumed(_)) {
            cx.editor.count = None;
        }
        result
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        let focused = editor.explorer_panel.visible && editor.explorer_panel.focused;
        if focused && !self.filter_editing {
            // The tree has the keyboard: hide the terminal cursor instead of
            // letting the editor underneath show its own, which reads as
            // focus having moved there.
            let row = self.area.y as usize + self.cursor.saturating_sub(self.scroll);
            return (
                Some(Position::new(row, self.area.x as usize)),
                CursorKind::Hidden,
            );
        }
        if !(self.filter_editing && focused && self.input_area.width > 0) {
            return (None, CursorKind::Hidden);
        }
        let prefix = FILTER_PREFIX.width();
        let avail = (self.input_area.width as usize).saturating_sub(prefix);
        let start = self.filter_window_start(avail);
        let column =
            self.input_area.x as usize + prefix + self.filter[start..self.filter_cursor].width();
        (
            Some(Position::new(self.input_area.y as usize, column)),
            editor.config().cursor_shape.from_mode(Mode::Insert),
        )
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        if !cx.editor.explorer_panel.visible {
            self.filter_editing = false;
            self.resizing = false;
            // A drag cut short by closing the window would otherwise grab the next clicks.
            self.float_drag = None;
            return;
        }
        // Only a shown panel refreshes git status: the layer stays installed while
        // hidden, and `git status` on a large repo is not free.
        self.request_git_status(cx);
        if !cx.editor.explorer_panel.focused {
            // Keys no longer reach the box once the panel lost the keyboard.
            self.filter_editing = false;
        }

        let side = cx.editor.explorer_side();
        let floating = side == ExplorerSide::Floating;
        // A drag of what the panel no longer has (it switched sides) is over.
        if floating {
            self.resizing = false;
        } else {
            self.float_drag = None;
        }
        let panel_width = cx
            .editor
            .explorer_width()
            .min(area.width.saturating_sub(MIN_EDITOR_WIDTH));

        // Mirror EditorView's framing: skip the command line's bottom row so
        // the panel does not paint over it. The pinned panel spans the full
        // height from the very top row; `EditorView` starts the buffer line
        // after the panel's column instead of above it.
        let config = cx.editor.config();
        let cmdline_popup_full_height =
            config.cmdline.style == CmdlineStyle::Popup && config.cmdline.use_full_height;
        let bottom = if cmdline_popup_full_height { 0 } else { 1 };

        let (rect, frame) = if floating {
            // Sized and centered like the file picker without its preview (the tree is a
            // list too): a share of the editor without the status line and the command
            // line, so it follows the screen.
            let bounds = area.clip_bottom(2);
            self.float_bounds = bounds;
            let frame = cx.editor.explorer_panel.floating_frame(
                &config.explorer,
                bounds,
                super::overlay::LIST_ONLY_PERCENTAGE,
            );
            // Gradient borders, when configured, set the border's thickness.
            let gradient = &cx.editor.config().gradient_borders;
            let t = if gradient.enable {
                u16::from(gradient.thickness.clamp(1, 5))
            } else {
                1
            };
            let rect = if frame.width <= 2 * t || frame.height <= 2 * t {
                frame
            } else {
                Rect::new(
                    frame.x + t,
                    frame.y + t,
                    frame.width - 2 * t,
                    frame.height - 2 * t,
                )
            };
            (rect, Some(frame))
        } else {
            let x = match side {
                ExplorerSide::Left => 0,
                ExplorerSide::Right => area.width.saturating_sub(panel_width),
                ExplorerSide::Floating => unreachable!(),
            };
            (
                Rect::new(x, 0, panel_width, area.height.saturating_sub(bottom)),
                None,
            )
        };
        if rect.width == 0 || area.width == 0 {
            return;
        }

        // A bordered filter box with its count heads the panel. A short panel makes do with a
        // line showing the folder, or the filter while there is one, with a blank row under it
        // to set it apart from the tree (no rule: the panel has its separator already).
        let boxed = rect.height >= FILTER_BOX_MIN_HEIGHT && rect.width >= 8;
        let header: u16 = if boxed {
            3
        } else if rect.height >= 6 {
            2
        } else if rect.height >= 3 {
            1
        } else {
            0
        };
        let rows_area = Rect::new(rect.x, rect.y + header, rect.width, rect.height - header);
        // Remember where things live so that mouse events can be hit-tested
        // against the panel without re-deriving the layout.
        self.panel = rect;
        self.area = rows_area;
        self.input_area = if header > 0 {
            Rect::new(rect.x, rect.y, rect.width, 1)
        } else {
            Rect::default()
        };
        self.screen_width = area.width;
        self.separator = None;
        self.frame = frame;

        // Results stream in from the scan thread while a filter is active. Taken in before
        // anything is drawn, so the filter box's count and the rows show the same results.
        if self.tree.poll_scan(tree_params(cx.editor)) {
            // Better matches may arrive as the scan proceeds; follow the top
            // one until the user picks a row themselves.
            if self.filter_editing && !self.filter_navigated {
                if let Some(row) = self.best_row(false) {
                    self.cursor = row;
                }
            }
            self.clamp_cursor();
            if !self.tree.scanning() {
                self.report_filter_limits(cx.editor);
            }
        }

        let theme = &cx.editor.theme;
        let text_style = theme.get("ui.text");
        let dir_style = theme.get("ui.text.directory");
        let sel_style = theme.get("ui.selection");
        let virtual_style = theme.get("ui.virtual");
        let border_style = explorer_border_style(theme, virtual_style);
        let tree_style = explorer_tree_style(theme, virtual_style);
        let tree_selected_style = explorer_tree_selected_style(theme, tree_style, sel_style);
        let inactive_style = theme.get("ui.text.inactive");
        let match_style = theme.get("special");
        // The panel's own scope so the background can be themed without
        // touching every other popup; falls back to `ui.popup`.
        let background_style = theme.get("ui.popup.explorer");

        match frame {
            Some(frame) => {
                // Floating: paint the backdrop, then the frame.
                // Clear, not just restyle: the editor's text underneath would
                // otherwise show through every cell this panel does not write.
                let config = cx.editor.config();
                // Only inside the border (see `clear_bordered`).
                crate::ui::clear_bordered(
                    surface,
                    frame,
                    background_style,
                    !crate::ui::border_cells_take_background(&config),
                );
                if config.gradient_borders.enable {
                    // `[editor.gradient-borders]` wins whenever it is enabled.
                    if let Some(border) = self.gradient(&config, theme) {
                        if !config.borders_hidden() {
                            border.render(frame, surface, theme, config.border_rounded());
                        }
                    }
                } else {
                    // Plain solid border otherwise.
                    self.gradient_border = None;
                    Widget::render(
                        Block::bordered()
                            .border_type(crate::ui::border_type(&config))
                            .border_style(explorer_border_style(
                                theme,
                                crate::ui::border_style(theme),
                            )),
                        frame,
                        surface,
                    );
                }
            }
            None => {
                // Pinned: paint the panel background and a separator line
                // between panel and editor.
                surface.clear_with(rect, background_style);
                let sep_x = if side == ExplorerSide::Left {
                    rect.x + rect.width
                } else {
                    rect.x.saturating_sub(1)
                };
                if sep_x < area.width {
                    self.separator = Some(sep_x);
                    for y in 0..rect.height {
                        surface[(sep_x, rect.y + y)]
                            .set_symbol("│")
                            .set_style(border_style);
                    }
                }
            }
        }

        // The path highlight: the root row, the ancestors of the cursor and the guide lines
        // that lead to it.
        let path_style = explorer_path_style(theme, sel_style);

        if boxed {
            self.render_filter_box(Rect::new(rect.x, rect.y, rect.width, 3), surface, cx.editor);
        } else if header > 0 && !self.filter_editing && self.filter.is_empty() {
            // The folder the tree starts in, until there is a filter to show.
            let label = root_label(&self.root, self.input_area.width as usize);
            // The root is always on the path to the cursor.
            for x in 0..self.input_area.width {
                surface[(self.input_area.x + x, self.input_area.y)].set_style(path_style);
            }
            surface.set_stringn(
                self.input_area.x,
                self.input_area.y,
                &label,
                self.input_area.width as usize,
                dir_style,
            );
        } else if header > 0 {
            let input = self.input_area;
            let editing = self.filter_editing;
            if editing {
                surface.set_style(input, theme.get("ui.menu"));
            }
            let prefix_style = if editing {
                theme.get("ui.text.focus")
            } else {
                virtual_style
            };
            surface.set_stringn(
                input.x,
                input.y,
                FILTER_PREFIX,
                input.width as usize,
                prefix_style,
            );
            let prefix_width = FILTER_PREFIX.width() as u16;
            let avail = input.width.saturating_sub(prefix_width) as usize;
            if avail > 0 && !self.filter.is_empty() {
                let start = self.filter_window_start(avail);
                let style = if editing {
                    theme.get("ui.menu").patch(text_style)
                } else {
                    text_style
                };
                surface.set_stringn(
                    input.x + prefix_width,
                    input.y,
                    &self.filter[start..],
                    avail,
                    style,
                );
            }
        }

        // Scroll bookkeeping: keep the cursor inside the rendered slice.
        let rows = rows_area.height as usize;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if rows > 0 && self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
        self.clamp_cursor();

        let icons = ICONS.load();
        let mime = icons.mime();
        let show_icons = cx.editor.config().explorer.icons;
        let guides = cx.editor.config().explorer.indent_guides;
        let show_git = cx.editor.config().explorer.git_status;
        let path = PathInfo::new(self.tree.visible(), self.cursor);
        let matcher = &mut self.matcher;
        let mut indices: Vec<u32> = Vec::new();

        for (i, entry) in self
            .tree
            .visible()
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(rows)
        {
            let y = rows_area.y + (i - self.scroll) as u16;
            if y >= surface.area.bottom() {
                break;
            }

            // Highlight the selected row across the full panel width.
            if i == self.cursor {
                for x in 0..rows_area.width {
                    surface[(rows_area.x + x, y)].set_style(sel_style);
                }
            } else if path.is_path_row(i) {
                for x in 0..rows_area.width {
                    surface[(rows_area.x + x, y)].set_style(path_style);
                }
            }

            let highlight = match self.tree.filter() {
                Some(filter) if filter.is_fuzzy() => {
                    let relative = entry
                        .path
                        .strip_prefix(self.tree.root())
                        .unwrap_or(&entry.path)
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    filter.match_indices(&entry.name, &relative, matcher, &mut indices);
                    !indices.is_empty() && !entry.is_parent
                }
                _ => false,
            };
            let name_spans = |base: Style| -> Vec<Span<'static>> {
                if highlight {
                    highlighted_name(&entry.name, &indices, base, match_style)
                } else {
                    vec![Span::styled(entry.name.clone(), base)]
                }
            };

            let mut spans: Vec<Span> = Vec::with_capacity(3);
            // The guide prefix is one 3-column block per ancestor level, then the branch of
            // the entry itself. Only the path rows (the ancestors and the cursor) are
            // highlighted: sibling rows keep their guide lines even where they share one.
            let prefix = Self::prefix(&self.tree, entry, guides);
            if guides {
                // The guides of the selected row are `ui.explorer.tree.selected`, over the
                // selection: the selected row is one highlight, not the path's.
                let style = if i == self.cursor {
                    tree_selected_style
                } else if path.is_path_row(i) {
                    path_style
                } else {
                    tree_style
                };
                spans.push(Span::styled(prefix, style));
            } else {
                spans.push(Span::styled(prefix, tree_style));
            }
            if entry.is_dir {
                // Expanded and collapsed folders get their own glyphs.
                let glyph = if !show_icons {
                    None
                } else if entry.is_empty {
                    mime.directory_empty()
                } else if entry.expanded {
                    mime.directory_open()
                } else {
                    mime.directory()
                };
                if let Some(glyph) = glyph {
                    spans.push(Span::styled(format!("{glyph} "), dir_style));
                }
                spans.extend(name_spans(dir_style));
            } else if let Some(icon) = mime
                .get(Some(&entry.path), Some(entry.name.as_str()))
                .filter(|_| show_icons)
            {
                let mut style = Style::default().fg(Color::Reset);
                if let Some(color) = icon.color() {
                    style = style.fg(color);
                }
                spans.push(Span::styled(format!("{} ", icon.glyph()), style));
                spans.extend(name_spans(text_style));
            } else {
                spans.extend(name_spans(text_style));
            }

            // Changed files are marked at the right edge, outside of what the name may take.
            let marker = if !show_git || entry.is_parent || entry.depth == 0 {
                None
            } else if entry.is_dir {
                self.git
                    .folder(&entry.path)
                    .map(|kind| ("•".to_string(), marker_style(&cx.editor.theme, kind)))
            } else {
                self.git
                    .file(&entry.path)
                    .map(|kind| (change_marker(kind), marker_style(&cx.editor.theme, kind)))
            }
            // Not for a panel so narrow that the marker would be all there is.
            .filter(|(text, _)| rows_area.width as usize > text.width() + 6);
            let name_width = match &marker {
                Some((text, _)) => rows_area.width as usize - text.width() - 2,
                None => rows_area.width as usize,
            };

            let mut x = rows_area.x;
            for span in truncate_spans(spans, name_width) {
                surface.set_string(x, y, &span.content, span.style);
                x += span.content.width() as u16;
            }
            if let Some((text, style)) = marker {
                let marker_x = rows_area.right() - text.width() as u16 - 1;
                surface.set_string(marker_x, y, &text, style);
            }
        }

        // A filter that matched nothing leaves only the root row.
        if self.tree.filter().is_some() && self.len() <= 1 && rows > 1 {
            let hint = if self.tree.scanning() {
                "searching…"
            } else {
                "no matches"
            };
            surface.set_stringn(
                rows_area.x + 1,
                rows_area.y + 1,
                hint,
                rows_area.width.saturating_sub(1) as usize,
                inactive_style,
            );
        }
    }
}

/// Style of the explorer's borders: the separator of a pinned panel and the
/// frame of the floating window (not the tree guide lines, see
/// `ui.explorer.tree`). Without `ui.explorer.border`, `fallback` is used.
fn explorer_border_style(
    theme: &helix_view::Theme,
    fallback: helix_view::graphics::Style,
) -> helix_view::graphics::Style {
    theme.try_get("ui.explorer.border").unwrap_or(fallback)
}

/// Style of the path to the cursor: the root row, the ancestors of the cursor and the
/// guide lines that lead to it. `ui.explorer.path`; without it, the primary cursor line,
/// so the selected row (the selection highlight) is the only strongly highlighted row.
/// The style of a git status marker: only the foreground of the change's `diff.*` style.
/// Many themes give those a background for diff views, which would paint a block behind
/// the marker instead of keeping the row's own background (and the selection highlight).
fn marker_style(theme: &helix_view::Theme, kind: ChangeKind) -> Style {
    let style = change_style(theme, kind);
    match style.fg {
        Some(fg) => Style::default().fg(fg),
        None => Style::default(),
    }
}

fn explorer_path_style(
    theme: &helix_view::Theme,
    selection: helix_view::graphics::Style,
) -> helix_view::graphics::Style {
    theme
        .try_get("ui.explorer.path")
        .or_else(|| theme.try_get("ui.cursorline.primary"))
        .unwrap_or(selection)
}

/// The rows on the path from the top of the tree down to the cursor.
struct PathInfo {
    /// Index of the ancestor at each depth, the cursor's own row last.
    ancestors: Vec<usize>,
}

impl PathInfo {
    fn new(entries: &[Entry], cursor: usize) -> Self {
        let Some(current) = entries.get(cursor) else {
            return Self {
                ancestors: Vec::new(),
            };
        };
        let depth = current.depth as usize;
        let mut ancestors = vec![cursor; depth + 1];
        let mut level = depth;
        for (index, entry) in entries[..cursor].iter().enumerate().rev() {
            if level == 0 {
                break;
            }
            let entry_depth = entry.depth as usize;
            if entry_depth < level {
                level = entry_depth;
                ancestors[level] = index;
            }
        }
        Self { ancestors }
    }

    /// Whether row `index` is the cursor's row or one of its ancestors.
    fn is_path_row(&self, index: usize) -> bool {
        self.ancestors.contains(&index)
    }
}

/// Style of the tree guide lines (`├─ └─ │`) on normal rows. `ui.explorer.tree`;
/// without it, `fallback` (`ui.virtual`).
fn explorer_tree_style(
    theme: &helix_view::Theme,
    fallback: helix_view::graphics::Style,
) -> helix_view::graphics::Style {
    theme.try_get("ui.explorer.tree").unwrap_or(fallback)
}

/// Style of the tree guide lines on the selected row. `ui.explorer.tree.selected`;
/// without it, the normal guide style over the selection highlight.
fn explorer_tree_selected_style(
    theme: &helix_view::Theme,
    normal: helix_view::graphics::Style,
    selection: helix_view::graphics::Style,
) -> helix_view::graphics::Style {
    theme
        .try_get("ui.explorer.tree.selected")
        .unwrap_or_else(|| normal.patch(selection))
}

/// The pointer shown over a part of the floating window's border.
fn float_pointer(drag: FloatDrag) -> PointerShape {
    match drag {
        FloatDrag::Move => PointerShape::Move,
        FloatDrag::Resize {
            left,
            right,
            top,
            bottom,
        } => match (left, right, top, bottom) {
            (true, _, true, _) | (_, true, _, true) => PointerShape::NwseResize,
            (true, _, _, true) | (_, true, true, _) => PointerShape::NeswResize,
            (true, ..) | (_, true, ..) => PointerShape::ColResize,
            _ => PointerShape::RowResize,
        },
    }
}

/// Splits `name` into spans, styling the characters at `indices` (sorted
/// character positions) with `matched` on top of `base`.
fn highlighted_name(
    name: &str,
    indices: &[u32],
    base: Style,
    matched: Style,
) -> Vec<Span<'static>> {
    let hit_style = base.patch(matched);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_hit = false;
    let mut next = indices.iter().copied().peekable();
    for (i, ch) in name.chars().enumerate() {
        let hit = next.peek() == Some(&(i as u32));
        if hit {
            next.next();
        }
        if hit != run_hit && !run.is_empty() {
            let style = if run_hit { hit_style } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_hit = hit;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_hit { hit_style } else { base }));
    }
    spans
}

/// Trims a styled line to at most `width` display columns. A line that is cut ends in `…`.
fn truncate_spans(spans: Vec<Span>, width: usize) -> Vec<Span> {
    if width == 0 {
        return Vec::new();
    }
    let total: usize = spans.iter().map(|span| span.content.width()).sum();
    if total <= width {
        return spans;
    }

    // One column is kept for the ellipsis.
    let budget = width - 1;
    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut used = 0usize;
    let mut last_style = Style::default();
    for span in spans {
        if used >= budget {
            break;
        }
        let mut left = budget - used;
        let mut taken = String::new();
        for grapheme in span.content.graphemes(true) {
            let w = grapheme.width();
            if w > left {
                break;
            }
            left -= w;
            taken.push_str(grapheme);
        }
        used += taken.width();
        last_style = span.style;
        out.push(Span::styled(taken, span.style));
    }
    out.push(Span::styled("…", last_style));
    out
}

/// The folder the panel shows in its top row: relative to the home folder when it is in it, and
/// cut at the start (`…/src/ui`) when it does not fit `width` columns.
fn root_label(root: &Path, width: usize) -> String {
    let label = helix_stdx::path::fold_home_dir(root).display().to_string();
    if label.width() <= width {
        return label;
    }
    // The end is what tells folders apart.
    let mut tail: Vec<&str> = Vec::new();
    let mut used = 1; // the ellipsis
    for grapheme in label.graphemes(true).rev() {
        let w = grapheme.width();
        if used + w > width {
            break;
        }
        used += w;
        tail.push(grapheme);
    }
    tail.reverse();
    format!("…{}", tail.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rename to another case of the same name is allowed where the file system ignores
    /// case; elsewhere, and for any other existing entry, the target is taken.
    #[test]
    fn only_the_entry_itself_is_the_same_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (a, b) = (tmp.path().join("a.txt"), tmp.path().join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert!(same_entry(&a, &a));
        assert!(same_entry(&a, &tmp.path().join("./a.txt")));
        assert!(!same_entry(&a, &b));
        assert!(!same_entry(&a, &tmp.path().join("missing")));
        // a link is an entry of its own
        #[cfg(unix)]
        {
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&a, &link).unwrap();
            assert!(!same_entry(&link, &a));
        }
        // where case doesn't matter, `A.txt` is `a.txt`
        let upper = tmp.path().join("A.txt");
        if upper.exists() {
            assert!(same_entry(&a, &upper));
        }
    }

    #[test]
    fn slow_git_status_is_asked_for_less_often() {
        use std::time::Duration;
        assert_eq!(
            git_status_interval(Duration::from_millis(20)),
            GIT_STATUS_INTERVAL
        );
        assert_eq!(
            git_status_interval(Duration::from_secs(1)),
            Duration::from_secs(10)
        );
        assert_eq!(
            git_status_interval(Duration::from_secs(60)),
            GIT_STATUS_RETRY
        );
    }

    #[test]
    fn git_markers_keep_the_row_background() {
        use helix_view::graphics::Color;
        // A theme that gives `diff.plus` a background, as diff-oriented themes do.
        let theme = helix_view::Theme::from(
            toml::from_str::<toml::Value>(r##""diff.plus" = { fg = "#00ff00", bg = "#003300" }"##)
                .unwrap(),
        );
        let style = marker_style(&theme, ChangeKind::Added);
        assert_eq!(style.fg, Some(Color::Rgb(0, 255, 0)));
        assert_eq!(style.bg, None);
    }

    #[test]
    fn matched_characters_are_split_into_spans() {
        let base = Style::default();
        let hit = Style::default().fg(Color::Red);
        let spans = highlighted_name("main.rs", &[0, 1, 6], base, hit);
        let parts: Vec<_> = spans
            .iter()
            .map(|span| (span.content.to_string(), span.style == hit))
            .collect();
        assert_eq!(
            parts,
            vec![
                ("ma".to_string(), true),
                ("in.r".to_string(), false),
                ("s".to_string(), true),
            ]
        );
        assert_eq!(highlighted_name("a", &[], base, hit).len(), 1);
    }

    #[test]
    fn long_lines_end_in_an_ellipsis() {
        let line = |text: &str| vec![Span::raw("  "), Span::raw(text.to_string())];
        let text = |spans: Vec<Span>| {
            spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(truncate_spans(line("short"), 20)), "  short");
        assert_eq!(
            text(truncate_spans(line("json_unmarshal"), 10)),
            "  json_un…"
        );
        // exactly fitting is not cut
        assert_eq!(text(truncate_spans(line("12345678"), 10)), "  12345678");
        assert_eq!(text(truncate_spans(line("abc"), 1)), "…");
        assert!(truncate_spans(line("abc"), 0).is_empty());
    }

    #[test]
    fn the_root_is_cut_at_the_start() {
        let root = Path::new("/very/long/folder/name");
        assert_eq!(root_label(root, 40), "/very/long/folder/name");
        assert_eq!(root_label(root, 12), "…folder/name");
        assert_eq!(root_label(root, 1), "…");
    }

    #[test]
    fn relative_paths_are_validated() {
        assert_eq!(
            validate_relative_path("a/b.txt").unwrap(),
            PathBuf::from("a/b.txt")
        );
        assert!(validate_relative_path("../x").is_err());
        assert!(validate_relative_path("a/../x").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("./x").is_err());
        assert!(validate_relative_path("").is_err());
    }

    #[test]
    fn rename_targets_are_single_names() {
        assert_eq!(validate_file_name("new.rs").unwrap(), "new.rs");
        assert!(validate_file_name("a/b").is_err());
        assert!(validate_file_name("..").is_err());
        assert!(validate_file_name("/abs").is_err());
        assert!(validate_file_name("").is_err());
    }

    #[test]
    fn remove_path_handles_files_trees_and_symlinks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tree/inner")).unwrap();
        std::fs::write(root.join("tree/inner/f.txt"), "f").unwrap();
        std::fs::write(root.join("file.txt"), "f").unwrap();

        remove_path(&root.join("file.txt")).unwrap();
        assert!(!root.join("file.txt").exists());

        remove_path(&root.join("tree")).unwrap();
        assert!(!root.join("tree").exists());

        #[cfg(unix)]
        {
            // A symlink to a directory is unlinked; its target survives.
            std::fs::create_dir(root.join("target")).unwrap();
            std::fs::write(root.join("target/keep.txt"), "k").unwrap();
            std::os::unix::fs::symlink(root.join("target"), root.join("link")).unwrap();
            remove_path(&root.join("link")).unwrap();
            assert!(!root.join("link").exists());
            assert!(root.join("target/keep.txt").exists());
        }
    }
}
