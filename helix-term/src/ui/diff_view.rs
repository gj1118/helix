//! Drawing side-by-side diffs (see [`helix_view::diff_view`]): the backgrounds of the changed
//! lines and words of each side, and the connector between the sides.

use std::ops::Range;
use std::sync::Arc;

use helix_core::diff_hunks::{hunk_at, line_count, DiffHunk, Side};
use helix_core::doc_formatter::FormattedGrapheme;
use helix_view::diff_view::{shown_hunks, DiffSession, DiffStyles};
use helix_view::graphics::{Rect, Style};
use helix_view::{Document, Editor, View};
use tui::buffer::Buffer as Surface;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// The backgrounds of the changed lines of one side, and of the changed words in them.
pub struct DiffDecoration {
    side: Side,
    /// The diff's hunks (shared with the session, not copied each frame).
    hunks: Arc<[DiffHunk]>,
    styles: DiffStyles,
    /// The changed words of this side in the hunks shown, in order.
    words: Vec<Range<usize>>,
    /// The word the next graphemes are checked against.
    next_word: usize,
    inner: Rect,
}

impl DiffDecoration {
    /// The decoration of `view`, when it is a side of a diff.
    pub fn new(editor: &Editor, doc: &Document, view: &View) -> Option<Self> {
        let session = editor.diff_session(view.id)?;
        let side = session.side_of(view.id)?;
        if session.doc(side) != doc.id() {
            return None;
        }
        let words = shown_hunks(&session.hunks, view, doc, side)
            .flat_map(|index| session.words(index, side).iter().cloned())
            .collect();
        Some(Self {
            side,
            hunks: Arc::clone(&session.hunks),
            styles: DiffStyles::new(&editor.theme),
            words,
            next_word: 0,
            inner: view.inner_area(doc),
        })
    }
}

impl Decoration for DiffDecoration {
    fn decorate_line(&mut self, renderer: &mut TextRenderer, pos: LinePos) {
        let (index, inside) = hunk_at(&self.hunks, pos.doc_line, self.side);
        if !inside {
            return;
        }
        let style = self.styles.lines(self.hunks[index].kind(), self.side);
        renderer.set_style(
            Rect::new(self.inner.x, pos.visual_line, self.inner.width, 1),
            style,
        );
    }

    fn reset_pos(&mut self, pos: usize) -> usize {
        self.next_word = self.words.partition_point(|word| word.end <= pos);
        self.words
            .get(self.next_word)
            .map_or(usize::MAX, |word| word.start.max(pos))
    }

    fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        let Some(word) = self.words.get(self.next_word).cloned() else {
            return usize::MAX;
        };
        if word.contains(&grapheme.char_idx) {
            let col = grapheme.visual_pos.col;
            let width = grapheme.width();
            if width > 0 && renderer.column_in_bounds(col, width) {
                let x = renderer.viewport.x + (col - renderer.offset.col) as u16;
                renderer.set_style(
                    Rect::new(x, grapheme.visual_pos.row as u16, width as u16, 1),
                    self.styles.change_emphasis,
                );
            }
        }
        let next = grapheme.char_idx + grapheme.doc_chars();
        if next < word.end {
            return next;
        }
        self.next_word += 1;
        self.words
            .get(self.next_word)
            .map_or(usize::MAX, |word| word.start.max(next))
    }
}

/// Where the lines of one side of a diff are on the screen: worked out once per frame, so that
/// only the lines in the window are laid out (a long file has many changes out of it).
struct SideRows<'a> {
    view: &'a View,
    doc: &'a Document,
    /// The first line in the window, and (an estimate, ignoring soft wrap) the last one.
    top: usize,
    last: usize,
    /// The lines of the document (see [`line_count`]).
    lines: usize,
    height: f32,
}

impl<'a> SideRows<'a> {
    fn new(view: &'a View, doc: &'a Document) -> Self {
        let text = doc.text().slice(..);
        let top = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
        let annotations = view.text_annotations(doc, None);
        Self {
            view,
            doc,
            top,
            last: view.estimate_last_doc_line(&annotations, doc),
            lines: line_count(text),
            height: view.inner_area(doc).height as f32,
        }
    }

    /// The row (from the top of the text area) where `line` starts, as a fraction: negative
    /// above the window, past its height below it (counted in lines there).
    fn row(&self, line: usize) -> f32 {
        if line < self.top {
            return line as f32 - self.top as f32;
        }
        if self.lines == 0 {
            // an empty document: its lines would start at the top
            return line as f32;
        }
        if line >= self.lines {
            // past the last line: the row after it
            return self.row(self.lines - 1) + (line - self.lines + 1) as f32;
        }
        let below = |line: usize| self.height + line.saturating_sub(self.last + 1) as f32;
        if line > self.last {
            // out of the window: not laid out
            return below(line);
        }
        let text = self.doc.text().slice(..);
        match self
            .view
            .screen_coords_at_pos(self.doc, text, text.line_to_char(line))
        {
            Some(pos) => pos.row as f32,
            // pushed out of the window by wrapped lines above it
            None => below(line),
        }
    }
}

/// Draws the connector of every diff shown, over the blank gutter in front of its right side:
/// for each change, a band from its lines on the left to its lines on the right, in the color
/// of the change; a thin line where one side has no lines.
pub fn render_connectors(editor: &Editor, surface: &mut Surface) {
    for session in &editor.diff_sessions {
        render_connector(editor, session, surface);
    }
}

fn render_connector(editor: &Editor, session: &DiffSession, surface: &mut Surface) {
    let (Some(left_view), Some(right_view)) = (
        editor.tree.try_get(session.left_view),
        editor.tree.try_get(session.right_view),
    ) else {
        return;
    };
    let (Some(left_doc), Some(right_doc)) = (
        editor.document(session.left_doc),
        editor.document(session.right_doc),
    ) else {
        return;
    };
    if right_view.gutter_offset(right_doc) == 0 {
        // too narrow for gutters
        return;
    }
    let right_inner = right_view.inner_area(right_doc);
    let left_inner = left_view.inner_area(left_doc);
    let width = editor
        .config()
        .diff
        .connector_width
        .min(right_view.gutter_offset(right_doc));
    if width < 2 {
        return;
    }
    let area = Rect::new(right_view.area.x, right_inner.y, width, right_inner.height);
    let styles = DiffStyles::new(&editor.theme);
    // the left side's rows, in the connector's rows
    let shift = left_inner.y as f32 - right_inner.y as f32;
    let height = area.height as f32;
    let (left, right) = (
        SideRows::new(left_view, left_doc),
        SideRows::new(right_view, right_doc),
    );
    for hunk in session.hunks.iter() {
        let left_top = left.row(hunk.left.start) + shift;
        let left_bottom = left.row(hunk.left.end) + shift;
        let right_top = right.row(hunk.right.start);
        let right_bottom = right.row(hunk.right.end);
        if (left_bottom < 0.0 && right_bottom < 0.0) || (left_top >= height && right_top >= height)
        {
            continue;
        }
        let style = styles.lines(hunk.kind(), Side::Right);
        draw_band(
            surface,
            area,
            style,
            (left_top, left_bottom),
            (right_top, right_bottom),
        );
    }
}

/// A band across `area`, from the rows `left` (top, bottom) at its left edge to `right` at its
/// right edge: the cells mostly inside it get `style`; where it is thinner than half a row, a
/// line in the color of `style` marks it.
fn draw_band(surface: &mut Surface, area: Rect, style: Style, left: (f32, f32), right: (f32, f32)) {
    let line_style = match style.bg {
        Some(color) => Style::default().fg(color),
        None => style,
    };
    let last = (area.width - 1) as f32;
    for column in 0..area.width {
        let t = column as f32 / last;
        // a little flat at both ends, the slope in between
        let t = ((t - 0.15) / 0.7).clamp(0.0, 1.0);
        let top = left.0 + (right.0 - left.0) * t;
        let bottom = left.1 + (right.1 - left.1) * t;
        let x = area.x + column;
        if bottom - top >= 0.5 {
            for row in 0..area.height {
                let (from, to) = (row as f32, row as f32 + 1.0);
                let covered = (to.min(bottom) - from.max(top)).max(0.0);
                if covered >= 0.5 {
                    surface[(x, area.y + row)].set_style(style);
                }
            }
        } else {
            // a line between two rows: at the top of the row below, or the bottom of the one
            // above at the end of the area
            let at = top.round();
            let (row, symbol) = if at < height_of(area) {
                (at, "▔")
            } else {
                (at - 1.0, "▁")
            };
            if row >= 0.0 && row < height_of(area) {
                let cell = &mut surface[(x, area.y + row as u16)];
                cell.set_symbol(symbol);
                cell.set_style(line_style);
            }
        }
    }
}

fn height_of(area: Rect) -> f32 {
    area.height as f32
}
