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
    editor::{Action, BufferLineRenderMode, CmdlineStyle},
    explorer::{
        copy_recursively, Entry, ExplorerClipboard, ExplorerSide, Tree, TreeParams,
        FILTER_SCAN_LIMIT,
    },
    graphics::{Color, Rect, Style},
    Editor,
};
use tui::buffer::Buffer as Surface;
use tui::text::Span;
use tui::widgets::{Block, Widget};

use crate::{
    compositor::{Component, Context, Event, EventResult},
    job::Callback,
    ui::{completers, CmdlinePopup, ConfirmBox, PromptEvent},
};

pub const EXPLORER_ID: &str = "explorer";

/// Minimum editor width the panel always leaves for views, even on narrow
/// terminals.
const MIN_EDITOR_WIDTH: u16 = 8;
/// A one-column separator line between the panel and the editor.
const SEPARATOR_WIDTH: u16 = 1;
/// Narrowest the panel can be dragged to.
const MIN_PANEL_WIDTH: u16 = 4;
/// Drawn in front of the filter input (it is focused with `/`).
const FILTER_PREFIX: &str = "/ ";
/// Greyed hint shown in the empty filter box.
const FILTER_PLACEHOLDER: &str = "filter: name or *.rs";

/// `TreeParams` as seen from the current editor state; what the tree ops and
/// the renderer both use.
pub fn tree_params(editor: &Editor) -> TreeParams {
    let config = editor.config();
    TreeParams::new(&config.explorer, &editor.explorer_panel)
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
    /// Text of the filter box.
    filter: String,
    /// Byte offset of the filter box's cursor.
    filter_cursor: usize,
    /// Keys edit the filter text instead of driving the tree.
    filter_editing: bool,
}

impl Explorer {
    /// Creates the panel rooted at `root`, loading the root's children only.
    pub fn new(root: &Path, editor: &Editor) -> io::Result<Self> {
        let tree = Tree::new(root.to_path_buf(), tree_params(editor))?;
        Ok(Self {
            root: root.to_path_buf(),
            tree,
            cursor: 0,
            scroll: 0,
            area: Rect::default(),
            input_area: Rect::default(),
            panel: Rect::default(),
            separator: None,
            screen_width: 0,
            resizing: false,
            filter: String::new(),
            filter_cursor: 0,
            filter_editing: false,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn len(&self) -> usize {
        self.tree.visible().len()
    }

    /// The row under the cursor (if the tree is non-empty).
    fn entry(&self) -> Option<&Entry> {
        self.tree.visible().get(self.cursor)
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
        self.tree.visible().get(self.cursor).cloned()
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
        self.clamp_cursor();
        if let Some(keep) = keep {
            self.select_path(&keep);
        }
    }

    /// Like [`Self::refresh_keep`] for a path that was just created or moved:
    /// its ancestors are expanded so the entry is actually on screen.
    pub fn refresh_reveal(&mut self, editor: &Editor, path: PathBuf) {
        self.tree.refresh(tree_params(editor));
        if !self.reveal_path(&path, editor) {
            self.clamp_cursor();
            self.select_path(&path);
        }
    }

    // --------------------------------------------------------------------
    // Filter box
    // --------------------------------------------------------------------

    /// Applies the filter box text to the tree. The selection stays on its
    /// entry when that is still listed; otherwise it moves to the first match.
    fn filter_changed(&mut self, editor: &mut Editor) {
        let params = tree_params(editor);
        let keep = self.entry().map(|entry| entry.path.clone());
        self.tree.set_filter(&self.filter, params);
        if self.tree.filter_truncated() {
            editor.set_status(format!(
                "Explorer: filter searched only the first {FILTER_SCAN_LIMIT} entries"
            ));
        }

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
            // Done editing, the filter stays applied.
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab
                if key.modifiers == KeyModifiers::NONE =>
            {
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
            KeyCode::Left if plain => self.filter_cursor = self.filter_prev_boundary(),
            KeyCode::Right if plain => self.filter_cursor = self.filter_next_boundary(),
            KeyCode::Home if plain => self.filter_cursor = 0,
            KeyCode::End if plain => self.filter_cursor = self.filter.len(),
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

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.cursor + 1 < self.len() {
                    self.cursor += 1;
                }
                EventResult::Consumed(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                EventResult::Consumed(None)
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.cursor = 0;
                EventResult::Consumed(None)
            }
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
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_to(mouse.column, cx),
                MouseEventKind::Up(_) => self.resizing = false,
                _ => {}
            }
            return EventResult::Consumed(None);
        }

        let panel = self.panel;
        let on_separator = self.separator == Some(mouse.column)
            && mouse.row >= panel.y
            && mouse.row < panel.bottom();
        if on_separator {
            return match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
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

        let area = self.area;
        // The filter box (and the rule below it).
        if mouse.row < area.y {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && mouse.row == self.input_area.y
                && self.input_area.width > 0
            {
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
                if dest.exists() {
                    cx.editor
                        .set_error(format!("Cannot rename: {} already exists", dest.display()));
                    return;
                }
                match std::fs::rename(&from, &dest) {
                    Ok(()) => cx.editor.set_status(format!(
                        "Renamed {} to {}",
                        from.display(),
                        dest.display()
                    )),
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
            move |compositor, cx| {
                match remove_path(&path) {
                    Ok(()) => cx.editor.set_status(format!("Deleted {}", path.display())),
                    Err(err) => cx
                        .editor
                        .set_error(format!("Failed to delete {}: {err}", path.display())),
                }
                if let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) {
                    explorer.refresh_keep(cx.editor, Some(keep));
                }
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
        if dest.starts_with(&clip.path) {
            cx.editor
                .set_error("Cannot paste a path into its own subtree");
            return EventResult::Consumed(None);
        }

        if dest.exists() {
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

    /// Guide characters for one row: `│  ` per ancestor, then `├─ `/`└─ `.
    fn prefix(&self, entry: &Entry) -> String {
        if entry.depth == 0 {
            return String::new();
        }
        // Every ancestor directory between root and entry (excluding the
        // root itself), nearest last.
        let mut ancestors: Vec<&Path> = Vec::with_capacity(entry.depth as usize);
        let mut cur = entry.path.parent();
        while let Some(parent) = cur {
            if self.tree.is_root(parent) {
                break;
            }
            ancestors.push(parent);
            cur = parent.parent();
        }
        let mut prefix = String::with_capacity((entry.depth as usize + 1) * 3);
        for ancestor in ancestors.iter().rev() {
            if self.tree.is_last_child(ancestor) {
                prefix.push_str("   "); // ancestor branch ended here
            } else {
                prefix.push_str("│  "); // vertical guide continues
            }
        }
        prefix.push_str(if entry.is_last {
            "└─ " // └─
        } else {
            "├─ " // ├─
        });
        prefix
    }
}

/// Paste implementation shared by plain paste and overwrite confirmation.
fn paste_now(cx: &mut Context, clip: &ExplorerClipboard, dest: &Path) {
    if let Err(err) = copy_recursively(&clip.path, dest) {
        cx.editor.set_error(format!(
            "Failed to paste {} to {}: {err}",
            clip.path.display(),
            dest.display()
        ));
        return;
    }

    if !clip.cut {
        cx.editor.set_status(format!(
            "Copied {} to {}",
            clip.path.display(),
            dest.display()
        ));
        refresh_after(cx, dest.to_path_buf());
        return;
    }

    match remove_path(&clip.path) {
        Ok(()) => {
            cx.editor.explorer_clipboard = None;
            cx.editor.set_status(format!(
                "Moved {} to {}",
                clip.path.display(),
                dest.display()
            ));
        }
        Err(err) => {
            // The copy succeeded, removing the original did not: keep the
            // clipboard entry so the user can retry the cut.
            cx.editor.set_error(format!(
                "Copied {} to {} but failed to remove the original: {err}",
                clip.path.display(),
                dest.display()
            ));
        }
    }
    refresh_after(cx, dest.to_path_buf());
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

        self.on_nav_key(*key, cx)
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        if !(self.filter_editing
            && editor.explorer_panel.visible
            && editor.explorer_panel.focused
            && self.input_area.width > 0)
        {
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
            return;
        }
        if !cx.editor.explorer_panel.focused {
            // Keys no longer reach the box once the panel lost the keyboard.
            self.filter_editing = false;
        }

        let side = cx.editor.explorer_side();
        let floating = side == ExplorerSide::Floating;
        let panel_width = cx
            .editor
            .explorer_width()
            .min(area.width.saturating_sub(MIN_EDITOR_WIDTH));

        // Mirror EditorView's framing: skip the command line's bottom row and
        // (when the buffer line is on) the top row, so the panel does not
        // paint over them.
        let config = cx.editor.config();
        let top = if matches!(config.bufferline.render_mode, BufferLineRenderMode::Always)
            || (config.bufferline.render_mode == BufferLineRenderMode::Multiple
                && cx.editor.documents.len() > 1)
        {
            1
        } else {
            0
        };
        let cmdline_popup_full_height =
            config.cmdline.style == CmdlineStyle::Popup && config.cmdline.use_full_height;
        let bottom = if cmdline_popup_full_height { 0 } else { 1 };

        let (rect, frame) = if floating {
            let config = cx.editor.config();
            let frame_width = config
                .explorer
                .floating_width
                .min(area.width.saturating_sub(2));
            let frame_height = config
                .explorer
                .floating_height
                .min(area.height.saturating_sub(2));
            let frame = Rect::new(
                area.x + (area.width.saturating_sub(frame_width)) / 2,
                area.y + (area.height.saturating_sub(frame_height)) / 2,
                frame_width,
                frame_height,
            );
            let rect = if frame.width < 3 || frame.height < 3 {
                frame
            } else {
                Rect::new(frame.x + 1, frame.y + 1, frame.width - 2, frame.height - 2)
            };
            (rect, Some(frame))
        } else {
            let x = match side {
                ExplorerSide::Left => 0,
                ExplorerSide::Right => area.width.saturating_sub(panel_width),
                ExplorerSide::Floating => unreachable!(),
            };
            (
                Rect::new(
                    x,
                    top,
                    panel_width,
                    area.height.saturating_sub(top.saturating_add(bottom)),
                ),
                None,
            )
        };
        if rect.width == 0 || area.width == 0 {
            return;
        }

        // The filter box takes the top row, with a rule under it when there is
        // room for both.
        let header: u16 = if rect.height >= 5 {
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

        let theme = &cx.editor.theme;
        let text_style = theme.get("ui.text");
        let dir_style = theme.get("ui.text.directory");
        let sel_style = theme.get("ui.selection");
        let virtual_style = theme.get("ui.virtual");
        let inactive_style = theme.get("ui.text.inactive");
        // The panel's own scope so the background can be themed without
        // touching every other popup; falls back to `ui.popup`.
        let background_style = theme.get("ui.popup.explorer");

        match frame {
            Some(frame) => {
                // Floating: paint the backdrop, then the frame.
                surface.set_style(frame, background_style);
                let block = Block::bordered();
                Widget::render(block, frame, surface);
            }
            None => {
                // Pinned: paint the panel background and a separator line
                // between panel and editor.
                surface.set_style(rect, background_style);
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
                            .set_style(virtual_style);
                    }
                }
            }
        }

        // Filter box.
        if header > 0 {
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
            if avail > 0 {
                if self.filter.is_empty() {
                    if !editing {
                        surface.set_stringn(
                            input.x + prefix_width,
                            input.y,
                            FILTER_PLACEHOLDER,
                            avail,
                            inactive_style,
                        );
                    }
                } else {
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
            if header == 2 {
                for x in 0..rect.width {
                    surface[(rect.x + x, rect.y + 1)]
                        .set_symbol("─")
                        .set_style(virtual_style);
                }
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
            }

            let mut spans: Vec<Span> = Vec::with_capacity(3);
            spans.push(Span::styled(self.prefix(entry), virtual_style));
            if entry.is_dir {
                // Expanded and collapsed folders get their own glyphs.
                let glyph = if !show_icons {
                    None
                } else if entry.expanded {
                    mime.directory_open()
                } else {
                    mime.directory()
                };
                if let Some(glyph) = glyph {
                    spans.push(Span::styled(format!("{glyph} "), dir_style));
                }
                spans.push(Span::styled(entry.name.clone(), dir_style));
            } else if let Some(icon) = mime
                .get(Some(&entry.path), Some(entry.name.as_str()))
                .filter(|_| show_icons)
            {
                let mut style = Style::default().fg(Color::Reset);
                if let Some(color) = icon.color() {
                    style = style.fg(color);
                }
                spans.push(Span::styled(format!("{} ", icon.glyph()), style));
                spans.push(Span::styled(entry.name.clone(), text_style));
            } else {
                spans.push(Span::styled(entry.name.clone(), text_style));
            }

            let mut x = rows_area.x;
            for span in truncate_spans(spans, rows_area.width as usize) {
                surface.set_string(x, y, &span.content, span.style);
                x += span.content.width() as u16;
            }
        }

        // A filter that matched nothing leaves only the root row.
        if self.tree.filter().is_some() && self.len() <= 1 && rows > 1 {
            surface.set_stringn(
                rows_area.x + 1,
                rows_area.y + 1,
                "no matches",
                rows_area.width.saturating_sub(1) as usize,
                inactive_style,
            );
        }
    }
}

/// Trims a styled line to at most `width` display columns.
fn truncate_spans(spans: Vec<Span>, width: usize) -> Vec<Span> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for span in spans {
        if used >= width {
            break;
        }
        let mut budget = width - used;
        let mut taken = String::new();
        for grapheme in span.content.graphemes(true) {
            let w = grapheme.width();
            if w > budget {
                break;
            }
            budget -= w;
            taken.push_str(grapheme);
        }
        used += taken.width();
        out.push(Span::styled(taken, span.style));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
