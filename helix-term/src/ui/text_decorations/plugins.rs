use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;
use helix_core::doc_formatter::{DocumentFormatter, TextFormat};
use helix_core::text_annotations::TextAnnotations;
use helix_core::unicode::width::UnicodeWidthChar;
use helix_core::Position;
use helix_core::RopeSlice;
use helix_view::annotations::plugins::{
    annotation_text, annotation_text_format, eol_layout, inline_annotations,
    inline_annotations_drop, inline_start_col, plugin_annotations_on_line, virtual_line_rows,
};
use helix_view::document::{PluginAnnotation, PluginOverlay};
use helix_view::{Document, Theme, ViewId};

/// Draws the annotations plugins set with `buffer:set_annotations`. The rows they take are
/// reserved by `helix_view::annotations::plugins::PluginLineAnnotations`, whose layout
/// helpers this uses so that both agree.
pub struct PluginDecoration<'a> {
    doc: &'a Document,
    theme: &'a Theme,
    view_id: ViewId,
}

impl<'a> PluginDecoration<'a> {
    pub fn new(doc: &'a Document, theme: &'a Theme, view_id: ViewId) -> Self {
        Self {
            doc,
            theme,
            view_id,
        }
    }

    fn build_style(&self, annot: &PluginAnnotation) -> helix_view::theme::Style {
        let mut style = annot
            .style
            .as_deref()
            .and_then(|s| self.theme.try_get(s))
            .unwrap_or_default();

        if let Some(fg) = &annot.fg {
            if let Ok(color) = helix_view::graphics::Color::from_hex(fg) {
                style.fg = Some(color);
            }
        }

        if let Some(bg) = &annot.bg {
            if let Ok(color) = helix_view::graphics::Color::from_hex(bg) {
                style.bg = Some(color);
            }
        }

        style
    }
}

/// Lay out `text` with `text_fmt` and draw it (when `draw` is set) with its first row at
/// `first_row` (rows after it at `next_rows`, `next_rows + 1`, ...) and its first column
/// at `col`. Returns the number of rows the text takes, counting only up to the bottom of
/// the view: nothing below it is visible.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    renderer: &mut TextRenderer,
    text: &str,
    text_fmt: &TextFormat,
    style: helix_view::theme::Style,
    first_row: usize,
    next_rows: usize,
    col: usize,
    draw: bool,
) -> usize {
    let annotations = TextAnnotations::default();
    // Format straight from the string slice, no per-frame Rope allocation.
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(RopeSlice::from(text), text_fmt, &annotations, 0);
    let mut last_row = 0;
    for grapheme in formatter {
        last_row = grapheme.visual_pos.row;
        let row = match grapheme.visual_pos.row {
            0 => first_row,
            row => next_rows.saturating_add(row - 1),
        };
        if row >= renderer.viewport.height as usize {
            break;
        }
        if draw {
            let col = col.saturating_add(grapheme.visual_pos.col);
            if let (Ok(row), Ok(col)) = (u16::try_from(row), u16::try_from(col)) {
                renderer.draw_decoration_grapheme(grapheme.raw, style, row, col);
            }
        }
    }
    last_row + 1
}

/// The start of `text` that fits in `width` columns.
fn cut_to_width(text: &str, width: usize) -> &str {
    let mut used = 0usize;
    for (idx, c) in text.char_indices() {
        used = used.saturating_add(c.width().unwrap_or(0));
        if used > width {
            return &text[..idx];
        }
    }
    text
}

impl Decoration for PluginDecoration<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // Only render on the last visual line of a document line
        // This prevents duplicate rendering if the code line wraps
        if !pos.is_last_visual_line {
            return Position::new(0, 0);
        }
        let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) else {
            return Position::new(0, 0);
        };
        let line_annots =
            plugin_annotations_on_line(annots, self.doc.text().slice(..), pos.doc_line);
        if line_annots.is_empty() {
            return Position::new(0, 0);
        }

        let width = renderer.viewport.width;
        let tab_width = self.doc.tab_width();
        let code_row = pos.visual_line as usize;
        let first_virt_row = code_row.saturating_add(virt_off.row);

        // Inline annotations: on the code line (rows they wrap onto go to virtual lines),
        // or all together on the first virtual line when they do not fit ("dropped").
        let dropped = inline_annotations_drop(line_annots, virt_off.col, width);
        let mut rows = 0usize;
        // The columns the inline annotations take after the end of the line, for the
        // decorations drawn after them.
        let mut inline_col_used = 0usize;
        for annot in inline_annotations(line_annots) {
            let start_col = inline_start_col(annot, dropped, virt_off.col);
            let Some(text_fmt) = annotation_text_format(width, start_col, tab_width) else {
                continue;
            };
            // Skip drawing (but not the rows) when scrolled out of view horizontally.
            let visible = dropped || renderer.column_in_bounds(start_col, 1);
            let draw_col = start_col.saturating_sub(renderer.offset.col);
            let text = annotation_text(annot, dropped);
            let (first_row, next_rows) = if dropped {
                (first_virt_row, first_virt_row.saturating_add(1))
            } else {
                (code_row, first_virt_row)
            };
            let height = draw_text(
                renderer,
                text,
                &text_fmt,
                self.build_style(annot),
                first_row,
                next_rows,
                draw_col,
                visible,
            );
            if dropped {
                rows = rows.max(height);
            } else {
                rows = rows.max(height - 1);
                inline_col_used = inline_col_used.max(
                    (annot.offset as usize)
                        .saturating_add(annot.text.chars().count())
                        .saturating_add(2),
                );
            }
        }

        // Pinned annotations, on the line's row: after the end of the line when it leaves
        // room in the window (`eol_or_right`), else against the right edge of the window
        // (over the text if the line reaches it), each next one to the left of the last.
        let view_start = renderer.offset.col;
        let layout = eol_layout(line_annots, virt_off.col, width, view_start);
        // The decorations drawn after these (Helix's end-of-line diagnostics) go after the
        // ones put after the end of the line.
        inline_col_used = inline_col_used.max(layout.eol_end.saturating_sub(virt_off.col));
        for (annot, start) in layout.placed {
            let draw_col = start - view_start;
            let available = (width as usize).saturating_sub(draw_col);
            let text = cut_to_width(&annot.text, available);
            let text_fmt = TextFormat {
                soft_wrap: false,
                tab_width: tab_width.min(u16::MAX as usize) as u16,
                max_wrap: 0,
                max_indent_retain: 0,
                wrap_indicator_highlight: None,
                viewport_width: available.min(u16::MAX as usize) as u16,
                soft_wrap_at_text_width: false,
            };
            draw_text(
                renderer,
                text,
                &text_fmt,
                self.build_style(annot),
                code_row,
                code_row,
                draw_col,
                true,
            );
        }

        // Virtual line annotations, one group of rows after the other; annotations with
        // the same `virt_line_idx` share their first row.
        for annots_in_row in virtual_line_rows(line_annots).values() {
            let row = first_virt_row.saturating_add(rows);
            let mut height = 0;
            for annot in annots_in_row {
                let start_col = annot.offset as usize;
                let Some(text_fmt) = annotation_text_format(width, start_col, tab_width) else {
                    continue;
                };
                let draw_col = start_col.saturating_sub(renderer.offset.col);
                height = height.max(draw_text(
                    renderer,
                    &annot.text,
                    &text_fmt,
                    self.build_style(annot),
                    row,
                    row.saturating_add(1),
                    draw_col,
                    true,
                ));
            }
            rows = rows.saturating_add(height);
        }

        Position::new(rows, inline_col_used)
    }
}

/// Draws the text plugins set with `buffer:set_overlays` over the blank cells of their
/// lines, on the first row of each: the indentation, and the cells after the end of the line
/// (when it ends on that row). The text of a line is never covered.
pub struct PluginOverlays<'a> {
    overlays: &'a [PluginOverlay],
    text: RopeSlice<'a>,
    theme: &'a Theme,
    tab_width: usize,
}

impl<'a> PluginOverlays<'a> {
    /// `None` when no plugin draws over the text of `view_id`.
    pub fn new(doc: &'a Document, theme: &'a Theme, view_id: ViewId) -> Option<Self> {
        let overlays = doc.plugin_overlays.get(&view_id)?;
        Some(Self {
            overlays,
            text: doc.text().slice(..),
            theme,
            tab_width: doc.tab_width(),
        })
    }
}

/// The blank columns of `line` (without its line ending; tabs expanded to `tab_width`):
/// the width of its indentation, and the column after its end, if it ends before `limit`.
fn blank_columns(line: RopeSlice, tab_width: usize, limit: usize) -> (usize, Option<usize>) {
    use helix_stdx::rope::RopeSliceExt;
    let ending = helix_core::line_ending::get_line_ending(&line).map_or(0, |e| e.len_chars());
    let line = line.slice(..line.len_chars() - ending);
    let mut col = 0usize;
    let mut indent = None;
    for grapheme in line.graphemes() {
        let blank = grapheme == " " || grapheme == "\t";
        if !blank && indent.is_none() {
            indent = Some(col);
        }
        if indent.is_some() && col > limit {
            return (indent.unwrap_or(col), None);
        }
        col += if grapheme == "\t" {
            tab_width.max(1) - col % tab_width.max(1)
        } else {
            helix_core::graphemes::grapheme_width(&std::borrow::Cow::from(grapheme))
        };
    }
    (indent.unwrap_or(col), Some(col))
}

impl Decoration for PluginOverlays<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        _virt_off: Position,
    ) -> Position {
        let none = Position::new(0, 0);
        if !pos.first_visual_line {
            return none;
        }
        let start = self.overlays.partition_point(|o| o.line < pos.doc_line);
        let on_line = &self.overlays[start..];
        let end = on_line.partition_point(|o| o.line == pos.doc_line);
        let on_line = &on_line[..end];
        if on_line.is_empty() || pos.doc_line >= self.text.len_lines() {
            return none;
        }
        // the row on the screen
        let Some(row) = (pos.visual_line as usize).checked_sub(renderer.offset.row) else {
            return none;
        };
        if row >= renderer.viewport.height as usize {
            return none;
        }
        let limit = on_line
            .iter()
            .map(|o| o.col.saturating_add(o.text.len()))
            .max()
            .unwrap_or(0);
        let (indent, end) = blank_columns(self.text.line(pos.doc_line), self.tab_width, limit);
        // past its end only where the line ends on this row (not soft-wrapped onto the next)
        let end = end.filter(|_| pos.is_last_visual_line);
        for overlay in on_line {
            let style = overlay
                .scope
                .as_deref()
                .map(|scope| self.theme.get(scope))
                .unwrap_or_default()
                .patch(overlay.style);
            let mut col = overlay.col;
            for grapheme in helix_core::unicode::segmentation::UnicodeSegmentation::graphemes(
                overlay.text.as_str(),
                true,
            ) {
                let width = helix_core::graphemes::grapheme_width(grapheme).max(1);
                let blank = col + width <= indent || end.is_some_and(|end| col >= end);
                if blank && renderer.column_in_bounds(col, width) {
                    let x = renderer.viewport.x as usize + col - renderer.offset.col;
                    renderer.set_string(x as u16, pos.visual_line, grapheme, style);
                }
                col += width;
            }
        }
        none
    }
}
