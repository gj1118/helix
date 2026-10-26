//! The explorer file tree panel (`[editor.explorer]`, commands
//! `explorer-*`, default key `<space>x`).
//!
//! This is the UI half of the feature; the tree model, configuration, and
//! runtime state live in `helix-view` (`helix-view/src/explorer.rs`).
//!
//! The [`Explorer`] component is installed once and then never removed:
//! toggling the panel only flips `editor.explorer_panel.visible`, so the
//! loaded tree survives hide/show cycles without any fs work. File
//! mutations (create/rename/delete/cut/copy/paste) are prompt-driven,
//! mirroring the `file_explorer` picker's prompt helpers.
//!
//! Key routing: while the panel is visible *and* focused, keys drive the
//! tree; unbound keys (and all keys while an editor keymap sequence like
//! `<space>` is pending) fall through to the editor, so multi-key editor
//! bindings keep working even with the panel focused.

use std::error::Error as _;
use std::io;
use std::path::{Path, PathBuf};

use helix_core::unicode::segmentation::UnicodeSegmentation;
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::icons::ICONS;
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
use helix_view::{
    document::Mode,
    editor::{Action, BufferLineRenderMode, CmdlineStyle},
    explorer::{copy_recursively, Entry, ExplorerClipboard, ExplorerSide, Tree, TreeParams},
    graphics::{Color, Rect, Style},
    Editor,
};
use tui::buffer::Buffer as Surface;
use tui::text::Span;
use tui::widgets::{Block, Widget};

use crate::{
    compositor::{Component, Context, Event, EventResult},
    job::Callback,
    ui::{completers, Prompt, PromptEvent},
};

pub const EXPLORER_ID: &str = "explorer";

/// Minimum editor width the panel always leaves for views, even on narrow
/// terminals.
const MIN_EDITOR_WIDTH: u16 = 8;
/// A one-column separator line between the panel and the editor.
const SEPARATOR_WIDTH: u16 = 1;

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

pub struct Explorer {
    root: PathBuf,
    tree: Tree,
    /// Index into the visible row list.
    cursor: usize,
    /// First rendered row (scroll offset), adjusted during render.
    scroll: usize,
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
        if self.tree.reveal(&path, tree_params(editor)) {
            self.select_path(&path);
            true
        } else {
            false
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
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => self.on_primary_open(cx),
            KeyCode::Char('v') => self.on_open_split(cx, Action::VerticalSplit),
            KeyCode::Char('s') => self.on_open_split(cx, Action::HorizontalSplit),
            KeyCode::Char('.') => self.on_reveal(cx),
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
            KeyCode::Char('a') => self.on_create(cx),
            KeyCode::Char('r') => self.on_rename(cx),
            KeyCode::Char('d') => self.on_delete(cx),
            KeyCode::Char('x') => self.on_cut(cx),
            KeyCode::Char('c') => self.on_copy(cx),
            KeyCode::Char('p') => self.on_paste(cx),
            KeyCode::Char('q') => {
                cx.editor.explorer_panel.visible = false;
                cx.editor.explorer_panel.focused = false;
                EventResult::Consumed(None)
            }
            KeyCode::Esc => {
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
                    return EventResult::Consumed(None);
                }
            }
        }

        match cx.editor.open(&path, action) {
            Ok(_) => {
                // Hand keyboard focus back to the editor; the tree stays open.
                cx.editor.explorer_panel.focused = false;
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

    /// `.`: reveal and select the focused document's file.
    fn on_reveal(&mut self, cx: &mut Context) -> EventResult {
        let path = helix_view::doc!(cx.editor).path().map(Path::to_path_buf);
        let Some(path) = path else {
            cx.editor.set_error("Current buffer has no file");
            return EventResult::Consumed(None);
        };
        if self.tree.reveal(&path, tree_params(cx.editor)) {
            self.select_path(&path);
        } else {
            cx.editor.set_error(format!(
                "{} is not inside the explorer root",
                path.display()
            ));
        }
        EventResult::Consumed(None)
    }

    /// Pushes a prompt via the jobs system (the compositor is not directly
    /// reachable from here), mirroring `file_explorer`'s prompt helpers.
    fn with_prompt<F>(&self, cx: &mut Context, make: F)
    where
        F: FnOnce(&mut Editor) -> Prompt + Send + 'static,
    {
        let callback = Box::pin(async move {
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                compositor.push(Box::new(make(editor)));
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
    /// The directory that subtree mutations (create/paste) act on: the
    /// selected directory itself, or the parent of the selected file.
    fn mutation_target(&self) -> Option<PathBuf> {
        let entry = self.entry()?;
        Some(if entry.is_dir {
            entry.path.clone()
        } else {
            entry
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone())
        })
    }

    fn on_create(&self, cx: &mut Context) -> EventResult {
        let Some(target) = self.mutation_target() else {
            return EventResult::Consumed(None);
        };
        self.with_prompt(cx, |_editor| {
            Prompt::new(
                format!(
                    "Add file/directory below {} (append '/' for a directory): ",
                    target.display()
                )
                .into(),
                None,
                completers::none,
                move |cx, input: &str, event: PromptEvent| {
                    if event != PromptEvent::Validate || input.is_empty() {
                        return;
                    }
                    let dest = if input.ends_with('/') {
                        target.join(input.trim_end_matches('/'))
                    } else {
                        target.join(input)
                    };
                    if dest.exists() {
                        cx.editor
                            .set_error(format!("{} already exists", dest.display()));
                    } else if input.ends_with('/') {
                        match std::fs::create_dir_all(&dest) {
                            Ok(()) => cx
                                .editor
                                .set_status(format!("Created directory {}", dest.display())),
                            Err(err) => cx
                                .editor
                                .set_error(format!("Failed to create {}: {err}", dest.display())),
                        }
                    } else {
                        match std::fs::File::create(&dest) {
                            Ok(_) => cx
                                .editor
                                .set_status(format!("Created file {}", dest.display())),
                            Err(err) => cx
                                .editor
                                .set_error(format!("Failed to create {}: {err}", dest.display())),
                        }
                    }
                    let keep = dest.clone();
                    refresh_after(cx, keep);
                },
            )
        });
        EventResult::Consumed(None)
    }

    fn on_rename(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        let from = entry.path.clone();
        let parent_dir = from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let from_display = from.display().to_string();
        let prefill = from
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.with_prompt(cx, move |editor| {
            let mut prompt = Prompt::new(
                format!("Rename {} to: ", from_display).into(),
                None,
                completers::none,
                move |cx, input: &str, event: PromptEvent| {
                    if event != PromptEvent::Validate || input.is_empty() {
                        return;
                    }
                    let dest = parent_dir.join(input);
                    if dest == from {
                        return;
                    }
                    if dest.exists() {
                        cx.editor
                            .set_error(format!("Cannot rename: {} already exists", dest.display()));
                        return;
                    }
                    match std::fs::rename(&from, &dest) {
                        Ok(()) => {
                            cx.editor.set_status(format!(
                                "Renamed {} to {}",
                                from.display(),
                                dest.display()
                            ));
                        }
                        Err(err) => {
                            cx.editor.set_error(format!(
                                "Failed to rename {} to {}: {err}",
                                from.display(),
                                dest.display()
                            ));
                        }
                    }
                    let keep = dest.clone();
                    refresh_after(cx, keep);
                },
            );
            prompt.set_line(prefill, editor);
            prompt
        });
        EventResult::Consumed(None)
    }

    fn on_delete(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        let path = entry.path.clone();
        let kind = if entry.is_dir { "directory" } else { "file" };
        let keep = entry
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let question = format!("This will remove {kind} {}. y/n? ", path.display());
        self.with_prompt(cx, move |_editor| {
            Prompt::new(
                question.into(),
                None,
                completers::none,
                move |cx, input: &str, event: PromptEvent| {
                    if event != PromptEvent::Validate || input != "y" {
                        return;
                    }
                    let result = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    match result {
                        Ok(()) => cx.editor.set_status(format!("Deleted {}", path.display())),
                        Err(err) => cx
                            .editor
                            .set_error(format!("Failed to delete {}: {err}", path.display())),
                    }
                    let keep = keep.clone();
                    refresh_after(cx, keep);
                },
            )
        });
        EventResult::Consumed(None)
    }

    /// `x`: cut into the explorer's clipboard for the next `p`.
    fn on_cut(&self, cx: &mut Context) -> EventResult {
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
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
        let Some(entry) = self.entry_cloned() else {
            return EventResult::Consumed(None);
        };
        let Some(clip) = cx.editor.explorer_clipboard.clone() else {
            cx.editor.set_error("Explorer clipboard is empty");
            return EventResult::Consumed(None);
        };

        let target_dir = if entry.is_dir {
            entry.path.clone()
        } else {
            entry
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone())
        };
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
            let message = format!("{} exists. Overwrite? (y/n) ", dest.display());
            confirm_then(cx, message, {
                let clip = clip.clone();
                let dest = dest.clone();
                move |cx| {
                    paste_now(cx, &clip, &dest);
                    None
                }
            });
        } else {
            paste_now(cx, &clip, &dest);
        }
        EventResult::Consumed(None)
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

    let removed = if clip.path.is_dir() {
        std::fs::remove_dir_all(&clip.path)
    } else {
        std::fs::remove_file(&clip.path)
    };
    match removed {
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
/// keeping the cursor at `keep`. Callable from inside prompt callbacks,
/// which have `jobs` but no direct compositor.
fn refresh_after(cx: &mut Context, keep: PathBuf) {
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
            if let Some(explorer) = compositor.find_id::<Explorer>(EXPLORER_ID) {
                explorer.refresh_keep(editor, Some(keep));
            }
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

/// Runs `op` only when the user answers `y` at the confirmation prompt.
///
/// The prompt is pushed via the jobs system; because prompt callbacks may be
/// invoked more than once (redraws), the stored op is kept in an `Option`.
fn confirm_then<F>(cx: &mut Context, message: String, op: F)
where
    F: FnOnce(&mut Context) -> Option<Result<String, String>> + Send + 'static,
{
    let mut op = Some(op);
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |_editor, compositor| {
            let prompt = Prompt::new(
                message.into(),
                None,
                completers::none,
                move |cx, input: &str, event: PromptEvent| {
                    if event != PromptEvent::Validate || input != "y" {
                        return;
                    }
                    if let Some(op) = op.take() {
                        if let Some(result) = op(cx) {
                            match result {
                                Ok(message) => cx.editor.set_status(message),
                                Err(message) => cx.editor.set_error(message),
                            }
                        }
                    }
                },
            );
            compositor.push(Box::new(prompt));
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

        self.on_nav_key(*key, cx)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        if !cx.editor.explorer_panel.visible {
            return;
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

        let theme = &cx.editor.theme;
        let text_style = theme.get("ui.text");
        let dir_style = theme.get("ui.text.directory");
        let sel_style = theme.get("ui.selection");
        let virtual_style = theme.get("ui.virtual");
        let background_style = theme.get("ui.popup");

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
                    for y in 0..rect.height {
                        surface[(sep_x, rect.y + y)]
                            .set_symbol("│")
                            .set_style(virtual_style);
                    }
                }
            }
        }

        // Scroll bookkeeping: keep the cursor inside the rendered slice.
        let rows = rect.height as usize;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if rows > 0 && self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
        self.clamp_cursor();

        let icons = ICONS.load();
        let mime = icons.mime();

        for (i, entry) in self
            .tree
            .visible()
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(rows)
        {
            let y = rect.y + (i - self.scroll) as u16;
            if y >= surface.area.bottom() {
                break;
            }

            // Highlight the selected row across the full panel width.
            if i == self.cursor {
                for x in 0..rect.width {
                    surface[(rect.x + x, y)].set_style(sel_style);
                }
            }

            let mut spans: Vec<Span> = Vec::with_capacity(3);
            spans.push(Span::styled(self.prefix(entry), virtual_style));
            if entry.is_dir {
                if let Some(glyph) = mime.directory() {
                    spans.push(Span::styled(format!("{glyph} "), dir_style));
                }
                spans.push(Span::styled(entry.name.clone(), dir_style));
            } else if let Some(icon) = mime.get(Some(&entry.path), Some(entry.name.as_str())) {
                let mut style = Style::default().fg(Color::Reset);
                if let Some(color) = icon.color() {
                    style = style.fg(color);
                }
                spans.push(Span::styled(format!("{} ", icon.glyph()), style));
                spans.push(Span::styled(entry.name.clone(), text_style));
            } else {
                spans.push(Span::styled(entry.name.clone(), text_style));
            }

            let mut x = rect.x;
            for span in truncate_spans(spans, rect.width as usize) {
                surface.set_string(x, y, &span.content, span.style);
                x += span.content.width() as u16;
            }
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
