//! Drawing side-by-side diffs (see [`helix_view::diff_view`]): the backgrounds of the changed
//! lines and words of each side, and the connector between the sides.

use std::ops::Range;
use std::sync::Arc;

use helix_core::diff_hunks::{hunk_at, line_count, DiffHunk, Side};
use helix_core::doc_formatter::FormattedGrapheme;
use helix_view::diff_connector::{connector_image, connector_png, Band, MAX_CELL};
use helix_view::diff_view::{shown_hunks, DiffSession, DiffStyles, Stage};
use helix_view::graphics::Color;
use helix_view::graphics::{Rect, Style};
use helix_view::image_preview::{iterm2, kitty, Encoded, Placement, Protocol};
use helix_view::{Document, Editor, View};
use tui::buffer::{Buffer as Surface, Overlay};

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
    for (index, session) in editor.diff_sessions.iter().enumerate() {
        render_connector(editor, index, session, surface);
    }
    // the images of the diffs closed since are forgotten
    let ids = KITTY_CONNECTOR_ID..KITTY_CONNECTOR_ID + editor.diff_sessions.len() as u32;
    IMAGES.with(|images| images.borrow_mut().retain(|id, _| ids.contains(id)));
}

/// The id of the connector image of the diff `index` in the terminal's memory (kitty
/// graphics protocol): its own, apart from the previews' one.
const KITTY_CONNECTOR_ID: u32 = 4_810_600;

fn render_connector(editor: &Editor, index: usize, session: &DiffSession, surface: &mut Surface) {
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
    // the stage of a change, against `HEAD`
    let config = editor.config();
    let marks = &config.diff.stage_markers;
    let mark_of = |hunk_index: usize| -> Option<String> {
        let git = session.git.as_ref().filter(|git| git.left_is_head)?;
        let mark = match git.stage(hunk_index)? {
            Stage::Staged => &marks.staged,
            Stage::Unstaged => &marks.unstaged,
            Stage::Partial => &marks.partial,
        };
        Some(mark.clone()).filter(|mark| !mark.is_empty())
    };
    let marker_fg = editor
        .theme
        .get("ui.text")
        .fg
        .or_else(|| editor.theme.get("ui.linenr.selected").fg);
    // the bands of the changes in view (or crossing it): from the first one not wholly above
    // both windows (binary search: a long file has tens of thousands of changes)
    let (left_top, right_top) = (left.top, right.top);
    let first = session
        .hunks
        .partition_point(|hunk| hunk.left.end < left_top && hunk.right.end < right_top);
    let mut bands = Vec::new();
    for (hunk_index, hunk) in session.hunks.iter().enumerate().skip(first) {
        let left_top = left.row(hunk.left.start) + shift;
        let left_bottom = left.row(hunk.left.end) + shift;
        let right_top = right.row(hunk.right.start);
        let right_bottom = right.row(hunk.right.end);
        if left_top >= height && right_top >= height {
            // this one and the next ones are below both windows
            break;
        }
        if left_bottom < 0.0 && right_bottom < 0.0 {
            continue;
        }
        let style = styles.lines(hunk.kind(), Side::Right);
        bands.push((
            hunk_index,
            (left_top, left_bottom),
            (right_top, right_bottom),
            style,
        ));
    }
    let image_bands: Option<(Protocol, Vec<Band>)> =
        connector_protocol(editor).and_then(|protocol| {
            bands
                .iter()
                .map(|(_, left, right, style)| match style.bg {
                    Some(Color::Rgb(r, g, b)) => Some(Band {
                        left: *left,
                        right: *right,
                        color: [r, g, b],
                    }),
                    // a color the image can't have: cells
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|image_bands| (protocol, image_bands))
        });
    let image = match image_bands {
        Some((protocol, image_bands)) => Some(draw_image(
            editor,
            protocol,
            KITTY_CONNECTOR_ID + index as u32,
            area,
            &image_bands,
            surface,
        )),
        None => {
            for (_, left, right, style) in &bands {
                draw_band(surface, area, *style, *left, *right);
            }
            None
        }
    };
    // the stage marks at the right edge, on each change's first row (over the image too)
    for (hunk_index, _, (right_top, _), _) in &bands {
        if let Some(mark) = mark_of(*hunk_index) {
            let row = right_top.floor();
            if (0.0..height).contains(&row) {
                let (x, y) = (area.right() - 1, area.y + row as u16);
                let cell = &mut surface[(x, y)];
                cell.set_symbol(&mark);
                if let Some(fg) = marker_fg {
                    cell.set_fg(fg);
                }
            }
        }
    }
    if let Some(image) = image {
        surface.add_overlay(image);
    }
}

/// The protocol the connector image is drawn with, `None` for cells
/// (`[editor.diff] connector-graphics`).
fn connector_protocol(editor: &Editor) -> Option<Protocol> {
    let config = editor.config();
    let preview = config
        .image_preview
        .protocol
        .resolve(|name| std::env::var(name).ok());
    config
        .diff
        .connector_graphics
        .protocol(preview, editor.kitty_graphics)
}

thread_local! {
    /// The last image of each connector (by its id in the terminal): its key, and what was
    /// sent for it, sent again unchanged without drawing and encoding it again.
    static IMAGES: std::cell::RefCell<std::collections::HashMap<u32, (u64, Encoded)>> =
        Default::default();
}

/// The connector over `area` as an image (`bands` drawn smooth and anti-aliased), shown with
/// `protocol` (the kitty graphics protocol as image `id`); its cells blank under it. To add to
/// `surface` once the text over it is drawn. An image like the one shown is not sent again
/// (see the overlays of [`Surface`]).
fn draw_image(
    editor: &Editor,
    protocol: Protocol,
    id: u32,
    area: Rect,
    bands: &[Band],
    surface: &mut Surface,
) -> Overlay {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            surface[(x, y)].set_symbol(" ");
        }
    }
    // the image's resolution: the terminal scales it to the cells, so the cell size (when the
    // terminal tells it) only up to a point, past which the image is only bigger to send
    let cell = editor.cell_size.map_or(MAX_CELL, |(width, height)| {
        (width.clamp(1, MAX_CELL.0), height.clamp(1, MAX_CELL.1))
    });
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (protocol, area.width, area.height, cell).hash(&mut hasher);
        // to a 64th of a row: what the image can show
        let at = |row: f32| (row * 64.0).round() as i64;
        for band in bands {
            (
                at(band.left.0),
                at(band.left.1),
                at(band.right.0),
                at(band.right.1),
                band.color,
            )
                .hash(&mut hasher);
        }
        hasher.finish()
    };
    let encoded = IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        match images.get(&id) {
            Some((shown, encoded)) if *shown == key => encoded.clone(),
            _ => {
                let image = connector_image(area.width, area.height, cell, bands);
                let placement = Placement {
                    cols: area.width,
                    rows: area.height,
                    x: 0,
                    y: 0,
                    width_px: image.width(),
                    height_px: image.height(),
                };
                let png = connector_png(&image);
                let encoded = match protocol {
                    Protocol::Iterm2 => iterm2(&png, &placement),
                    _ => kitty(&png, &placement, id),
                };
                images.insert(id, (key, encoded.clone()));
                encoded
            }
        }
    });
    Overlay {
        area,
        key,
        payload: Arc::from(encoded.payload.as_slice()),
        cleanup: encoded.cleanup.as_deref().map(Arc::from),
        // iTerm2's images take the place of the text of their cells (the stage markers)
        text_on_top: protocol == Protocol::Iterm2,
        cells: 0,
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
