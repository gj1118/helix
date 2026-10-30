use helix_core::{
    doc_formatter::{FormattedGrapheme, TextFormat},
    Position,
};
use helix_view::{
    annotations::inline_completion::{layout, InlineCompletionRow},
    graphics::Style,
    inline_completion::InlineCompletion,
};

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Paints an inline completion preview at the cursor. The text after the cursor
/// is part of the preview (it moves to the right), so the cursor stays where it is.
pub struct InlineCompletionDecoration<'a> {
    completion: &'a InlineCompletion,
    style: Style,
    format: TextFormat,
    line_end: usize,
    anchor: Option<Position>,
    rows: Vec<InlineCompletionRow>,
    end_row: Option<usize>,
    rendered: bool,
}

impl<'a> InlineCompletionDecoration<'a> {
    pub fn new(
        completion: &'a InlineCompletion,
        style: Style,
        format: TextFormat,
        line_end: usize,
    ) -> Self {
        Self {
            completion,
            style,
            format,
            line_end,
            anchor: None,
            rows: Vec::new(),
            end_row: None,
            rendered: false,
        }
    }

    /// Clears the rest of a row and draws `row` on it.
    fn draw(
        renderer: &mut TextRenderer,
        row: Option<&InlineCompletionRow>,
        pos: Position,
        style: Style,
        soft_wrap: bool,
    ) {
        // Clip to this view before narrowing coordinates or touching the surface.
        if pos.row < renderer.offset.row
            || pos.row - renderer.offset.row >= renderer.viewport.height as usize
            || pos.col >= renderer.offset.col + renderer.viewport.width as usize
        {
            return;
        }
        let start = pos.col.saturating_sub(renderer.offset.col);
        let width = renderer.viewport.width as usize - start;
        let x = renderer.viewport.x + start as u16;
        renderer.set_string_truncated(
            x,
            pos.row as u16,
            &" ".repeat(width),
            width,
            |_| style,
            false,
            false,
        );
        let Some(row) = row else { return };
        let col = row.col.saturating_sub(renderer.offset.col);
        if col >= renderer.viewport.width as usize {
            return;
        }
        // Cut off what scrolled out of view on the left, padding wide graphemes.
        use helix_core::unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr};
        let mut logical_col = row.col;
        let visible: String = row
            .text
            .graphemes(true)
            .filter_map(|g| {
                let from = logical_col;
                logical_col += g.width();
                if from >= renderer.offset.col {
                    Some(g.to_owned())
                } else if logical_col > renderer.offset.col {
                    Some(" ".repeat(logical_col - renderer.offset.col))
                } else {
                    None
                }
            })
            .collect();
        renderer.set_string_truncated(
            renderer.viewport.x + col as u16,
            pos.row as u16,
            &visible,
            renderer.viewport.width as usize - col,
            |_| style,
            !soft_wrap,
            false,
        );
    }
}

impl Decoration for InlineCompletionDecoration<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.anchor = None;
        self.rows.clear();
        self.end_row = None;
        self.rendered = false;
        if pos <= self.completion.cursor {
            self.completion.cursor
        } else {
            usize::MAX
        }
    }

    fn decorate_grapheme(
        &mut self,
        _renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        if grapheme.char_idx == self.completion.cursor {
            self.anchor = Some(grapheme.visual_pos);
            self.rows = layout(
                &self.completion.lines,
                &self.format,
                grapheme.visual_pos.col,
            );
        }
        if grapheme.char_idx >= self.line_end {
            self.end_row = Some(grapheme.visual_pos.row);
            usize::MAX
        } else {
            self.line_end
        }
    }

    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        let Some(anchor) = self.anchor else {
            return Position::new(0, 0);
        };
        if self.rendered {
            return Position::new(0, 0);
        }
        let row = pos.visual_line as usize;
        let relative = row.saturating_sub(anchor.row);
        Self::draw(
            renderer,
            self.rows.get(relative),
            Position::new(row, if relative == 0 { anchor.col } else { 0 }),
            self.style,
            self.format.soft_wrap,
        );
        if self.end_row != Some(row) {
            return Position::new(0, 0);
        }
        // Last row of the line: the remaining preview rows go on the reserved virtual lines.
        self.rendered = true;
        let existing_rows = relative + 1;
        for (i, preview) in self.rows.iter().skip(existing_rows).enumerate() {
            Self::draw(
                renderer,
                Some(preview),
                Position::new(row + virt_off.row + i, 0),
                self.style,
                self.format.soft_wrap,
            );
        }
        Position::new(self.rows.len().saturating_sub(existing_rows), 0)
    }
}
