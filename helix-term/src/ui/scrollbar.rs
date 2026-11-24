//! Draws the scrollbar of a view (`[editor.scrollbar]`, after nvim-scrollbar) in the column
//! the view keeps for it on its right (see `View::scrollbar_offset`): the handle beside the
//! lines on screen, and the marks of the cursor, search matches, diagnostics, git changes and
//! plugins. What each row shows is worked out in `helix_view::scrollbar`.
//!
//! The search marks need the whole buffer searched: that runs in the background, at most
//! once per `throttle-ms`, and the bar is drawn again when it is done.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helix_core::diagnostic::Severity;
use helix_stdx::rope::{self, RopeSliceExt};
use helix_view::editor::ScrollbarConfig;
use helix_view::graphics::{Color, Modifier, Rect, Style};
use helix_view::scrollbar::{line_count, Layout, Mark, MarkType, RowMark, Rows, EIGHTHS};
use helix_view::{Document, DocumentId, Editor, Theme, View, ViewId};
use tui::buffer::Buffer as Surface;

/// The most search matches marked (a pattern matching everything marks every line anyway).
const MAX_SEARCH_MATCHES: usize = 100_000;

/// Blocks as tall as eighths of a cell, from the bottom: none to all of it.
const LOWER_BLOCKS: [&str; EIGHTHS + 1] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// The scrollbars' state: the matches of the last search, per buffer, and the last line on
/// the screen of each view.
#[derive(Default)]
pub struct Scrollbars {
    search: Arc<Mutex<HashMap<DocumentId, SearchMatches>>>,
    last_lines: Mutex<HashMap<ViewId, (ScreenKey, usize)>>,
}

/// What the last line on a view's screen depends on: the buffer and its version, where the
/// view starts and its size, its folds, and what takes rows of its own (soft wrap, the
/// diagnostics' virtual lines). Moving the cursor without scrolling changes none of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenKey {
    doc: DocumentId,
    version: i32,
    anchor: usize,
    vertical_offset: usize,
    width: u16,
    height: u16,
    folds: (usize, usize),
    diagnostics: usize,
    soft_wrap: bool,
}

/// The matches of a search of a buffer, and the search running for them.
struct SearchMatches {
    /// What was searched for.
    query: Query,
    /// Its regex (also to tell whether the cursor is on a match).
    regex: rope::Regex,
    /// The version of the buffer searched (`None`: none yet), and the line of each match.
    version: Option<i32>,
    lines: Arc<Vec<usize>>,
    /// When the last search started, and whether one is running.
    started: Option<Instant>,
    running: bool,
    /// A redraw is asked for once the throttle lets the next search run.
    waking: bool,
}

/// The last search: its pattern, as `n` searches for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    pattern: String,
    case_insensitive: bool,
    crlf: bool,
}

impl Query {
    fn regex(&self) -> Option<rope::Regex> {
        rope::RegexBuilder::new()
            .syntax(
                rope::Config::new()
                    .case_insensitive(self.case_insensitive)
                    .multi_line(true)
                    .crlf(self.crlf),
            )
            .build(&self.pattern)
            .ok()
    }
}

/// A handle held with the mouse: its view, where it was pressed (in eighths of rows) and the
/// first line on screen then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub view: ViewId,
    at: usize,
    first: usize,
}

/// Where a view's bar is and what it covers.
struct Geometry {
    /// The view's text area: the bar is in the column right of it.
    inner: Rect,
    layout: Layout,
    /// The buffer's lines, and the first and last on screen.
    lines: usize,
    first: usize,
    last: usize,
    /// The handle, in eighths of rows.
    handle: (usize, usize),
}

impl Scrollbars {
    /// Where the bar of `view` is, if it has one.
    fn geometry(&self, editor: &Editor, doc: &Document, view: &View) -> Option<Geometry> {
        let config = editor.config();
        let bar = &config.scrollbar;
        let inner = view.inner_area(doc);
        if view.scrollbar_offset(doc) == 0 || inner.height == 0 {
            return None;
        }
        let text = doc.text().slice(..);
        let lines = line_count(text);
        let offset = doc.view_offset(view.id);
        let first = text
            .char_to_line(offset.anchor.min(text.len_chars()))
            .min(lines - 1);
        self.forget_closed_views(editor);
        let last = self
            .last_line_on_screen(doc, view, inner)
            .clamp(first, lines - 1);
        let folds: Vec<(usize, usize)> = match doc.fold_container(view.id) {
            Some(container) if bar.folds > 0 && lines <= bar.folds => container
                .start_points()
                .iter()
                .filter(|start| start.is_superest())
                .map(|start| {
                    let fold = start.fold(container);
                    (fold.start.line, fold.end.line)
                })
                .collect(),
            _ => Vec::new(),
        };
        let layout = Layout::new(inner.height, lines, folds);
        let handle = layout.handle(first, last);
        Some(Geometry {
            inner,
            layout,
            lines,
            first,
            last,
            handle,
        })
    }

    /// Draws the scrollbar of `view`, if it has one, beside its text area `inner`.
    pub fn render(
        &self,
        editor: &Editor,
        doc: &Document,
        view: &View,
        inner: Rect,
        surface: &mut Surface,
        is_focused: bool,
    ) {
        let config = editor.config();
        let bar = &config.scrollbar;
        if bar.show_in_active_only && !is_focused {
            return;
        }
        let Some(Geometry {
            inner: bar_inner,
            layout,
            lines,
            first,
            last,
            handle: (handle_top, handle_bottom),
        }) = self.geometry(editor, doc, view)
        else {
            return;
        };
        debug_assert_eq!(bar_inner, inner);
        let x = inner.right();
        if x >= surface.area.right() {
            return;
        }
        let theme = &editor.theme;
        let text = doc.text().slice(..);

        let all_visible = first == 0 && last + 1 >= lines;
        if all_visible && bar.hide_if_all_visible {
            return;
        }
        let show_handle = bar.handlers.handle && !(all_visible && bar.handle.hide_if_all_visible);

        let mut rows = Rows::new(&layout, bar);
        if bar.handlers.cursor {
            let cursor = doc.selection(view.id).primary().cursor(text);
            rows.add(text.char_to_line(cursor), &Mark::new(MarkType::Cursor));
        }
        if bar.handlers.search {
            if let Some(matches) = self.search_lines(editor, doc, view) {
                let mark = Mark::new(MarkType::Search);
                for &line in matches.iter() {
                    rows.add(line, &mark);
                }
            }
        }
        if bar.handlers.diagnostic {
            for diagnostic in doc.diagnostics() {
                let kind = match diagnostic.severity {
                    Some(Severity::Error) => MarkType::Error,
                    Some(Severity::Warning) | None => MarkType::Warn,
                    Some(Severity::Info) => MarkType::Info,
                    Some(Severity::Hint) => MarkType::Hint,
                };
                rows.add(diagnostic.line, &Mark::new(kind));
            }
        }
        if bar.handlers.git {
            add_git_marks(doc, lines, &mut rows);
        }
        if bar.handlers.plugins {
            for plugin_mark in &doc.plugin_scrollbar_marks {
                let mut mark = Mark::new(plugin_mark.kind);
                mark.level = plugin_mark.level;
                mark.text = plugin_mark.text.clone();
                mark.scope = plugin_mark.scope.clone();
                mark.style = plugin_mark.style;
                rows.add(plugin_mark.line, &mark);
            }
        }
        let rows = rows.into_rows();

        let track = theme.try_get_exact("ui.scrollbar").unwrap_or_default();
        let handle = handle_style(theme, bar);
        // The handle's ends by eighths of a row, a block character as tall as the part of
        // the row it covers, when its text is blank (else by whole rows).
        let smooth = bar.handle.smooth
            && bar.handle.text.trim().is_empty()
            && handle.bg.is_some_and(|bg| bg != Color::Reset);
        let (handle_top, handle_bottom) = if smooth {
            (handle_top, handle_bottom)
        } else {
            (
                handle_top / EIGHTHS * EIGHTHS,
                handle_bottom.div_ceil(EIGHTHS) * EIGHTHS,
            )
        };
        let background = track
            .bg
            .or_else(|| theme.get("ui.background").bg)
            .unwrap_or(Color::Reset);
        let mut styles: HashMap<MarkType, Style> = HashMap::new();
        for (row, mark) in rows.iter().enumerate() {
            let y = inner.y + row as u16;
            // the eighths of the row the handle covers, and which end of it
            let (row_top, row_bottom) = (row * EIGHTHS, (row + 1) * EIGHTHS);
            let covered = if show_handle {
                handle_bottom
                    .min(row_bottom)
                    .saturating_sub(handle_top.max(row_top))
            } else {
                0
            };
            let in_handle = covered * 2 >= EIGHTHS;
            let base = if in_handle {
                track.patch(handle)
            } else {
                track
            };
            let (symbol, style) = match mark {
                Some(mark) => {
                    let kind = mark.mark.kind;
                    let style = *styles
                        .entry(kind)
                        .or_insert_with(|| mark_style(theme, bar, kind));
                    (mark.text(bar), base.patch(own_style(theme, mark, style)))
                }
                None if covered == EIGHTHS => (bar.handle.text.as_str(), base),
                None if covered > 0 => {
                    let colour = handle.bg.unwrap_or(Color::Reset);
                    let style = Style::default().fg(colour).bg(background);
                    if handle_top > row_top {
                        // the top end: the handle in the lower part of the row
                        (LOWER_BLOCKS[covered], style)
                    } else {
                        // the bottom end: the handle in the upper part, the background
                        // (drawn in reverse video) in the lower part
                        (
                            LOWER_BLOCKS[EIGHTHS - covered],
                            style.add_modifier(Modifier::REVERSED),
                        )
                    }
                }
                None => (" ", base),
            };
            surface[(x, y)].set_symbol(" ").set_style(base);
            surface.set_stringn(x, y, symbol, 1, style);
        }
    }

    /// The lines of the matches of the last search in `doc`, when they are to be marked: the
    /// matches of the last search done of the buffer (of an earlier version while the buffer
    /// is searched again), none while a new pattern is searched for. With `search-calm-down`,
    /// only while the cursor is on a match.
    fn search_lines(
        &self,
        editor: &Editor,
        doc: &Document,
        view: &View,
    ) -> Option<Arc<Vec<usize>>> {
        let config = editor.config();
        let bar = &config.scrollbar;
        let query = query(editor, doc)?;
        let mut searches = self.search.lock().ok()?;
        // (closed buffers' matches go)
        searches.retain(|id, _| editor.documents.contains_key(id));
        if searches
            .get(&doc.id())
            .is_none_or(|entry| entry.query != query)
        {
            // a new pattern: compiled once
            let regex = query.regex()?;
            searches.insert(
                doc.id(),
                SearchMatches {
                    query: query.clone(),
                    regex,
                    version: None,
                    lines: Arc::new(Vec::new()),
                    started: None,
                    running: false,
                    waking: false,
                },
            );
        }
        let entry = searches.get_mut(&doc.id())?;
        if entry.version != Some(doc.version()) && !entry.running {
            let throttle = Duration::from_millis(bar.throttle_ms);
            let elapsed = entry.started.map_or(throttle, |started| started.elapsed());
            if elapsed >= throttle {
                self.search_in_background(entry, doc);
            } else if !entry.waking {
                // the next search when the throttle lets it run
                entry.waking = true;
                let wait = throttle - elapsed;
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    helix_event::request_redraw();
                });
            }
        }
        if bar.search_calm_down && !cursor_on_match(doc, view, &entry.regex) {
            return None;
        }
        Some(entry.lines.clone())
    }

    /// Searches `doc` for `entry`'s query on a blocking thread, then asks for the bar to be
    /// drawn again.
    fn search_in_background(&self, entry: &mut SearchMatches, doc: &Document) {
        entry.running = true;
        entry.waking = false;
        entry.started = Some(Instant::now());
        let text = doc.text().clone();
        let version = doc.version();
        let id = doc.id();
        let (query, regex) = (entry.query.clone(), entry.regex.clone());
        let searches = self.search.clone();
        tokio::task::spawn_blocking(move || {
            let slice = text.slice(..);
            let mut lines = Vec::new();
            for found in regex.find_iter(slice.regex_input()) {
                if lines.len() == MAX_SEARCH_MATCHES {
                    break;
                }
                lines.push(slice.byte_to_line(found.start()));
            }
            if let Ok(mut searches) = searches.lock() {
                if let Some(entry) = searches.get_mut(&id) {
                    entry.running = false;
                    // (not if another pattern was searched for meanwhile)
                    if entry.query == query {
                        entry.version = Some(version);
                        entry.lines = Arc::new(lines);
                    }
                }
            }
            helix_event::request_redraw();
        });
    }
}

/// The last search, as `n` searches for it.
fn query(editor: &Editor, doc: &Document) -> Option<Query> {
    let pattern = editor
        .registers
        .first(editor.registers.last_search_register, editor)?
        .to_string();
    if pattern.is_empty() {
        return None;
    }
    let case_insensitive =
        editor.config().search.smart_case && !pattern.chars().any(char::is_uppercase);
    Some(Query {
        pattern,
        case_insensitive,
        crlf: doc.line_ending == helix_core::LineEnding::Crlf,
    })
}

/// Whether the primary cursor of `view` is on a match of `regex` (one starting on its line).
fn cursor_on_match(doc: &Document, view: &View, regex: &rope::Regex) -> bool {
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let cursor_byte = text.char_to_byte(cursor);
    let line = text.char_to_line(cursor);
    let start = text.line_to_byte(line);
    let end = text.line_to_byte((line + 1).min(text.len_lines()));
    regex
        .find_iter(text.regex_input_at_bytes(start..end))
        .take_while(|found| found.start() <= cursor_byte)
        .any(|found| cursor_byte < found.end().max(found.start() + 1))
}

impl Scrollbars {
    /// A press of the mouse at `row`, `column`: on the bar of a view, it is focused and what
    /// a drag starts from is returned: the view, the place pressed (in eighths of rows) and
    /// the first line on screen. A press beside the handle first scrolls the view to put the
    /// handle's middle there.
    pub fn press(&self, editor: &mut Editor, row: u16, column: u16) -> Option<Held> {
        if !editor.config().scrollbar.mouse {
            return None;
        }
        let (view_id, geometry) = editor.tree.views().find_map(|(view, _)| {
            let geometry = self.geometry(editor, &editor.documents[&view.doc], view)?;
            let inner = geometry.inner;
            (column == inner.right() && (inner.top()..inner.bottom()).contains(&row))
                .then_some((view.id, geometry))
        })?;
        let at = usize::from(row - geometry.inner.y) * EIGHTHS + EIGHTHS / 2;
        let (top, bottom) = geometry.handle;
        editor.focus(view_id);
        let first = if (top..bottom).contains(&at) {
            geometry.first
        } else {
            let line = geometry
                .layout
                .line_at(at.saturating_sub((bottom - top) / 2));
            scroll_to(editor, view_id, &geometry, line)
        };
        Some(Held {
            view: view_id,
            at,
            first,
        })
    }

    /// The mouse dragged to `row`, holding a handle: the view scrolls by as many lines as
    /// the handle moved from where it was pressed (none for no move).
    pub fn drag(&self, editor: &mut Editor, held: Held, row: u16) {
        let Some(view) = editor.tree.try_get(held.view) else {
            return;
        };
        let Some(geometry) = self.geometry(editor, &editor.documents[&view.doc], view) else {
            return;
        };
        // (above the bar: as at its top)
        let rows = usize::from(row.saturating_sub(geometry.inner.y));
        let at = rows * EIGHTHS + EIGHTHS / 2;
        let line = geometry
            .layout
            .moved(held.first, at as isize - held.at as isize);
        scroll_to(editor, held.view, &geometry, line);
    }

    /// The last line of `doc` on the screen of `view` (soft-wrapped lines counted by their
    /// rows): worked out again only when the view scrolls, resizes or the buffer changes.
    fn last_line_on_screen(&self, doc: &Document, view: &View, inner: Rect) -> usize {
        let offset = doc.view_offset(view.id);
        let folds = doc.fold_container(view.id).map_or((0, 0), |container| {
            let starts = container.start_points();
            (starts.len(), starts.iter().map(|start| start.line).sum())
        });
        let key = ScreenKey {
            doc: doc.id(),
            version: doc.version(),
            anchor: offset.anchor,
            vertical_offset: offset.vertical_offset,
            width: inner.width,
            height: inner.height,
            folds,
            diagnostics: doc.diagnostics().len(),
            soft_wrap: doc.text_format(inner.width, None).soft_wrap,
        };
        let mut last_lines = self.last_lines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&(known, line)) = last_lines.get(&view.id) {
            if known == key {
                return line;
            }
        }
        let line = last_line_on_screen(doc, view, inner);
        last_lines.insert(view.id, (key, line));
        line
    }

    /// How many windows something is kept for (tests).
    pub fn windows_kept(&self) -> usize {
        self.last_lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Drops what is kept for the windows closed since (every split made and closed would
    /// leave its entry otherwise).
    fn forget_closed_views(&self, editor: &Editor) {
        let mut last_lines = self.last_lines.lock().unwrap_or_else(|e| e.into_inner());
        if last_lines.len() > editor.tree.views().count() {
            last_lines.retain(|view, _| editor.tree.contains(*view));
        }
    }
}

/// Scrolls `view_id` to put `line` at the top, as far as the last lines can go up, and keeps
/// the cursor on screen, `scrolloff` rows from its edges, as the mouse wheel does. Returns
/// the line at the top.
fn scroll_to(editor: &mut Editor, view_id: ViewId, geometry: &Geometry, line: usize) -> usize {
    let shown = geometry.last + 1 - geometry.first;
    let line = line.min(geometry.lines.saturating_sub(shown));
    editor.stop_scroll_animation();
    let scrolloff = editor.config().scrolloff.vertical;
    let select = editor.mode == helix_view::document::Mode::Select;
    let Some(view) = editor.tree.try_get(view_id) else {
        return line;
    };
    let Some(doc) = editor.documents.get_mut(&view.doc) else {
        return line;
    };
    let anchor = doc.text().line_to_char(line);
    let mut offset = doc.view_offset(view_id);
    if offset.anchor == anchor && offset.vertical_offset == 0 {
        return line;
    }
    offset.anchor = anchor;
    offset.vertical_offset = 0;
    doc.set_view_offset(view_id, offset);

    let text = doc.text().slice(..);
    let height = view.inner_height(doc);
    let scrolloff = scrolloff.min(height.saturating_sub(1) / 2);
    let text_format = doc.text_format(view.inner_area(doc).width, None);
    let annotations = view.text_annotations(doc, None);
    let at_row = |row: usize| {
        helix_core::char_idx_at_visual_offset(
            text,
            offset.anchor,
            row as isize,
            0,
            &text_format,
            &annotations,
        )
        .0
    };
    let (first, last) = (
        at_row(scrolloff),
        at_row(height.saturating_sub(scrolloff + 1)),
    );
    let primary = doc.selection(view_id).primary();
    let cursor = primary.cursor(text);
    let head = if cursor < first {
        first
    } else if cursor > last {
        last
    } else {
        return line;
    };
    drop(annotations);
    let range = if select {
        helix_core::Range::new(primary.anchor, head)
    } else {
        helix_core::Range::point(head)
    };
    let selection = doc.selection(view_id).clone();
    let index = selection.primary_index();
    doc.set_selection(view_id, selection.replace(index, range));
    line
}

/// The last line of `doc` on the screen of `view` (soft-wrapped lines counted by their rows).
fn last_line_on_screen(doc: &Document, view: &View, inner: Rect) -> usize {
    let text = doc.text().slice(..);
    let annotations = view.text_annotations(doc, None);
    let text_format = doc.text_format(inner.width, None);
    if !text_format.soft_wrap {
        return view.estimate_last_doc_line(&annotations, doc);
    }
    let offset = doc.view_offset(view.id);
    let (char, _) = helix_core::char_idx_at_visual_offset(
        text,
        offset.anchor,
        (offset.vertical_offset + inner.height.saturating_sub(1) as usize) as isize,
        0,
        &text_format,
        &annotations,
    );
    text.char_to_line(char.min(text.len_chars()))
}

/// The marks of the changes since the last commit, as gitsigns has them: added lines,
/// changed lines (then added ones when a change has more lines than it replaced), and a mark
/// for removed lines where they were.
fn add_git_marks(doc: &Document, lines: usize, rows: &mut Rows) {
    let Some(diff) = doc.diff_handle() else {
        return;
    };
    let hunks = diff.load();
    let (add, change, delete) = (
        Mark::new(MarkType::GitAdd),
        Mark::new(MarkType::GitChange),
        Mark::new(MarkType::GitDelete),
    );
    let last_line = lines.saturating_sub(1);
    for i in 0..hunks.len() {
        let hunk = hunks.nth_hunk(i);
        let (start, end) = (hunk.after.start as usize, hunk.after.end as usize);
        if hunk.is_pure_removal() {
            rows.add(start.min(last_line), &delete);
        } else if hunk.is_pure_insertion() {
            rows.add_lines(start, end - 1, &add);
        } else {
            let removed = (hunk.before.end - hunk.before.start) as usize;
            let changed_end = (start + removed).min(end);
            rows.add_lines(start, changed_end - 1, &change);
            if changed_end < end {
                rows.add_lines(changed_end, end - 1, &add);
            }
        }
    }
}

/// The colour of `kind`'s marks: its `color`, else its `highlight` key's foreground, else
/// its theme keys' (the first with a foreground, else the first with a background).
fn mark_style(theme: &Theme, bar: &ScrollbarConfig, kind: MarkType) -> Style {
    let config = kind.config(&bar.marks);
    let keys: Vec<&str> = config
        .highlight
        .as_deref()
        .into_iter()
        .chain(kind.scopes().iter().copied())
        .collect();
    let fg = config
        .color
        .as_deref()
        .and_then(|color| helix_view::theme::parse_color(color).ok())
        .or_else(|| colour_of(theme, &keys, |style| style.fg))
        .or_else(|| colour_of(theme, &keys, |style| style.bg));
    let mut style = Style {
        fg,
        ..Default::default()
    };
    if let Some(modifiers) = &config.modifiers {
        let modifiers = modifiers
            .iter()
            .filter_map(|name| name.parse::<Modifier>().ok())
            .fold(Modifier::empty(), |all, modifier| all | modifier);
        style = style.add_modifier(modifiers);
    }
    style
}

/// A plugin's mark in its own colour (a theme key, then a style), over its type's.
fn own_style(theme: &Theme, mark: &RowMark, style: Style) -> Style {
    let mut style = style;
    if let Some(scope) = &mark.mark.scope {
        style = style.patch(theme.get(scope));
    }
    if let Some(own) = mark.mark.style {
        style = style.patch(own);
    }
    style
}

/// The first colour `get` finds in the theme's `keys`.
fn colour_of(theme: &Theme, keys: &[&str], get: impl Fn(Style) -> Option<Color>) -> Option<Color> {
    keys.iter()
        .find_map(|key| theme.try_get_exact(key).and_then(&get))
}

/// The handle's style: its background is its `color`, else its `highlight` key's, else the
/// theme's (`ui.scrollbar.handle`, then the cursor column's or line's, then the selection's),
/// mixed with the editor's background by `blend`.
fn handle_style(theme: &Theme, bar: &ScrollbarConfig) -> Style {
    let keys: Vec<&str> = bar
        .handle
        .highlight
        .as_deref()
        .into_iter()
        .chain([
            "ui.scrollbar.handle",
            "ui.cursorcolumn.primary",
            "ui.cursorcolumn",
            "ui.cursorline.primary",
            "ui.cursorline",
            "ui.selection",
        ])
        .collect();
    let bg = bar
        .handle
        .color
        .as_deref()
        .and_then(|color| helix_view::theme::parse_color(color).ok())
        .or_else(|| colour_of(theme, &keys, |style| style.bg));
    let background = theme.get("ui.background").bg;
    let mut style = theme
        .try_get_exact("ui.scrollbar.handle")
        .unwrap_or_default();
    style.bg = match (bg, background) {
        (Some(handle), Some(background)) => Some(blend(handle, background, bar.handle.blend)),
        (handle, _) => handle,
    };
    style
}

/// `color` with `percent` of `background` showing through it (both RGB; else `color`).
fn blend(color: Color, background: Color, percent: u8) -> Color {
    match (color, background) {
        (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) => {
            let share = u16::from(percent.min(100));
            let mix = |front: u8, back: u8| {
                ((u16::from(front) * (100 - share) + u16::from(back) * share + 50) / 100) as u8
            };
            Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
        }
        _ => color,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn the_handle_blends_with_the_background() {
        let handle = Color::Rgb(200, 100, 0);
        let background = Color::Rgb(0, 0, 100);
        assert_eq!(blend(handle, background, 0), handle);
        assert_eq!(blend(handle, background, 100), background);
        assert_eq!(blend(handle, background, 30), Color::Rgb(140, 70, 30));
        // not RGB: as it is
        assert_eq!(blend(Color::Red, background, 30), Color::Red);
        assert_eq!(blend(handle, Color::Reset, 30), handle);
    }
}
