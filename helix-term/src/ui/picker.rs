mod handlers;
mod query;

use crate::{
    alt,
    compositor::{self, Component, Compositor, Context, Event, EventResult},
    ctrl, key, shift,
    ui::{
        self,
        document::{render_document, LinePos, TextRenderer},
        gradient_border::GradientBorder,
        picker::query::PickerQuery,
        text_decorations::DecorationManager,
        EditorView,
    },
};
use futures_util::future::BoxFuture;
use helix_event::AsyncHook;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tui::{
    buffer::Buffer as Surface,
    layout::Constraint,
    text::{Span, Spans},
    widgets::{Block, BorderType, Cell, Row, Table},
};

use tui::widgets::Widget;

use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    io::Read,
    path::Path,
    sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
};

use crate::ui::{Prompt, PromptEvent};
use helix_core::{
    char_idx_at_visual_offset, fuzzy::MATCHER, movement::Direction,
    text_annotations::TextAnnotations, unicode::segmentation::UnicodeSegmentation,
    visual_offset_from_anchor, Position,
};
use helix_view::{
    editor::Action,
    graphics::{CursorKind, Margin, Modifier, Rect},
    input::KeyEvent,
    theme::Style,
    view::ViewPosition,
    Document, DocumentId, Editor,
};

use self::handlers::{DynamicQueryChange, DynamicQueryHandler, PreviewHighlightHandler};

pub const ID: &str = "picker";

pub const MIN_AREA_WIDTH_FOR_PREVIEW: u16 = 72;
pub const MIN_AREA_HEIGHT_FOR_PREVIEW: u16 = 24;
/// Biggest file size to preview in bytes
pub const MAX_FILE_SIZE_FOR_PREVIEW: u64 = 10 * 1024 * 1024;
/// Files bigger than this (in bytes) are read on a blocking thread, so that moving through big
/// files doesn't stall the picker. Smaller ones are read right away.
const MAX_FILE_SIZE_FOR_SYNC_PREVIEW: u64 = 256 * 1024;
/// How many previews a picker keeps. Going back and forth between results stays instant, but
/// walking through thousands of files doesn't keep all of them in memory.
const PREVIEW_CACHE_CAPACITY: usize = 64;

#[derive(PartialEq, Eq, Hash)]
pub enum PathOrId<'a> {
    Id(DocumentId),
    Path(&'a Path),
}

impl<'a> From<&'a Path> for PathOrId<'a> {
    fn from(path: &'a Path) -> Self {
        Self::Path(path)
    }
}

impl From<DocumentId> for PathOrId<'_> {
    fn from(v: DocumentId) -> Self {
        Self::Id(v)
    }
}

type FileCallback<T> = Box<dyn for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>>>;

/// File path and range of lines (used to align and highlight lines)
pub type FileLocation<'a> = (PathOrId<'a>, Option<(usize, usize)>);

pub enum CachedPreview {
    Document(Box<Document>),
    Directory(Vec<(String, bool)>),
    Binary,
    LargeFile,
    NotFound,
    Image(super::image_preview::ImagePreview),
    /// The file is being read in the background, see [`MAX_FILE_SIZE_FOR_SYNC_PREVIEW`].
    Loading,
}

// We don't store this enum in the cache so as to avoid lifetime constraints
// from borrowing a document already opened in the editor.
pub enum Preview<'picker, 'editor> {
    Cached(&'picker CachedPreview),
    EditorDocument(&'editor Document),
}

impl Preview<'_, '_> {
    fn document(&self) -> Option<&Document> {
        match self {
            Preview::EditorDocument(doc) => Some(doc),
            Preview::Cached(CachedPreview::Document(doc)) => Some(doc),
            _ => None,
        }
    }

    fn image(&self) -> Option<&super::image_preview::ImagePreview> {
        match self {
            Preview::Cached(CachedPreview::Image(image)) => Some(image),
            _ => None,
        }
    }

    fn dir_content(&self) -> Option<&Vec<(String, bool)>> {
        match self {
            Preview::Cached(CachedPreview::Directory(dir_content)) => Some(dir_content),
            _ => None,
        }
    }

    /// Alternate text to show for the preview.
    fn placeholder(&self) -> &str {
        match *self {
            Self::EditorDocument(_) => "<Invalid file location>",
            Self::Cached(preview) => match preview {
                CachedPreview::Document(_) => "<Invalid file location>",
                CachedPreview::Directory(_) => "<Invalid directory location>",
                CachedPreview::Binary => "<Binary file>",
                CachedPreview::LargeFile => "<File too large to preview>",
                CachedPreview::NotFound => "<File not found>",
                CachedPreview::Image(_) => "<Image>",
                CachedPreview::Loading => "<Loading…>",
            },
        }
    }
}

fn inject_nucleo_item<T, D>(
    injector: &nucleo::Injector<T>,
    columns: &[Column<T, D>],
    item: T,
    editor_data: &D,
) {
    injector.push(item, |item, dst| {
        for (column, text) in columns.iter().filter(|column| column.filter).zip(dst) {
            *text = column.format_text(item, editor_data).into()
        }
    });
}

pub struct Injector<T, D> {
    dst: nucleo::Injector<T>,
    columns: Arc<[Column<T, D>]>,
    editor_data: Arc<D>,
    version: usize,
    picker_version: Arc<AtomicUsize>,
    /// A marker that requests a redraw when the injector drops.
    /// This marker causes the "running" indicator to disappear when a background job
    /// providing items is finished and drops. This could be wrapped in an [Arc] to ensure
    /// that the redraw is only requested when all Injectors drop for a Picker (which removes
    /// the "running" indicator) but the redraw handle is debounced so this is unnecessary.
    _redraw: helix_event::RequestRedrawOnDrop,
}

impl<I, D> Clone for Injector<I, D> {
    fn clone(&self) -> Self {
        Injector {
            dst: self.dst.clone(),
            columns: self.columns.clone(),
            editor_data: self.editor_data.clone(),
            version: self.version,
            picker_version: self.picker_version.clone(),
            _redraw: helix_event::RequestRedrawOnDrop,
        }
    }
}

#[derive(Error, Debug)]
#[error("picker has been shut down")]
pub struct InjectorShutdown;

impl<T, D> Injector<T, D> {
    /// Whether the picker it feeds is still open (items pushed are shown).
    pub fn is_open(&self) -> bool {
        self.version == self.picker_version.load(atomic::Ordering::Relaxed)
    }

    pub fn push(&self, item: T) -> Result<(), InjectorShutdown> {
        if self.version != self.picker_version.load(atomic::Ordering::Relaxed) {
            return Err(InjectorShutdown);
        }

        inject_nucleo_item(&self.dst, &self.columns, item, &self.editor_data);
        Ok(())
    }
}

type ColumnFormatFn<T, D> = for<'a> fn(&'a T, &'a D) -> Cell<'a>;

pub struct Column<T, D> {
    name: Arc<str>,
    format: ColumnFormatFn<T, D>,
    /// Whether the column should be passed to nucleo for matching and filtering.
    /// `DynamicPicker` uses this so that the dynamic column (for example regex in
    /// global search) is not used for filtering twice.
    filter: bool,
    hidden: bool,
    /// The most columns it takes (`max_width`); longer texts are cut.
    max_width: Option<u16>,
}

impl<T, D> Column<T, D> {
    pub fn new(name: impl Into<Arc<str>>, format: ColumnFormatFn<T, D>) -> Self {
        Self {
            name: name.into(),
            format,
            filter: true,
            hidden: false,
            max_width: None,
        }
    }

    /// Takes at most `width` columns, leaving the others more room: a longer text is cut
    /// (and shown in full when its item is selected, with `Picker::wrap_selected`).
    pub fn max_width(mut self, width: u16) -> Self {
        self.max_width = Some(width);
        self
    }

    /// A column which does not display any contents
    pub fn hidden(name: impl Into<Arc<str>>) -> Self {
        let format = |_: &T, _: &D| unreachable!();

        Self {
            name: name.into(),
            format,
            filter: false,
            hidden: true,
            max_width: None,
        }
    }

    pub fn without_filtering(mut self) -> Self {
        self.filter = false;
        self
    }

    fn format<'a>(&self, item: &'a T, data: &'a D) -> Cell<'a> {
        (self.format)(item, data)
    }

    fn format_text<'a>(&self, item: &'a T, data: &'a D) -> Cow<'a, str> {
        let text: String = self.format(item, data).content.into();
        text.into()
    }
}

/// What a picker's replace action applies to.
pub enum ReplaceScope<'a, T> {
    /// Only the selected result.
    Selected(&'a T),
    /// Every result currently matching the picker's filters.
    All(Vec<&'a T>),
}

/// Applies a replacement: `(cx, query, replacement, scope)`.
type ReplaceCallback<T> = Box<dyn Fn(&mut Context, &str, &str, ReplaceScope<'_, T>)>;

/// Label drawn before the replacement input.
const REPLACE_LABEL: &str = "replace: ";

/// The replacement input row of a picker with a replace action.
struct ReplaceInput {
    prompt: Prompt,
    /// Whether typed text goes here rather than into the search input.
    focused: bool,
}

/// Returns a new list of options to replace the contents of the picker
/// when called with the current picker query,
type DynQueryCallback<T, D> =
    fn(&str, &mut Editor, Arc<D>, &Injector<T, D>) -> BoxFuture<'static, anyhow::Result<()>>;

/// Keeps a picker's results in sync with the editor, see [`Picker::with_live_refresh`].
struct LiveRefresh<T: 'static + Send + Sync, D: 'static> {
    /// Cheap summary of the data the results come from. A change triggers a refresh.
    fingerprint: Box<dyn Fn(&Editor) -> u64>,
    /// Pushes the up to date items. `None` re-runs the picker's dynamic query.
    rebuild: Option<RebuildFn<T, D>>,
    /// Identifies an item so the selection can be restored after a refresh.
    key: Box<dyn Fn(&T) -> String>,
    last: Option<u64>,
    last_refresh: std::time::Instant,
    wake_scheduled: bool,
    /// The item to select once the refreshed results are in.
    reselect: Option<Reselect>,
}

/// Finds the previously selected item in refreshed results. Keys are built once per item, as
/// items arrive (a refresh can stream in tens of thousands while the search runs), and the
/// item's position among the matches is then found by address.
struct Reselect {
    key: String,
    /// Address of the first item, which tells a rebuilt item list from the scanned one.
    items: usize,
    /// Items (by insertion index) already compared with `key`.
    scanned: u32,
    /// Address of the item whose key is `key`.
    found: Option<usize>,
}

impl Reselect {
    fn new(key: String) -> Self {
        Self {
            key,
            items: 0,
            scanned: 0,
            found: None,
        }
    }
}

/// Minimum time between two refreshes of a picker.
const LIVE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long a refreshed picker waits for its previously selected item to show up again.
const RESELECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The query and selected row of a picker, as they were before the latest key was handled.
/// A callback that runs on `Enter` can read the state the user left the picker in, to restore
/// it when it reopens a picker (see [`Picker::with_state_tracker`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickerState {
    pub query: String,
    pub cursor: u32,
}

/// Pushes the fresh items of a live picker into its injector.
type RebuildFn<T, D> = Box<dyn Fn(&Editor, &Injector<T, D>)>;

pub struct Picker<T: 'static + Send + Sync, D: 'static> {
    columns: Arc<[Column<T, D>]>,
    primary_column: usize,
    editor_data: Arc<D>,
    version: Arc<AtomicUsize>,
    matcher: Nucleo<T>,

    /// Current height of the completions box
    completion_height: u16,

    cursor: u32,
    prompt: Prompt,
    query: PickerQuery,
    /// Where the query and cursor are published before every key, see [`PickerState`].
    state_tracker: Option<Arc<std::sync::Mutex<PickerState>>>,

    /// Whether to show the preview panel (default true)
    show_preview: bool,
    /// Constraints for tabular formatting
    widths: Vec<Constraint>,

    callback_fn: PickerCallback<T>,
    /// The action of `Enter` and `Alt-Enter`, see [`Picker::with_default_action`].
    default_action: Action,
    custom_key_handlers: PickerKeyHandlers<T, D>,

    pub truncate_start: bool,
    /// Whether the selected item shows all the text of its columns, wrapped (`wrap_selected`).
    wrap_selected: bool,
    /// Caches paths to documents
    preview_cache: HashMap<Arc<Path>, CachedPreview>,
    /// The paths of `preview_cache`, oldest first, to evict beyond [`PREVIEW_CACHE_CAPACITY`].
    preview_cache_order: VecDeque<Arc<Path>>,
    read_buffer: Vec<u8>,
    /// Given an item in the picker, return the file path and line number to display.
    file_fn: Option<FileCallback<T>>,
    /// An event handler for syntax highlighting the currently previewed file.
    preview_highlight_handler: Sender<Arc<Path>>,
    dynamic_query_handler: Option<Sender<DynamicQueryChange>>,

    /// Vertical scroll of the file preview, in visual (soft-wrapped) rows relative to
    /// the natural top of the preview. Positive scrolls down, negative scrolls up.
    preview_scroll_offset: isize,
    /// Height in rows of the preview pane's inner text area, used for page scrolling.
    preview_height: u16,
    /// Selected item the current `preview_scroll_offset` applies to; the scroll resets
    /// to the top when the selection changes.
    preview_scroll_cursor: u32,

    /// Cached gradient border for rendering when enabled in config
    gradient_border: Option<GradientBorder>,
    /// Title to display at the top of the picker
    title: Option<Cow<'static, str>>,

    /// Replace action (`Alt-r` reveals the replacement input).
    replace_fn: Option<ReplaceCallback<T>>,
    replace: Option<ReplaceInput>,

    live: Option<LiveRefresh<T, D>>,

    /// The docked mode: the picker is the quickfix panel (see [`Picker::docked`]).
    docked: Option<Docked>,
}

/// The state of a picker shown as the quickfix panel (see `ui::quickfix`).
#[derive(Debug, Default)]
struct Docked {
    /// Whether the keys edit the query (after `/`) rather than move through the list.
    filtering: bool,
    /// `Ctrl-w` was pressed: the next key picks the window to go to.
    window_key: bool,
    /// Where the panel was last drawn (for the mouse).
    area: Rect,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    pub fn stream(
        columns: impl IntoIterator<Item = Column<T, D>>,
        editor_data: D,
    ) -> (Nucleo<T>, Injector<T, D>) {
        let columns: Arc<[_]> = columns.into_iter().collect();
        let matcher_columns = columns.iter().filter(|col| col.filter).count() as u32;
        assert!(matcher_columns > 0);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(helix_event::request_redraw),
            None,
            matcher_columns,
        );
        let streamer = Injector {
            dst: matcher.injector(),
            columns,
            editor_data: Arc::new(editor_data),
            version: 0,
            picker_version: Arc::new(AtomicUsize::new(0)),
            _redraw: helix_event::RequestRedrawOnDrop,
        };
        (matcher, streamer)
    }

    pub fn new<C, O, F>(
        columns: C,
        primary_column: usize,
        options: O,
        editor_data: D,
        callback_fn: F,
    ) -> Self
    where
        C: IntoIterator<Item = Column<T, D>>,
        O: IntoIterator<Item = T>,
        F: Fn(&mut Context, &T, Action) + 'static,
    {
        let columns: Arc<[_]> = columns.into_iter().collect();
        let matcher_columns = columns.iter().filter(|col| col.filter).count() as u32;
        assert!(matcher_columns > 0);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(helix_event::request_redraw),
            None,
            matcher_columns,
        );
        let injector = matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &columns, item, &editor_data);
        }
        Self::with(
            matcher,
            columns,
            primary_column,
            Arc::new(editor_data),
            Arc::new(AtomicUsize::new(0)),
            callback_fn,
        )
    }

    pub fn with_stream(
        matcher: Nucleo<T>,
        primary_column: usize,
        injector: Injector<T, D>,
        callback_fn: impl Fn(&mut Context, &T, Action) + 'static,
    ) -> Self {
        Self::with(
            matcher,
            injector.columns,
            primary_column,
            injector.editor_data,
            injector.picker_version,
            callback_fn,
        )
    }

    fn with(
        matcher: Nucleo<T>,
        columns: Arc<[Column<T, D>]>,
        default_column: usize,
        editor_data: Arc<D>,
        version: Arc<AtomicUsize>,
        callback_fn: impl Fn(&mut Context, &T, Action) + 'static,
    ) -> Self {
        assert!(!columns.is_empty());

        let prompt = Prompt::new(
            "".into(),
            None,
            ui::completers::none,
            |_editor: &mut Context, _pattern: &str, _event: PromptEvent| {},
        );

        let widths = columns
            .iter()
            .map(|column| Constraint::Length(column.name.chars().count() as u16))
            .collect();

        let query = PickerQuery::new(columns.iter().map(|col| &col.name).cloned(), default_column);

        Self {
            columns,
            primary_column: default_column,
            matcher,
            editor_data,
            version,
            cursor: 0,
            prompt,
            query,
            truncate_start: true,
            wrap_selected: false,
            show_preview: true,
            callback_fn: Box::new(callback_fn),
            completion_height: 0,
            widths,
            preview_cache: HashMap::new(),
            preview_cache_order: VecDeque::new(),
            default_action: Action::Replace,
            custom_key_handlers: HashMap::new(),
            read_buffer: Vec::with_capacity(1024),
            file_fn: None,
            preview_highlight_handler: PreviewHighlightHandler::<T, D>::default().spawn(),
            dynamic_query_handler: None,
            preview_scroll_offset: 0,
            preview_height: 0,
            preview_scroll_cursor: 0,
            gradient_border: None,
            title: None,
            replace_fn: None,
            replace: None,
            live: None,
            state_tracker: None,
            docked: None,
        }
    }

    /// Keeps the results up to date while the picker is open.
    ///
    /// `fingerprint` summarizes the data the results are built from and is
    /// checked on every redraw (at most every 250ms); when it changes `rebuild`
    /// pushes the fresh items into an injector. Pass `None` for `rebuild` on
    /// pickers with a dynamic query to re-run the query instead. `key`
    /// identifies an item, so the selection stays on the same item (when it
    /// still exists) and the query stays as typed.
    pub fn with_live_refresh(
        mut self,
        fingerprint: impl Fn(&Editor) -> u64 + 'static,
        rebuild: Option<RebuildFn<T, D>>,
        key: impl Fn(&T) -> String + 'static,
    ) -> Self {
        self.live = Some(LiveRefresh {
            fingerprint: Box::new(fingerprint),
            rebuild,
            key: Box::new(key),
            last: None,
            last_refresh: std::time::Instant::now(),
            wake_scheduled: false,
            reselect: None,
        });
        self
    }

    /// Refreshes the results when the data they come from changed.
    fn refresh_if_stale(&mut self, editor: &Editor) {
        let Some(mut live) = self.live.take() else {
            return;
        };
        self.refresh_if_stale_with(&mut live, editor);
        self.live = Some(live);
    }

    fn refresh_if_stale_with(&mut self, live: &mut LiveRefresh<T, D>, editor: &Editor) {
        let current = (live.fingerprint)(editor);
        match live.last {
            None => {
                live.last = Some(current);
                return;
            }
            Some(last) if last == current => return,
            Some(_) => {}
        }
        let since = live.last_refresh.elapsed();
        if since < LIVE_REFRESH_INTERVAL {
            // Too soon: come back for another redraw once the interval has passed.
            if !live.wake_scheduled {
                live.wake_scheduled = true;
                let wait = LIVE_REFRESH_INTERVAL - since;
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    helix_event::request_redraw();
                });
            }
            return;
        }
        live.last = Some(current);
        live.last_refresh = std::time::Instant::now();
        live.wake_scheduled = false;
        // Remember what is selected, unless a refresh is still waiting to restore it.
        if live.reselect.is_none() {
            live.reselect = self
                .matcher
                .snapshot()
                .get_matched_item(self.cursor)
                .map(|item| Reselect::new((live.key)(item.data)));
        }
        if let Some(rebuild) = &live.rebuild {
            self.version.fetch_add(1, atomic::Ordering::Relaxed);
            self.matcher.restart(false);
            let injector = self.injector();
            rebuild(editor, &injector);
        } else {
            self.refresh_results();
        }
    }

    /// Replaces the quicklist with the locations of the results that currently match, in the
    /// order they are shown in. Pickers whose results aren't locations (no preview) are left
    /// alone; a picker without matches clears the quicklist.
    fn copy_to_quicklist(&mut self, editor: &mut Editor) {
        if self.file_fn.is_none() {
            editor.set_status("The results of this picker are not locations");
            return;
        }
        self.settle_matcher();
        let Some(file_fn) = &self.file_fn else { return };
        let snapshot = self.matcher.snapshot();
        let entries = crate::commands::quicklist::entries_from_locations(
            editor,
            snapshot
                .matched_items(..)
                .filter_map(|item| file_fn(editor, item.data)),
        );
        crate::commands::quicklist::set_from_picker(editor, entries);
    }

    /// Waits (briefly) until the results are those of the current query. The matcher works in
    /// the background, so after typing, its snapshot is still the one of the previous query
    /// for a moment, and `tick` can report that nothing is running while the job for the new
    /// query is only queued: it is the snapshot's own pattern that tells.
    fn settle_matcher(&mut self) {
        let columns = self.columns.iter().filter(|column| column.filter).count();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut changed = false;
        loop {
            let status = self.matcher.tick(10);
            changed |= status.changed;
            let up_to_date = {
                let snapshot = self.matcher.snapshot();
                (0..columns).all(|column| {
                    snapshot.pattern().column_pattern(column).atoms
                        == self.matcher.pattern.column_pattern(column).atoms
                })
            };
            if (up_to_date && !status.running) || std::time::Instant::now() >= deadline {
                break;
            }
        }
        // What `render_picker` does with a change.
        if changed {
            self.restore_selection();
            let matched = self.matcher.snapshot().matched_item_count();
            self.cursor = self.cursor.min(matched.saturating_sub(1));
        }
    }

    /// Moves the cursor back to the item that was selected before a refresh.
    fn restore_selection(&mut self) {
        let Some(live) = &mut self.live else { return };
        let Some(reselect) = &mut live.reselect else {
            return;
        };
        let snapshot = self.matcher.snapshot();
        let address = |data: &T| data as *const T as usize;
        let items = snapshot.get_item(0).map_or(0, |item| address(item.data));
        if items != reselect.items {
            *reselect = Reselect {
                items,
                ..Reselect::new(std::mem::take(&mut reselect.key))
            };
        }
        if reselect.found.is_none() {
            while reselect.scanned < snapshot.item_count() {
                // Not yet written by the injector: look again on the next frame.
                let Some(item) = snapshot.get_item(reselect.scanned) else {
                    break;
                };
                reselect.scanned += 1;
                if (live.key)(item.data) == reselect.key {
                    reselect.found = Some(address(item.data));
                    break;
                }
            }
        }
        let Some(found) = reselect.found else { return };
        if let Some(index) = snapshot
            .matched_items(..)
            .position(|item| address(item.data) == found)
        {
            self.cursor = index as u32;
            live.reselect = None;
        }
    }

    /// Gives the picker a replace action. `Alt-r` reveals a replacement input
    /// below the query (and switches focus between the two); while it is shown,
    /// `Enter` replaces the selected result and `Ctrl-Enter` (or `Alt-a`)
    /// replaces every matching result. The callback gets the query, the
    /// replacement text and the scope; the picker re-runs its query afterwards.
    pub fn with_replace(
        mut self,
        callback: impl Fn(&mut Context, &str, &str, ReplaceScope<'_, T>) + 'static,
    ) -> Self {
        self.replace_fn = Some(Box::new(callback));
        self
    }

    /// Reveals the replacement input, or moves focus between it and the query.
    fn toggle_replace_focus(&mut self) {
        match &mut self.replace {
            Some(replace) => replace.focused = !replace.focused,
            None => {
                self.replace = Some(ReplaceInput {
                    prompt: Prompt::new(
                        "".into(),
                        None,
                        ui::completers::none,
                        |_cx: &mut Context, _line: &str, _event: PromptEvent| {},
                    ),
                    focused: true,
                });
            }
        }
    }

    /// Re-runs the dynamic query, e.g. after the results were changed on disk.
    fn refresh_results(&self) {
        if let Some(handler) = &self.dynamic_query_handler {
            helix_event::send_blocking(
                handler,
                DynamicQueryChange {
                    query: self.primary_query(),
                    is_paste: true,
                    force: true,
                },
            );
        }
    }

    /// Runs the replace action on the selected result, or on all results.
    fn run_replace(&self, cx: &mut Context, all: bool) {
        let (Some(replace_fn), Some(input)) = (&self.replace_fn, &self.replace) else {
            return;
        };
        let replacement = input.prompt.line().clone();
        let query = self.primary_query();
        let snapshot = self.matcher.snapshot();
        if all {
            let items: Vec<&T> = snapshot.matched_items(..).map(|item| item.data).collect();
            if items.is_empty() {
                return;
            }
            replace_fn(cx, &query, &replacement, ReplaceScope::All(items));
        } else if let Some(item) = self.selection() {
            replace_fn(cx, &query, &replacement, ReplaceScope::Selected(item));
        } else {
            return;
        }
        self.refresh_results();
    }

    pub fn with_key_handlers(mut self, handlers: PickerKeyHandlers<T, D>) -> Self {
        self.custom_key_handlers = handlers;
        self
    }

    /// The action an item is opened with on `Enter` (and `Alt-Enter`), `Action::Replace` by
    /// default; e.g. `:vsplit <dir>` opens the picked file in a vertical split.
    pub fn with_default_action(mut self, action: Action) -> Self {
        self.default_action = action;
        self
    }

    pub fn injector(&self) -> Injector<T, D> {
        Injector {
            dst: self.matcher.injector(),
            columns: self.columns.clone(),
            editor_data: self.editor_data.clone(),
            version: self.version.load(atomic::Ordering::Relaxed),
            picker_version: self.version.clone(),
            _redraw: helix_event::RequestRedrawOnDrop,
        }
    }

    /// Shows all the text of the selected item's columns, wrapped over several lines (a long
    /// description, say); the other items keep one line each.
    pub fn wrap_selected(mut self, wrap: bool) -> Self {
        self.wrap_selected = wrap;
        self
    }

    pub fn truncate_start(mut self, truncate_start: bool) -> Self {
        self.truncate_start = truncate_start;
        self
    }

    pub fn with_preview(
        mut self,
        preview_fn: impl for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>> + 'static,
    ) -> Self {
        self.file_fn = Some(Box::new(preview_fn));
        // assumption: if we have a preview we are matching paths... If this is ever
        // not true this could be a separate builder function
        self.matcher.update_config(Config::DEFAULT.match_paths());
        self
    }

    pub fn with_history_register(mut self, history_register: Option<char>) -> Self {
        self.prompt.with_history_register(history_register);
        self
    }

    /// Labels the query input with `prompt` (e.g. `"Choose:"`), drawn before the typed text;
    /// pickers have none by default. Call it before [`Self::with_query`], whose text it drops.
    pub fn with_prompt(mut self, prompt: impl Into<Cow<'static, str>>) -> Self {
        let history_register = self.prompt.history_register();
        let mut prompt = Prompt::new(
            prompt.into(),
            None,
            ui::completers::none,
            |_editor: &mut Context, _pattern: &str, _event: PromptEvent| {},
        );
        prompt.with_history_register(history_register);
        self.prompt = prompt;
        self
    }

    pub fn show_preview(mut self, show_preview: bool) -> Self {
        self.show_preview = show_preview;
        self
    }

    /// Set a title to display at the top of the picker
    pub fn with_title(mut self, title: impl Into<Cow<'static, str>>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_dynamic_query(
        mut self,
        callback: DynQueryCallback<T, D>,
        debounce_ms: Option<u64>,
    ) -> Self {
        let handler = DynamicQueryHandler::new(callback, debounce_ms).spawn();
        let event = DynamicQueryChange {
            query: self.primary_query(),
            // Treat the initial query as a paste.
            is_paste: true,
            force: false,
        };
        helix_event::send_blocking(&handler, event);
        self.dynamic_query_handler = Some(handler);
        self
    }

    /// Scrolls the file preview by `amount` visual lines, down for a positive amount and
    /// up for a negative one.
    ///
    /// The offset is counted in visual (soft-wrapped) rows so a file with long lines can
    /// be scrolled to its end one wrapped row at a time. It is clamped against the file
    /// bounds later, when the preview is rendered and the soft-wrap layout is known.
    fn scroll_preview(&mut self, amount: isize) {
        self.preview_scroll_offset = self.preview_scroll_offset.saturating_add(amount);
    }

    /// Whether a scrollable file preview is currently shown. Pickers without a preview
    /// callback (no `file_fn`) or with the preview toggled off route preview-scroll keys
    /// back to result-list navigation.
    fn preview_shown(&self) -> bool {
        self.show_preview && self.file_fn.is_some()
    }

    /// Move the cursor by a number of lines, either down (`Forward`) or up (`Backward`)
    pub fn move_by(&mut self, amount: u32, direction: Direction) {
        let len = self.matcher.snapshot().matched_item_count();

        if len == 0 {
            // No results, can't move.
            return;
        }

        match direction {
            Direction::Forward => {
                self.cursor = self.cursor.saturating_add(amount) % len;
            }
            Direction::Backward => {
                self.cursor = self.cursor.saturating_add(len).saturating_sub(amount) % len;
            }
        }
    }

    /// Move the cursor down by exactly one page. After the last page comes the first page.
    pub fn page_up(&mut self) {
        self.move_by(self.completion_height as u32, Direction::Backward);
    }

    /// Move the cursor up by exactly one page. After the first page comes the last page.
    pub fn page_down(&mut self) {
        self.move_by(self.completion_height as u32, Direction::Forward);
    }

    /// Move the cursor to the first entry
    pub fn to_start(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the last entry
    pub fn to_end(&mut self) {
        self.cursor = self
            .matcher
            .snapshot()
            .matched_item_count()
            .saturating_sub(1);
    }

    /// Starts the picker with `query` typed into the prompt. Call [`Self::with_cursor`]
    /// afterwards to select a row, a changed query resets the selection.
    pub fn with_query(mut self, query: &str, editor: &Editor) -> Self {
        self.prompt.set_line(query.to_owned(), editor);
        self.handle_prompt_change(false);
        self
    }

    /// Publishes the query and the selected row into `tracker` before every key is handled,
    /// so the state before an `Enter` can be read by the picker's callback.
    pub fn with_state_tracker(mut self, tracker: Arc<std::sync::Mutex<PickerState>>) -> Self {
        self.state_tracker = Some(tracker);
        self
    }

    pub fn with_cursor(mut self, cursor: u32) -> Self {
        self.cursor = cursor;
        self
    }

    pub fn selection(&self) -> Option<&T> {
        self.matcher
            .snapshot()
            .get_matched_item(self.cursor)
            .map(|item| item.data)
    }

    fn primary_query(&self) -> Arc<str> {
        self.query
            .get(&self.columns[self.primary_column].name)
            .cloned()
            .unwrap_or_else(|| "".into())
    }

    fn header_height(&self) -> u16 {
        if self.columns.len() > 1 {
            1
        } else {
            0
        }
    }

    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
    }

    fn custom_key_event_handler(&mut self, event: &KeyEvent, cx: &mut Context) -> EventResult {
        if let (Some(callback), Some(selected)) =
            (self.custom_key_handlers.get(event), self.selection())
        {
            callback(cx, selected, Arc::clone(&self.editor_data), self.cursor);
            EventResult::Consumed(None)
        } else {
            EventResult::Ignored(None)
        }
    }

    fn prompt_handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if let Some(replace) = self.replace.as_mut().filter(|replace| replace.focused) {
            // Typing goes into the replacement; it does not change the query.
            replace.prompt.handle_event(event, cx);
            return EventResult::Consumed(None);
        }
        if let EventResult::Consumed(_) = self.prompt.handle_event(event, cx) {
            self.handle_prompt_change(matches!(event, Event::Paste(_)));
        }
        EventResult::Consumed(None)
    }

    fn handle_prompt_change(&mut self, is_paste: bool) {
        // TODO: better track how the pattern has changed
        let line = self.prompt.line();
        let old_query = self.query.parse(line);
        if self.query == old_query {
            return;
        }
        // If the query has meaningfully changed, reset the cursor to the top of the results.
        self.cursor = 0;
        // Have nucleo reparse each changed column.
        for (i, column) in self
            .columns
            .iter()
            .filter(|column| column.filter)
            .enumerate()
        {
            let pattern = self
                .query
                .get(&column.name)
                .map(|f| &**f)
                .unwrap_or_default();
            let old_pattern = old_query
                .get(&column.name)
                .map(|f| &**f)
                .unwrap_or_default();
            // Fastlane: most columns will remain unchanged after each edit.
            if pattern == old_pattern {
                continue;
            }
            let is_append = pattern.starts_with(old_pattern);
            self.matcher.pattern.reparse(
                i,
                pattern,
                CaseMatching::Smart,
                Normalization::Smart,
                is_append,
            );
        }
        // If this is a dynamic picker, notify the query hook that the primary
        // query might have been updated.
        if let Some(handler) = &self.dynamic_query_handler {
            let event = DynamicQueryChange {
                query: self.primary_query(),
                is_paste,
                force: false,
            };
            helix_event::send_blocking(handler, event);
        }
    }

    /// Get (cached) preview for the currently selected item. If a document corresponding
    /// to the path is already open in the editor, it is used instead.
    fn get_preview<'picker, 'editor>(
        &'picker mut self,
        editor: &'editor Editor,
    ) -> Option<(Preview<'picker, 'editor>, Option<(usize, usize)>)> {
        let current = self.selection()?;
        let (path_or_id, range) = (self.file_fn.as_ref()?)(editor, current)?;

        match path_or_id {
            PathOrId::Path(path) => {
                if let Some(doc) = editor.document_by_path(path) {
                    return Some((Preview::EditorDocument(doc), range));
                }

                if self.preview_cache.contains_key(path) {
                    // NOTE: we use `HashMap::get_key_value` here instead of indexing so we can
                    // retrieve the `Arc<Path>` key. The `path` in scope here is a `&Path` and
                    // we can cheaply clone the key for the preview highlight handler.
                    let (path, preview) = self.preview_cache.get_key_value(path).unwrap();
                    // A document whose highlighting was skipped (the handler only highlights
                    // the last of several quickly previewed files) asks again. One without a
                    // language never gets a syntax: asking for it on every frame would wake
                    // the editor up again and again.
                    if matches!(preview, CachedPreview::Document(doc) if doc.language_config().is_some() && doc.syntax().is_none())
                    {
                        helix_event::send_blocking(&self.preview_highlight_handler, path.clone());
                    }
                    return Some((Preview::Cached(preview), range));
                }

                let path: Arc<Path> = path.into();
                let read_editor_config = editor.config().editor_config;
                let mut listing_directory = false;
                let preview = std::fs::metadata(&path)
                    .and_then(|metadata| {
                        if metadata.is_dir() {
                            // Listing a big (or network) directory can take a while.
                            listing_directory = true;
                            Self::load_directory_in_background(
                                path.clone(),
                                editor.config().file_explorer.clone(),
                            );
                            Ok(CachedPreview::Loading)
                        } else if metadata.is_file() {
                            if metadata.len() > MAX_FILE_SIZE_FOR_PREVIEW {
                                return Ok(CachedPreview::LargeFile);
                            }
                            let (is_binary, is_image) =
                                std::fs::File::open(&path).and_then(|file| {
                                    // Read up to 1kb to detect the content type
                                    let n = file.take(1024).read_to_end(&mut self.read_buffer)?;
                                    let start = &self.read_buffer[..n];
                                    let kind = (
                                        crate::is_binary(start),
                                        helix_view::image_preview::is_image(start),
                                    );
                                    self.read_buffer.clear();
                                    Ok(kind)
                                })?;
                            if is_image && editor.config().image_preview.enable {
                                return Ok(CachedPreview::Image(
                                    super::image_preview::ImagePreview::open(&path),
                                ));
                            }
                            if is_binary {
                                return Ok(CachedPreview::Binary);
                            }
                            if metadata.len() > MAX_FILE_SIZE_FOR_SYNC_PREVIEW {
                                return Ok(CachedPreview::Loading);
                            }
                            Document::open(
                                &path,
                                None,
                                false,
                                editor.config.clone(),
                                editor.syn_loader.clone(),
                            )
                            .map_or(
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "Cannot open document",
                                )),
                                |mut doc| {
                                    let loader = editor.syn_loader.load();
                                    if let Some(language_config) =
                                        doc.detect_language_config(&loader)
                                    {
                                        doc.language = Some(language_config);

                                        // Asynchronously highlight the new document
                                        helix_event::send_blocking(
                                            &self.preview_highlight_handler,
                                            path.clone(),
                                        );
                                    }
                                    Ok(CachedPreview::Document(Box::new(doc)))
                                },
                            )
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "Neither a dir, nor a file",
                            ))
                        }
                    })
                    .unwrap_or(CachedPreview::NotFound);
                if matches!(preview, CachedPreview::Loading) && !listing_directory {
                    Self::load_preview_in_background(path.clone(), read_editor_config);
                }
                self.cache_preview(path.clone(), preview);
                Some((Preview::Cached(&self.preview_cache[&path]), range))
            }
            PathOrId::Id(id) => {
                // The buffer may have been closed since the item was made.
                let doc = editor.documents.get(&id)?;
                Some((Preview::EditorDocument(doc), range))
            }
        }
    }

    /// Adds a preview to the cache, dropping the oldest ones beyond [`PREVIEW_CACHE_CAPACITY`].
    fn cache_preview(&mut self, path: Arc<Path>, preview: CachedPreview) {
        while self.preview_cache.len() >= PREVIEW_CACHE_CAPACITY {
            let Some(oldest) = self.preview_cache_order.pop_front() else {
                break;
            };
            self.preview_cache.remove(&oldest);
        }
        if self.preview_cache.insert(path.clone(), preview).is_none() {
            self.preview_cache_order.push_back(path);
        }
    }

    /// Lists a previewed directory on a blocking thread, then puts its preview in place of the
    /// [`CachedPreview::Loading`] one, if the picker is still open.
    fn load_directory_in_background(
        path: Arc<Path>,
        explorer: helix_view::editor::FileExplorerConfig,
    ) {
        tokio::task::spawn_blocking(move || {
            let listing = super::directory_content_with(&path, &explorer).map(|files| {
                files
                    .iter()
                    .filter_map(|(path, is_dir)| {
                        let name = path.file_name()?.to_string_lossy();
                        Some(if *is_dir {
                            (format!("{name}/"), true)
                        } else {
                            (name.into_owned(), false)
                        })
                    })
                    .collect::<Vec<_>>()
            });
            crate::job::dispatch_blocking(move |_editor, compositor| {
                let Some(picker) = find_picker::<T, D>(compositor) else {
                    return;
                };
                if let Some(entry @ CachedPreview::Loading) = picker.preview_cache.get_mut(&path) {
                    *entry = match listing {
                        Ok(names) => CachedPreview::Directory(names),
                        Err(err) => {
                            log::info!("previewing {} failed: {err}", path.display());
                            CachedPreview::NotFound
                        }
                    };
                }
            });
        });
    }

    /// Reads a file too big to read on the UI thread (see [`MAX_FILE_SIZE_FOR_SYNC_PREVIEW`])
    /// on a blocking thread, then puts its preview in place of the [`CachedPreview::Loading`]
    /// one, if the picker is still open and the preview is still wanted.
    fn load_preview_in_background(path: Arc<Path>, read_editor_config: bool) {
        tokio::task::spawn_blocking(move || {
            let encoding = if read_editor_config {
                helix_core::editor_config::EditorConfig::find(&path).encoding
            } else {
                None
            };
            let loaded = std::fs::File::open(&path)
                .and_then(|mut file| helix_view::document::from_reader(&mut file, encoding));
            crate::job::dispatch_blocking(move |editor, compositor| {
                let Some(picker) = find_picker::<T, D>(compositor) else {
                    return;
                };
                let Some(entry @ CachedPreview::Loading) = picker.preview_cache.get_mut(&path)
                else {
                    return;
                };
                *entry = match loaded {
                    Ok((rope, encoding, has_bom)) => {
                        let mut doc = Document::from(
                            rope,
                            Some((encoding, has_bom)),
                            editor.config.clone(),
                            editor.syn_loader.clone(),
                        );
                        doc.set_path(Some(&path));
                        doc.detect_indent_and_line_ending();
                        let loader = editor.syn_loader.load();
                        if let Some(language_config) = doc.detect_language_config(&loader) {
                            doc.language = Some(language_config);
                            helix_event::send_blocking(
                                &picker.preview_highlight_handler,
                                path.clone(),
                            );
                        }
                        CachedPreview::Document(Box::new(doc))
                    }
                    Err(err) => {
                        log::info!("previewing {} failed: {err}", path.display());
                        CachedPreview::NotFound
                    }
                };
            });
        });
    }

    fn render_picker(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.refresh_if_stale(cx.editor);
        let status = self.matcher.tick(10);
        if status.changed {
            self.restore_selection();
        }
        let snapshot = self.matcher.snapshot();
        if status.changed {
            self.cursor = self
                .cursor
                .min(snapshot.matched_item_count().saturating_sub(1))
        }
        if let Some(live) = &mut self.live {
            // The selected item is gone (or the results never came): stop looking.
            if !status.running && live.last_refresh.elapsed() > RESELECT_TIMEOUT {
                live.reselect = None;
            }
        }

        let text_style = cx.editor.theme.get("ui.text");
        let selected = cx
            .editor
            .theme
            .try_get("ui.text.focus")
            .filter(|s| s.bg.is_some())
            .unwrap_or_else(|| cx.editor.theme.get("ui.menu.selected"));
        let highlight_style = cx.editor.theme.get("special").add_modifier(Modifier::BOLD);

        // -- Render the frame:
        // clear area
        let background = cx.editor.theme.get("ui.background");
        // Only inside the border (see `clear_bordered`).
        crate::ui::clear_bordered(
            surface,
            area,
            background,
            !self.border_cells_take_background(&cx.editor.config()),
        );

        // calculate the inner area inside the box
        let inner = if self.docked.is_some() {
            // The quickfix panel has no border: its title is on the rule at its top.
            area
        } else if self.gradient_border_shown(&cx.editor.config()) {
            if self.gradient_border.is_none() {
                self.gradient_border = Some(GradientBorder::from_theme(
                    &cx.editor.theme,
                    &cx.editor.config().gradient_borders,
                ));
            }

            if let Some(ref mut gradient_border) = self.gradient_border {
                let rounded = cx.editor.config().border_rounded();
                if !cx.editor.config().borders_hidden() {
                    gradient_border.render_with_title(
                        area,
                        surface,
                        &cx.editor.theme,
                        self.title.as_deref(),
                        rounded,
                    );
                }
            }

            // The gradient border is one cell wide whatever its thickness.
            crate::ui::gradient_border::GradientBorder::inner_area(area)
        } else {
            let block = if let Some(title) = self.title.as_deref() {
                // `ui.popup.title` when the theme sets it; otherwise the title takes the
                // border's style, as upstream.
                match cx.editor.theme.try_get("ui.popup.title") {
                    Some(style) => Block::bordered().title(tui::text::Span::styled(title, style)),
                    None => Block::bordered().title(title),
                }
            } else {
                Block::bordered()
            }
            .border_type(crate::ui::border_type(&cx.editor.config()))
            .border_style(crate::ui::border_style(&cx.editor.theme));
            let inner_area = block.inner(area);
            block.render(area, surface);
            inner_area
        };

        // -- Render the input bar:

        let count = format!(
            "{}{}/{}",
            if status.running || self.matcher.active_injectors() > 0 {
                "(running) "
            } else {
                ""
            },
            snapshot.matched_item_count(),
            snapshot.item_count(),
        );

        // The quickfix panel: the rule (with the title) first, then the query.
        let docked = self.docked.is_some();
        let prompt_row = if docked { 1 } else { 0 };
        let area = inner
            .clip_left(1)
            .clip_top(prompt_row)
            .with_height(1.min(inner.height.saturating_sub(prompt_row)));
        let line_area = area.clip_right(count.len() as u16 + 1);

        // render the prompt first since it will clear its background
        self.prompt.render(line_area, surface, cx);

        surface.set_stringn(
            (area.x + area.width).saturating_sub(count.len() as u16 + 1),
            area.y,
            &count,
            (count.len()).min(area.width as usize),
            text_style,
        );

        // -- Replacement input (second row), when revealed
        let replace_rows = u16::from(self.replace.is_some() && inner.height > 4);
        if replace_rows > 0 {
            if let Some(replace) = self.replace.as_mut() {
                let label_width = REPLACE_LABEL.len() as u16;
                let row = Rect::new(area.x, area.y + 1, area.width, 1);
                surface.clear_with(row, text_style);
                surface.set_stringn(
                    row.x,
                    row.y,
                    REPLACE_LABEL,
                    label_width as usize,
                    cx.editor.theme.get("ui.virtual"),
                );
                let input_area = Rect::new(
                    line_area.x + label_width,
                    row.y,
                    line_area.width.saturating_sub(label_width),
                    1,
                );
                replace.prompt.render(input_area, surface, cx);
            }
        }

        // -- Separator (the quickfix panel's top rule, with its title)
        let sep_style = cx.editor.theme.get("ui.background.separator");
        let borders = BorderType::line_symbols(crate::ui::separator_type(&cx.editor.config()));
        let sep_y = if docked {
            inner.y
        } else {
            inner.y + 1 + replace_rows
        };
        for x in inner.left()..inner.right() {
            if let Some(cell) = surface.get_mut(x, sep_y) {
                cell.set_symbol(borders.horizontal).set_style(sep_style);
            }
        }
        if let (true, Some(title)) = (docked, self.title.as_deref()) {
            let title_style = cx
                .editor
                .theme
                .try_get("ui.popup.title")
                .unwrap_or(text_style);
            let title = format!(" {title} ");
            surface.set_stringn(
                inner.x.saturating_add(1),
                sep_y,
                &title,
                inner.width.saturating_sub(2) as usize,
                title_style,
            );
        }

        // -- Render the contents:
        // subtract area of prompt (and replacement row) from top
        let inner = inner.clip_top(2 + replace_rows);
        let rows = inner.height.saturating_sub(self.header_height()) as u32;
        let offset = self.cursor - (self.cursor % std::cmp::max(1, rows));
        let cursor = self.cursor.saturating_sub(offset);
        let end = offset
            .saturating_add(rows)
            .min(snapshot.matched_item_count());
        let mut indices = Vec::new();
        let mut matcher = MATCHER.lock();
        matcher.config = Config::DEFAULT;
        if self.file_fn.is_some() {
            matcher.config.set_match_paths()
        }

        let options = snapshot.matched_items(offset..end).map(|item| {
            let mut widths = self.widths.iter_mut();
            let mut matcher_index = 0;

            Row::new(self.columns.iter().map(|column| {
                if column.hidden {
                    return Cell::default();
                }

                let Some(Constraint::Length(max_width)) = widths.next() else {
                    unreachable!();
                };
                let mut cell = column.format(item.data, &self.editor_data);
                let width = if column.filter {
                    snapshot.pattern().column_pattern(matcher_index).indices(
                        item.matcher_columns[matcher_index].slice(..),
                        &mut matcher,
                        &mut indices,
                    );
                    indices.sort_unstable();
                    indices.dedup();
                    let mut indices = indices.drain(..);
                    let mut next_highlight_idx = indices.next().unwrap_or(u32::MAX);
                    let mut span_list = Vec::new();
                    let mut current_span = String::new();
                    let mut current_style = Style::default();
                    let mut grapheme_idx = 0u32;
                    let mut width = 0;

                    let spans: &[Span] =
                        cell.content.lines.first().map_or(&[], |it| it.0.as_slice());
                    for span in spans {
                        // this looks like a bug on first glance, we are iterating
                        // graphemes but treating them as char indices. The reason that
                        // this is correct is that nucleo will only ever consider the first char
                        // of a grapheme (and discard the rest of the grapheme) so the indices
                        // returned by nucleo are essentially grapheme indecies
                        for grapheme in span.content.graphemes(true) {
                            let style = if grapheme_idx == next_highlight_idx {
                                next_highlight_idx = indices.next().unwrap_or(u32::MAX);
                                span.style.patch(highlight_style)
                            } else {
                                span.style
                            };
                            if style != current_style {
                                if !current_span.is_empty() {
                                    span_list.push(Span::styled(current_span, current_style))
                                }
                                current_span = String::new();
                                current_style = style;
                            }
                            current_span.push_str(grapheme);
                            grapheme_idx += 1;
                        }
                        width += span.width();
                    }

                    span_list.push(Span::styled(current_span, current_style));
                    // The matches are in the first line; the others stay (a doc of several
                    // lines, shown in full by `wrap_selected`).
                    let mut lines = vec![Spans::from(span_list)];
                    lines.extend(cell.content.lines.iter().skip(1).cloned());
                    cell = Cell::from(tui::text::Text::from(lines));
                    matcher_index += 1;
                    width
                } else {
                    cell.content
                        .lines
                        .first()
                        .map(|line| line.width())
                        .unwrap_or_default()
                };

                let width = column
                    .max_width
                    .map_or(width, |cap| width.min(cap as usize));
                if width as u16 > *max_width {
                    *max_width = width as u16;
                }

                cell
            }))
        });

        let cfg = cx.editor.config();
        let picker_symbol = cfg.picker_symbol.as_str();
        let mut table = Table::new(options)
            .style(text_style)
            .highlight_style(selected)
            .highlight_symbol(picker_symbol)
            .column_spacing(1)
            .wrap_selected(self.wrap_selected)
            .widths(&self.widths);

        // -- Header
        if self.columns.len() > 1 {
            let active_column = self.query.active_column(self.prompt.position());
            let header_style = cx.editor.theme.get("ui.picker.header");
            let header_column_style = cx.editor.theme.get("ui.picker.header.column");

            table = table.header(
                Row::new(self.columns.iter().map(|column| {
                    if column.hidden {
                        Cell::default()
                    } else {
                        let style =
                            if active_column.is_some_and(|name| Arc::ptr_eq(name, &column.name)) {
                                cx.editor.theme.get("ui.picker.header.column.active")
                            } else {
                                header_column_style
                            };

                        Cell::from(Span::styled(Cow::from(&*column.name), style))
                    }
                }))
                .style(header_style),
            );
        }

        use tui::widgets::TableState;

        table.render_table(
            inner,
            surface,
            &mut TableState {
                offset: 0,
                selected: Some(cursor as usize),
            },
            self.truncate_start,
        );
    }

    fn render_preview(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // -- Render the frame:
        // clear area
        let background = cx.editor.theme.get("ui.background");
        let text = cx.editor.theme.get("ui.text");
        let directory = cx.editor.theme.get("ui.text.directory");
        // Only inside the border (see `clear_bordered`).
        crate::ui::clear_bordered(
            surface,
            area,
            background,
            !self.border_cells_take_background(&cx.editor.config()),
        );

        // calculate the inner area inside the box
        let base_inner = if self.docked.is_some() {
            // The quickfix panel's preview takes the place of the code, without a border.
            area
        } else if self.gradient_border_shown(&cx.editor.config()) {
            if self.gradient_border.is_none() {
                self.gradient_border = Some(GradientBorder::from_theme(
                    &cx.editor.theme,
                    &cx.editor.config().gradient_borders,
                ));
            }
            if let Some(ref mut gradient_border) = self.gradient_border {
                let rounded = cx.editor.config().border_rounded();
                if !cx.editor.config().borders_hidden() {
                    gradient_border.render(area, surface, &cx.editor.theme, rounded);
                }
            }
            // The gradient border is one cell wide whatever its thickness.
            crate::ui::gradient_border::GradientBorder::inner_area(area)
        } else {
            let block = Block::bordered()
                .border_type(crate::ui::border_type(&cx.editor.config()))
                .border_style(crate::ui::border_style(&cx.editor.theme));
            let inner_area = block.inner(area);
            block.render(area, surface);
            inner_area
        };
        // 1 column gap on either side
        let margin = Margin::horizontal(1);
        let inner = base_inner.inner(margin);

        // Reset the preview scroll on a selection change, before the `get_preview` borrow.
        if self.cursor != self.preview_scroll_cursor {
            self.preview_scroll_offset = 0;
            self.preview_scroll_cursor = self.cursor;
        }

        // `get_preview` borrows `self` for the block below, so the offset is read into a
        // local here, clamped against the soft-wrap layout inside, then written back after.
        let mut scroll = self.preview_scroll_offset;

        if let Some((preview, range)) = self.get_preview(cx.editor) {
            let doc = match preview.document() {
                Some(doc)
                    if range.is_none_or(|(start, end)| {
                        start <= end && end <= doc.text().len_lines()
                    }) =>
                {
                    doc
                }
                _ => {
                    if let Some(image) = preview.image() {
                        use super::image_preview::Status;
                        let message = match image.draw(surface, inner, cx.editor) {
                            Status::Shown => return,
                            Status::Loading => "Loading image…".to_string(),
                            Status::Failed(err) => format!("<{err}>"),
                        };
                        let x = inner.x
                            + inner.width.saturating_sub(message.chars().count() as u16) / 2;
                        let y = inner.y + inner.height / 2;
                        surface.set_stringn(x, y, &message, inner.width as usize, text);
                        return;
                    }

                    if let Some(dir_content) = preview.dir_content() {
                        for (i, (path, is_dir)) in
                            dir_content.iter().take(inner.height as usize).enumerate()
                        {
                            let style = if *is_dir { directory } else { text };
                            surface.set_stringn(
                                inner.x,
                                inner.y + i as u16,
                                path,
                                inner.width as usize,
                                style,
                            );
                        }
                        return;
                    }

                    let alt_text = preview.placeholder();
                    let x = inner.x + inner.width.saturating_sub(alt_text.len() as u16) / 2;
                    let y = inner.y + inner.height / 2;
                    surface.set_stringn(x, y, alt_text, inner.width as usize, text);
                    return;
                }
            };
            let doc_height = doc.text().len_lines();

            let mut offset = ViewPosition::default();
            if let Some((start_line, end_line)) = range {
                let height = end_line - start_line;
                let text = doc.text().slice(..);
                let start = text.line_to_char(start_line);
                let middle = text.line_to_char(start_line + height / 2);
                if height < inner.height as usize {
                    let text_fmt = doc.text_format(inner.width, None);
                    let annotations = TextAnnotations::default();
                    (offset.anchor, offset.vertical_offset) = char_idx_at_visual_offset(
                        text,
                        middle,
                        // align to middle
                        -(inner.height as isize / 2),
                        0,
                        &text_fmt,
                        &annotations,
                    );
                    if start < offset.anchor {
                        offset.anchor = start;
                        offset.vertical_offset = 0;
                    }
                } else {
                    offset.anchor = start;
                }
            }

            // Apply the preview scroll by moving the anchor by whole visual lines, so
            // soft-wrapped lines scroll one wrapped row at a time. `offset.anchor` is the
            // natural top of the preview (line 0, or the centred match for range previews)
            // and is left untouched when there is no scroll, keeping that centring. An
            // upward scroll is clamped to the start of the file by `char_idx_at_visual_offset`.
            let mut at_bottom = false;
            if scroll != 0 {
                let text = doc.text().slice(..);
                let text_fmt = doc.text_format(inner.width, None);
                let annotations = TextAnnotations::default();
                let natural_anchor = offset.anchor;

                let (anchor, vertical_offset) = char_idx_at_visual_offset(
                    text,
                    natural_anchor,
                    scroll,
                    0,
                    &text_fmt,
                    &annotations,
                );

                // The anchor that keeps the file's last visual line on the bottom row, so a
                // downward scroll can't run past the end into empty space. Found by walking
                // up one viewport from the end: a screenful of rows at most.
                let (max_anchor, _) = char_idx_at_visual_offset(
                    text,
                    text.len_chars().saturating_sub(1),
                    -(inner.height as isize - 1),
                    0,
                    &text_fmt,
                    &annotations,
                );

                if anchor >= max_anchor {
                    at_bottom = true;
                    offset.anchor = max_anchor;
                    offset.vertical_offset = 0;

                    // Scrolled past the bottom: clamp the stored offset to the scroll that
                    // exactly reaches it, so the offset can't accumulate and the next upward
                    // scroll responds at once. Measured between the in-range natural and
                    // bottom anchors (not the applied anchor, which may sit past EOF where it
                    // can't be measured), capped at the current offset.
                    if anchor > max_anchor {
                        let max_scroll = visual_offset_from_anchor(
                            text,
                            natural_anchor,
                            max_anchor,
                            &text_fmt,
                            &annotations,
                            scroll.unsigned_abs(),
                        )
                        .map_or(scroll, |(pos, _)| pos.row as isize);
                        scroll = scroll.min(max_scroll);
                    }
                } else {
                    offset.anchor = anchor;
                    offset.vertical_offset = vertical_offset;

                    // Past the top: `char_idx_at_visual_offset` already pinned the anchor to
                    // the start, so normalise a negative offset to the rows actually
                    // scrolled, keeping it from accumulating above the top.
                    if scroll < 0 && anchor <= natural_anchor {
                        scroll = visual_offset_from_anchor(
                            text,
                            anchor,
                            natural_anchor,
                            &text_fmt,
                            &annotations,
                            scroll.unsigned_abs(),
                        )
                        .map_or(scroll, |(pos, _)| -(pos.row as isize));
                    }
                }
            }

            let loader = cx.editor.syn_loader.load();
            let config = cx.editor.config();

            let syntax_highlighter = EditorView::doc_syntax_highlighter(
                doc,
                &TextAnnotations::default(),
                offset.anchor,
                area.height,
                &loader,
            );
            let annotations = TextAnnotations::default();
            let mut overlay_highlights = Vec::new();
            if doc
                .language_config()
                .and_then(|config| config.rainbow_brackets)
                .unwrap_or(config.rainbow_brackets)
            {
                if let Some(overlay) = EditorView::doc_rainbow_highlights(
                    doc,
                    &annotations,
                    offset.anchor,
                    area.height,
                    &cx.editor.theme,
                    &loader,
                ) {
                    overlay_highlights.push(overlay);
                }
            }

            EditorView::doc_diagnostics_highlights_into(
                doc,
                &cx.editor.theme,
                &mut overlay_highlights,
            );

            let mut decorations = DecorationManager::default();

            if let Some((start, end)) = range {
                let style = cx
                    .editor
                    .theme
                    .try_get("ui.highlight")
                    .unwrap_or_else(|| cx.editor.theme.get("ui.selection"));
                let draw_highlight = move |renderer: &mut TextRenderer, pos: LinePos| {
                    if (start..=end).contains(&pos.doc_line) {
                        let area = Rect::new(
                            renderer.viewport.x,
                            pos.visual_line,
                            renderer.viewport.width,
                            1,
                        );
                        renderer.set_style(area, style)
                    }
                };
                decorations.add_decoration(draw_highlight);
            }

            let current_line = doc.text().slice(..).char_to_line(offset.anchor);

            render_document(
                surface,
                inner,
                doc,
                offset,
                // TODO: compute text annotations asynchronously here (like inlay hints)
                &TextAnnotations::default(),
                syntax_highlighter,
                overlay_highlights,
                &cx.editor.theme,
                decorations,
            );

            // Scroll indicator on the right edge. The thumb is placed from document lines,
            // which is approximate when lines soft-wrap, so it is pinned to the end once the
            // preview is scrolled to the bottom and otherwise clamped within the track.
            let win_height = inner.height as usize;
            let scroll_style = cx.editor.theme.get("ui.menu.scroll");

            if doc_height > win_height {
                let scroll_height = win_height.pow(2).div_ceil(doc_height).min(win_height);
                let track = win_height - scroll_height;
                let scroll_line = if at_bottom {
                    track
                } else {
                    (track * current_line / std::cmp::max(1, doc_height.saturating_sub(win_height)))
                        .min(track)
                };

                let mut cell;
                for i in 0..win_height {
                    cell = &mut surface[(inner.right() - 1, inner.top() + i as u16)];
                    cell.set_symbol("▐");

                    if scroll_line <= i && i < scroll_line + scroll_height {
                        // thumb
                        cell.set_fg(scroll_style.fg.unwrap_or(helix_view::theme::Color::Reset));
                    } else {
                        // track
                        cell.set_fg(scroll_style.bg.unwrap_or(helix_view::theme::Color::Reset));
                    }
                }
            }
        }

        // Persist the offset clamped against the soft-wrap layout above.
        self.preview_scroll_offset = scroll;
    }
}

impl<I: 'static + Send + Sync, D: 'static + Send + Sync> Component for Picker<I, D> {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        if self.docked.is_some() {
            return self.render_docked(area, surface, cx);
        }
        // default render
        // +--title--+ +---------+
        // +---------+ +---------+
        // |prompt   | |preview  |
        // +---------+ |         |
        // |picker   | |         |
        // |         | |         |
        // +---------+ +---------+
        //
        // stack vertically
        // +---------+
        // |prompt   |
        // +---------+
        // |picker   |
        // |         |
        // +---------+
        // |preview  |
        // |         |
        // |         |
        // +---------+

        let render_preview = self.show_preview
            && self.file_fn.is_some()
            && area.width >= MIN_AREA_WIDTH_FOR_PREVIEW
            && area.height >= MIN_AREA_HEIGHT_FOR_PREVIEW;
        let stack_vertically = area.width / 2 < MIN_AREA_WIDTH_FOR_PREVIEW;

        let picker_area = if render_preview {
            if stack_vertically {
                area.with_height(area.height / 3)
            } else {
                area.with_width(area.width / 2)
            }
        } else {
            area
        };

        self.render_picker(picker_area, surface, cx);

        if render_preview {
            let preview_area = if stack_vertically {
                area.clip_top(picker_area.height)
            } else {
                area.clip_left(picker_area.width)
            };
            self.render_preview(preview_area, surface, cx);
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        if let Some(tracker) = &self.state_tracker {
            if let Ok(mut state) = tracker.lock() {
                state.query = self.prompt.line().clone();
                state.cursor = self.cursor;
            }
        }
        if self.docked.is_some() {
            return self.handle_docked_event(event, ctx);
        }
        let key_event = match event {
            Event::Key(event) => *event,
            Event::Paste(..) => return self.prompt_handle_event(event, ctx),
            Event::Resize(..) => return EventResult::Consumed(None),
            // Picker is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Ignored(None),
        };
        let Some(key_event) = super::component_key(
            ctx.editor,
            helix_view::component_keys::Component::Picker,
            key_event,
        ) else {
            return EventResult::Consumed(None);
        };

        let close_fn = |picker: &mut Self| {
            // A closed picker no longer follows the editor (this also stops its file watcher).
            picker.live = None;
            // Nor keeps its previews (whole documents) while it waits to be reopened.
            picker.preview_cache.clear();
            picker.preview_cache_order.clear();
            // if the picker is very large don't store it as last_picker to avoid
            // excessive memory consumption
            let callback: compositor::Callback = if picker.matcher.snapshot().item_count() > 100_000
            {
                Box::new(|compositor: &mut Compositor, _ctx| {
                    // remove the layer
                    compositor.pop();
                })
            } else {
                // stop streaming in new items in the background, really we should
                // be restarting the stream somehow once the picker gets
                // reopened instead (like for an FS crawl) that would also remove the
                // need for the special case above but that is pretty tricky
                picker.version.fetch_add(1, atomic::Ordering::Relaxed);
                Box::new(|compositor: &mut Compositor, _ctx| {
                    // remove the layer
                    compositor.last_picker = compositor.pop();
                })
            };
            EventResult::Consumed(Some(callback))
        };

        // Navigating cancels restoring the selection after a refresh.
        if let Some(live) = &mut self.live {
            live.reselect = None;
        }

        // handle custom keybindings, if exist
        if let EventResult::Consumed(_) = self.custom_key_event_handler(&key_event, ctx) {
            return EventResult::Consumed(None);
        }

        // Replace action: reveal/switch the replacement input, apply it.
        if self.replace_fn.is_some() {
            match key_event {
                alt!('r') => {
                    self.toggle_replace_focus();
                    return EventResult::Consumed(None);
                }
                ctrl!(Enter) | alt!('a') if self.replace.is_some() => {
                    self.run_replace(ctx, true);
                    return EventResult::Consumed(None);
                }
                key!(Enter) if self.replace.is_some() => {
                    self.run_replace(ctx, false);
                    return EventResult::Consumed(None);
                }
                _ => {}
            }
        }

        match key_event {
            shift!(Tab) | key!(Up) | ctrl!('p') => {
                self.move_by(1, Direction::Backward);
            }
            key!(Tab) | key!(Down) | ctrl!('n') => {
                self.move_by(1, Direction::Forward);
            }
            // Ctrl-d/Ctrl-u always page the result list. PageDown/PageUp scroll the
            // preview a full page when one is shown, and otherwise page the list.
            ctrl!('d') => {
                self.page_down();
            }
            ctrl!('u') => {
                self.page_up();
            }
            key!(PageDown) if self.preview_shown() => {
                self.scroll_preview(self.preview_height as isize);
            }
            key!(PageUp) if self.preview_shown() => {
                self.scroll_preview(-(self.preview_height as isize));
            }
            key!(PageDown) => {
                self.page_down();
            }
            key!(PageUp) => {
                self.page_up();
            }
            key!(Home) => {
                self.to_start();
            }
            key!(End) => {
                self.to_end();
            }
            key!(Esc) | ctrl!('c') => return close_fn(self),
            alt!(Enter) => {
                if let Some(option) = self.selection() {
                    (self.callback_fn)(ctx, option, self.default_action);
                }
            }
            key!(Enter) => {
                // If the prompt has a history completion and is empty, use enter to accept
                // that completion
                if let Some(completion) = self
                    .prompt
                    .first_history_completion(ctx.editor)
                    .filter(|_| self.prompt.line().is_empty())
                {
                    // The percent character is used by the query language and needs to be
                    // escaped with a backslash.
                    let completion = if completion.contains('%') {
                        completion.replace('%', "\\%")
                    } else {
                        completion.into_owned()
                    };
                    self.prompt.set_line(completion, ctx.editor);

                    // Inserting from the history register is a paste.
                    self.handle_prompt_change(true);
                } else {
                    if let Some(option) = self.selection() {
                        (self.callback_fn)(ctx, option, self.default_action);
                    }
                    if let Some(history_register) = self.prompt.history_register() {
                        if let Err(err) = ctx
                            .editor
                            .registers
                            .push(history_register, self.primary_query().to_string())
                        {
                            ctx.editor.set_error(err.to_string());
                        }
                    }
                    return close_fn(self);
                }
            }
            ctrl!('s') => {
                if let Some(option) = self.selection() {
                    (self.callback_fn)(ctx, option, Action::HorizontalSplit);
                }
                return close_fn(self);
            }
            ctrl!('v') => {
                if let Some(option) = self.selection() {
                    (self.callback_fn)(ctx, option, Action::VerticalSplit);
                }
                return close_fn(self);
            }
            ctrl!('t') => {
                self.toggle_preview();
            }
            // Copy the matched locations into the quicklist.
            ctrl!('q') => {
                self.copy_to_quicklist(ctx.editor);
            }
            // Preview line scrolling. Alt-d/f/b are intentionally avoided here: the prompt
            // keybinds (which also apply in pickers) use them for word editing in the
            // query, and full-page preview scrolling is already on PageUp/PageDown.
            alt!('k') | shift!(Up) if self.preview_shown() => {
                let lines = ctx.editor.config().scroll_lines.unsigned_abs() as isize;
                self.scroll_preview(-lines);
            }
            alt!('j') | shift!(Down) if self.preview_shown() => {
                let lines = ctx.editor.config().scroll_lines.unsigned_abs() as isize;
                self.scroll_preview(lines);
            }
            _ => {
                self.prompt_handle_event(event, ctx);
            }
        }

        EventResult::Consumed(None)
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        if self.docked.is_some() {
            return self.docked_cursor(area, editor);
        }
        // calculate the inner area inside the box
        let inner = if editor.config().gradient_borders.enable {
            // The gradient border is one cell wide whatever its thickness.
            crate::ui::gradient_border::GradientBorder::inner_area(area)
        } else {
            let block = Block::bordered();
            block.inner(area)
        };

        // prompt area
        let render_preview = self.show_preview
            && self.file_fn.is_some()
            && area.width >= MIN_AREA_WIDTH_FOR_PREVIEW
            && area.height >= MIN_AREA_HEIGHT_FOR_PREVIEW;
        let stack_vertically = area.width / 2 < MIN_AREA_WIDTH_FOR_PREVIEW;

        let picker_width = if render_preview && !stack_vertically {
            inner.width / 2
        } else {
            inner.width
        };
        let area = inner.clip_left(1).with_height(1).with_width(picker_width);

        if let Some(replace) = self.replace.as_ref().filter(|replace| replace.focused) {
            let label_width = REPLACE_LABEL.len() as u16;
            let row = Rect::new(
                area.x + label_width,
                area.y + 1,
                area.width.saturating_sub(label_width),
                1,
            );
            return replace.prompt.cursor(row, editor);
        }

        self.prompt.cursor(area, editor)
    }

    fn required_size(&mut self, (width, height): (u16, u16)) -> Option<(u16, u16)> {
        self.completion_height =
            height.saturating_sub(4 + self.header_height() + u16::from(self.replace.is_some()));
        self.preview_height = height.saturating_sub(2);
        Some((width, height))
    }

    fn id(&self) -> Option<&'static str> {
        if self.docked.is_some() {
            Some(ui::quickfix::ID)
        } else {
            Some(ID)
        }
    }

    fn quickfix_step(&mut self, forward: bool, count: usize, cx: &mut Context) -> bool {
        if self.docked.is_none() {
            return false;
        }
        let direction = if forward {
            Direction::Forward
        } else {
            Direction::Backward
        };
        self.move_by(count.max(1).min(u32::MAX as usize) as u32, direction);
        match self.selection() {
            Some(option) => {
                (self.callback_fn)(cx, option, Action::Replace);
                let total = self.matcher.snapshot().matched_item_count();
                cx.editor
                    .set_status(format!("Quickfix {}/{total}", self.cursor + 1));
            }
            None => cx.editor.set_status("The quickfix list is empty"),
        }
        true
    }
}

/// The docked mode: the quickfix panel.
impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    /// The picker as the quickfix panel (see `ui::quickfix`): drawn at the bottom of the
    /// views and kept open while its entries are visited. A list that needs a query to show
    /// anything (a global search) starts with the query to type; the others with the list.
    /// The data the columns are formatted with.
    pub fn editor_data(&self) -> &D {
        &self.editor_data
    }

    pub fn docked(mut self) -> Self {
        let filtering = self.dynamic_query_handler.is_some() && self.prompt.line().is_empty();
        self.docked = Some(Docked {
            filtering,
            ..Default::default()
        });
        self
    }

    /// Whether the borders are gradients (`gradient-borders`): the quickfix panel (and its
    /// preview over the code), a part of the editor's frame rather than a window over it, has
    /// no border at all.
    fn gradient_border_shown(&self, config: &helix_view::editor::Config) -> bool {
        config.gradient_borders.enable && self.docked.is_none()
    }

    /// See `ui::border_cells_take_background`, for the borders this picker draws.
    fn border_cells_take_background(&self, config: &helix_view::editor::Config) -> bool {
        if self.docked.is_some() {
            // no border: the whole area is the panel's
            true
        } else {
            crate::ui::border_cells_take_background(config)
        }
    }

    /// Whether the keys edit the query (or the replacement) rather than move in the list.
    fn docked_typing(&self) -> bool {
        self.docked.as_ref().is_some_and(|docked| docked.filtering)
            || self.replace.as_ref().is_some_and(|replace| replace.focused)
    }

    fn render_docked(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let frame = ui::editor::frame(cx.editor, area);
        let panel = ui::quickfix::panel_area(cx.editor, frame);
        if let Some(docked) = &mut self.docked {
            docked.area = panel;
        }
        if panel.height == 0 || panel.width == 0 {
            return;
        }
        self.render_picker(panel, surface, cx);

        // With the focus, the selected location over the code.
        let config = cx.editor.config();
        if ui::quickfix::focused(cx.editor)
            && config.quickfix.preview
            && self.show_preview
            && self.file_fn.is_some()
        {
            // The statusline above the panel stays.
            let views = ui::quickfix::layout(cx.editor, frame).clip_bottom(1);
            if views.width >= 10 && views.height >= 3 {
                self.preview_height = views.height.saturating_sub(2);
                self.render_preview(views, surface, cx);
            }
        }
    }

    /// The rows of the list in the panel `area`, and the index of the item on its first row.
    fn docked_list(&self, area: Rect) -> (Rect, u32) {
        // no border: the rule, the query (and the replacement), the list
        let inner = area;
        let replace_rows = u16::from(self.replace.is_some() && inner.height > 4);
        let list = inner
            .clip_top(2 + replace_rows)
            .clip_top(self.header_height());
        let rows = list.height.max(1) as u32;
        (list, self.cursor - (self.cursor % rows))
    }

    fn docked_cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        if !ui::quickfix::focused(editor) {
            return (None, CursorKind::Hidden);
        }
        let panel = ui::quickfix::panel_area(editor, ui::editor::frame(editor, area));
        if !self.docked_typing() {
            // The list has the focus: no cursor (nor the editor's, see `Compositor::render`).
            return (
                Some(Position::new(panel.y as usize, panel.x as usize)),
                CursorKind::Hidden,
            );
        }
        // no border: the query is on the row under the rule
        let line = panel.clip_left(1).clip_top(1).with_height(1);
        if let Some(replace) = self.replace.as_ref().filter(|replace| replace.focused) {
            let label_width = REPLACE_LABEL.len() as u16;
            let row = Rect::new(
                line.x + label_width,
                line.y + 1,
                line.width.saturating_sub(label_width),
                1,
            );
            return replace.prompt.cursor(row, editor);
        }
        self.prompt.cursor(line, editor)
    }

    /// Opens the selected location with `action`; the code gets the focus unless `keep_focus`.
    fn docked_open(&mut self, cx: &mut Context, action: Action, keep_focus: bool) {
        if let Some(option) = self.selection() {
            (self.callback_fn)(cx, option, action);
        }
        if !keep_focus {
            cx.editor.quickfix_panel.focused = false;
        }
    }

    /// Closes the panel, which `:copen` can show again.
    fn docked_close(&mut self) -> EventResult {
        // A closed list no longer follows the editor, nor keeps its previews.
        self.live = None;
        self.preview_cache.clear();
        self.preview_cache_order.clear();
        self.version.fetch_add(1, atomic::Ordering::Relaxed);
        if let Some(docked) = &mut self.docked {
            docked.filtering = false;
            docked.window_key = false;
        }
        EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, cx| {
            ui::quickfix::close(compositor, cx.editor)
        })))
    }

    fn handle_docked_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        use helix_view::input::{MouseButton, MouseEventKind};

        let area = self
            .docked
            .as_ref()
            .map(|docked| docked.area)
            .unwrap_or_default();
        let key_event = match event {
            Event::Mouse(mouse) => {
                let inside = (area.left()..area.right()).contains(&mouse.column)
                    && (area.top()..area.bottom()).contains(&mouse.row);
                if !inside {
                    // A click elsewhere gives the focus back to what is clicked.
                    if matches!(mouse.kind, MouseEventKind::Down(_)) {
                        cx.editor.quickfix_panel.focused = false;
                    }
                    return EventResult::Ignored(None);
                }
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        ui::quickfix::focus(cx.editor);
                        let (list, offset) = self.docked_list(area);
                        if mouse.row >= list.y && mouse.row < list.bottom() {
                            let index = offset + (mouse.row - list.y) as u32;
                            if index < self.matcher.snapshot().matched_item_count() {
                                self.cursor = index;
                            }
                        }
                    }
                    MouseEventKind::ScrollDown => self.move_by(1, Direction::Forward),
                    MouseEventKind::ScrollUp => self.move_by(1, Direction::Backward),
                    _ => {}
                }
                return EventResult::Consumed(None);
            }
            // The editor (under the panel) follows the size of the screen.
            Event::Resize(..) => return EventResult::Ignored(None),
            _ if !ui::quickfix::focused(cx.editor) => return EventResult::Ignored(None),
            Event::Paste(..) if self.docked_typing() => return self.prompt_handle_event(event, cx),
            Event::Paste(..) => return EventResult::Consumed(None),
            Event::Key(key) => *key,
            _ => return EventResult::Ignored(None),
        };
        let Some(key_event) = super::component_key(
            cx.editor,
            helix_view::component_keys::Component::Picker,
            key_event,
        ) else {
            return EventResult::Consumed(None);
        };

        // Navigating cancels restoring the selection after a refresh.
        if let Some(live) = &mut self.live {
            live.reselect = None;
        }

        // `Ctrl-w` then a window key: back to the code.
        if let Some(docked) = self.docked.as_mut().filter(|docked| docked.window_key) {
            docked.window_key = false;
            if matches!(
                key_event,
                key!('k') | key!(Up) | ctrl!('k') | key!('w') | ctrl!('w') | key!('p') | ctrl!('p')
            ) {
                cx.editor.quickfix_panel.focused = false;
            }
            return EventResult::Consumed(None);
        }

        // The replacement of a global search, as in the picker.
        if self.replace_fn.is_some() {
            match key_event {
                alt!('r') => {
                    self.toggle_replace_focus();
                    return EventResult::Consumed(None);
                }
                ctrl!(Enter) | alt!('a') if self.replace.is_some() => {
                    self.run_replace(cx, true);
                    return EventResult::Consumed(None);
                }
                key!(Enter) if self.replace.as_ref().is_some_and(|replace| replace.focused) => {
                    self.run_replace(cx, false);
                    return EventResult::Consumed(None);
                }
                _ => {}
            }
        }

        // Moving through the list, whatever has the keys.
        match key_event {
            key!(Down) | ctrl!('n') | key!(Tab) => {
                self.move_by(1, Direction::Forward);
                return EventResult::Consumed(None);
            }
            key!(Up) | ctrl!('p') | shift!(Tab) => {
                self.move_by(1, Direction::Backward);
                return EventResult::Consumed(None);
            }
            ctrl!('d') | key!(PageDown) => {
                self.page_down();
                return EventResult::Consumed(None);
            }
            ctrl!('u') | key!(PageUp) => {
                self.page_up();
                return EventResult::Consumed(None);
            }
            _ => {}
        }

        if self.docked_typing() {
            match key_event {
                // done typing: the keys move through the list again
                key!(Esc) | key!(Enter) => {
                    let replacing = self.replace.as_ref().is_some_and(|replace| replace.focused);
                    if replacing {
                        self.toggle_replace_focus();
                    } else if key_event == key!(Enter)
                        && self.prompt.line().is_empty()
                        && self.prompt.first_history_completion(cx.editor).is_some()
                    {
                        // The last query, as in the picker.
                        let completion = self
                            .prompt
                            .first_history_completion(cx.editor)
                            .unwrap()
                            .replace('%', "\\%");
                        self.prompt.set_line(completion, cx.editor);
                        self.handle_prompt_change(true);
                    } else if let Some(docked) = &mut self.docked {
                        docked.filtering = false;
                        if let Some(register) = self.prompt.history_register() {
                            let query = self.primary_query().to_string();
                            if !query.is_empty() {
                                if let Err(err) = cx.editor.registers.push(register, query) {
                                    cx.editor.set_error(err.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {
                    self.prompt_handle_event(event, cx);
                }
            }
            return EventResult::Consumed(None);
        }

        if let EventResult::Consumed(_) = self.custom_key_event_handler(&key_event, cx) {
            return EventResult::Consumed(None);
        }

        match key_event {
            key!('j') => self.move_by(1, Direction::Forward),
            key!('k') => self.move_by(1, Direction::Backward),
            key!('g') | key!(Home) => self.to_start(),
            key!('G') | key!(End) => self.to_end(),
            key!(Enter) => self.docked_open(cx, self.default_action, false),
            // open, keeping the focus in the list
            alt!(Enter) | key!('o') => self.docked_open(cx, self.default_action, true),
            ctrl!('s') => self.docked_open(cx, Action::HorizontalSplit, false),
            ctrl!('v') => self.docked_open(cx, Action::VerticalSplit, false),
            key!('/') => {
                if let Some(docked) = &mut self.docked {
                    docked.filtering = true;
                }
            }
            ctrl!('w') => {
                if let Some(docked) = &mut self.docked {
                    docked.window_key = true;
                }
            }
            ctrl!('t') => self.toggle_preview(),
            ctrl!('q') => self.copy_to_quicklist(cx.editor),
            key!('q') | key!(Esc) | ctrl!('c') => return self.docked_close(),
            // The command line, from the list too.
            key!(':') => return EventResult::Ignored(None),
            _ => {}
        }
        EventResult::Consumed(None)
    }
}

impl<T: 'static + Send + Sync, D> Drop for Picker<T, D> {
    fn drop(&mut self) {
        // ensure we cancel any ongoing background threads streaming into the picker
        self.version.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

/// The open picker with items `T` and data `D`, shown in an overlay (most pickers) or on its
/// own (some debugger pickers).
pub(super) fn find_picker<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    compositor: &mut Compositor,
) -> Option<&mut Picker<T, D>> {
    if compositor
        .find::<ui::overlay::Overlay<Picker<T, D>>>()
        .is_some()
    {
        return compositor
            .find::<ui::overlay::Overlay<Picker<T, D>>>()
            .map(|overlay| &mut overlay.content);
    }
    compositor.find::<Picker<T, D>>()
}

type PickerCallback<T> = Box<dyn Fn(&mut Context, &T, Action)>;
pub type PickerKeyHandler<T, D> = Box<dyn Fn(&mut Context, &T, Arc<D>, u32) + 'static>;
pub type PickerKeyHandlers<T, D> = HashMap<KeyEvent, PickerKeyHandler<T, D>>;

#[cfg(test)]
mod tests {
    use super::*;

    // ===========================================
    // Picker Title Tests
    // ===========================================

    #[test]
    fn test_picker_title_default_none() {
        // Create a simple picker and verify title is None by default
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["item1".to_string(), "item2".to_string()],
            (),
            |_cx, _item, _action| {},
        );

        assert!(picker.title.is_none());
    }

    #[test]
    fn test_picker_with_title() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["item1".to_string()],
            (),
            |_cx, _item, _action| {},
        )
        .with_title("Test Title");

        assert_eq!(picker.title.as_deref(), Some("Test Title"));
    }

    #[test]
    fn test_picker_title_chaining() {
        // Test that with_title can be chained with other builder methods
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["item1".to_string()],
            (),
            |_cx, _item, _action| {},
        )
        .truncate_start(false)
        .with_title("Chained Title")
        .show_preview(false);

        assert_eq!(picker.title.as_deref(), Some("Chained Title"));
        assert!(!picker.truncate_start);
        assert!(!picker.show_preview);
    }

    #[test]
    fn test_picker_title_empty_string() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {}).with_title("");

        // Empty string is still Some("")
        assert_eq!(picker.title.as_deref(), Some(""));
    }

    #[test]
    fn test_picker_title_with_special_characters() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {})
                .with_title("Files 📁 & Folders");

        assert_eq!(picker.title.as_deref(), Some("Files 📁 & Folders"));
    }

    #[test]
    fn test_picker_title_various_picker_types() {
        // Test with different item types to ensure generics work correctly

        // String items
        let columns_str = [Column::new("name", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker_str: Picker<String, ()> = Picker::new(
            columns_str,
            0,
            vec!["a".to_string()],
            (),
            |_cx, _item, _action| {},
        )
        .with_title("String Picker");
        assert_eq!(picker_str.title.as_deref(), Some("String Picker"));

        // Integer items
        let columns_int = [Column::new("num", |item: &i32, _: &()| {
            item.to_string().into()
        })];
        let picker_int: Picker<i32, ()> =
            Picker::new(columns_int, 0, vec![1, 2, 3], (), |_cx, _item, _action| {})
                .with_title("Integer Picker");
        assert_eq!(picker_int.title.as_deref(), Some("Integer Picker"));
    }

    // ===========================================
    // Picker Constants Tests
    // ===========================================

    #[test]
    fn test_picker_id_constant() {
        assert_eq!(ID, "picker");
    }

    #[test]
    fn test_min_area_constants() {
        assert_eq!(MIN_AREA_WIDTH_FOR_PREVIEW, 72);
        assert_eq!(MIN_AREA_HEIGHT_FOR_PREVIEW, 24);
    }

    #[test]
    fn test_max_file_size_for_preview() {
        // 10 MB
        assert_eq!(MAX_FILE_SIZE_FOR_PREVIEW, 10 * 1024 * 1024);
    }

    // ===========================================
    // Picker Movement Tests
    // ===========================================

    #[test]
    fn test_picker_cursor_starts_at_zero() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            (),
            |_cx, _item, _action| {},
        );

        assert_eq!(picker.cursor, 0);
    }

    // ===========================================
    // Preview Scroll Tests
    // ===========================================

    #[test]
    fn test_scroll_preview_default_zero() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        assert_eq!(picker.preview_scroll_offset, 0);
    }

    #[test]
    fn test_scroll_preview_positive() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(5);
        assert_eq!(picker.preview_scroll_offset, 5);
    }

    #[test]
    fn test_scroll_preview_negative() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(-3);
        assert_eq!(picker.preview_scroll_offset, -3);
    }

    #[test]
    fn test_scroll_preview_accumulates() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(10);
        picker.scroll_preview(-3);
        assert_eq!(picker.preview_scroll_offset, 7);
    }

    #[test]
    fn test_scroll_preview_saturation_positive() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(isize::MAX);
        picker.scroll_preview(1);
        // Should not overflow, should stay at isize::MAX
        assert_eq!(picker.preview_scroll_offset, isize::MAX);
    }

    #[test]
    fn test_scroll_preview_saturation_negative() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(isize::MIN);
        picker.scroll_preview(-1);
        // Should not underflow, should stay at isize::MIN
        assert_eq!(picker.preview_scroll_offset, isize::MIN);
    }

    #[test]
    fn test_scroll_preview_resets_to_zero() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        picker.scroll_preview(42);
        assert_eq!(picker.preview_scroll_offset, 42);
        picker.preview_scroll_offset = 0;
        assert_eq!(picker.preview_scroll_offset, 0);
    }

    #[test]
    fn test_preview_shown_no_file_fn() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        // Default show_preview is true, but file_fn is None
        assert!(!picker.preview_shown());
    }

    #[test]
    fn test_preview_shown_with_file_fn() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {})
                .with_preview(|_editor, _item| None);
        assert!(picker.preview_shown());
    }

    #[test]
    fn test_preview_shown_toggled_off() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["a".to_string(), "b".to_string()],
            (),
            |_cx, _item, _action| {},
        )
        .with_preview(|_editor, _item| None)
        .show_preview(false);
        assert!(!picker.preview_shown());
    }

    #[test]
    fn test_preview_scroll_cursor_tracks_selection() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["a".to_string(), "b".to_string()],
            (),
            |_cx, _item, _action| {},
        );
        // Initially cursor and preview_scroll_cursor are both 0
        assert_eq!(picker.cursor, 0);
        assert_eq!(picker.preview_scroll_cursor, 0);

        // Set a scroll offset
        picker.scroll_preview(10);
        assert_eq!(picker.preview_scroll_offset, 10);

        // Change cursor (simulating selection change)
        picker.cursor = 1;

        // The render_preview method resets scroll when cursor != preview_scroll_cursor
        // We simulate the check here (same logic as in render_preview)
        if picker.cursor != picker.preview_scroll_cursor {
            picker.preview_scroll_offset = 0;
            picker.preview_scroll_cursor = picker.cursor;
        }

        assert_eq!(picker.preview_scroll_offset, 0);
        assert_eq!(picker.preview_scroll_cursor, 1);
    }

    #[test]
    fn test_preview_scroll_cursor_no_reset_on_same_selection() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> = Picker::new(
            columns,
            0,
            vec!["a".to_string(), "b".to_string()],
            (),
            |_cx, _item, _action| {},
        );
        picker.preview_scroll_cursor = 0;
        picker.scroll_preview(10);

        // Cursor hasn't changed, scroll should not be reset
        if picker.cursor != picker.preview_scroll_cursor {
            picker.preview_scroll_offset = 0;
            picker.preview_scroll_cursor = picker.cursor;
        }

        assert_eq!(picker.preview_scroll_offset, 10);
    }

    #[test]
    fn test_required_size_sets_preview_height() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let mut picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});

        assert_eq!(picker.preview_height, 0);

        picker.required_size((100, 50));
        // preview_height = height - 2 (for borders) = 48
        assert_eq!(picker.preview_height, 48);
    }

    #[test]
    fn test_preview_scroll_cursor_initial_value() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        assert_eq!(picker.preview_scroll_cursor, 0);
    }

    #[test]
    fn with_prompt_labels_the_query_and_keeps_the_history() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {})
                .with_history_register(Some('x'))
                .with_prompt("Choose:");
        assert_eq!(picker.prompt.prompt(), "Choose:");
        assert_eq!(picker.prompt.history_register(), Some('x'));
    }

    #[test]
    fn the_default_action_replaces_unless_told_otherwise() {
        let columns = [Column::new("test", |item: &String, _: &()| {
            item.as_str().into()
        })];
        let picker: Picker<String, ()> =
            Picker::new(columns, 0, vec![], (), |_cx, _item, _action| {});
        assert!(matches!(picker.default_action, Action::Replace));
        let picker = picker.with_default_action(Action::VerticalSplit);
        assert!(matches!(picker.default_action, Action::VerticalSplit));
    }
}
