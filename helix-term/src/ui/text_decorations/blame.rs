use helix_core::Position;

use helix_view::theme::Style;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

pub enum LineBlame {
    OneLine((usize, String)),
    /// The blame of the visible lines: `lines[i]` belongs to document line
    /// `first_line + i`.
    ManyLines {
        first_line: usize,
        lines: Vec<Option<String>>,
    },
}

pub struct InlineBlame {
    lines: LineBlame,
    style: Style,
}

impl InlineBlame {
    pub fn new(style: Style, lines: LineBlame) -> Self {
        InlineBlame { style, lines }
    }
}

impl Decoration for InlineBlame {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // Only render inline blame on the last visual line
        // This prevents it being rendered multiple times per line, if the
        // line is wrapping.
        //
        // We want to render it on the last visual line because unless the user
        // has `soft-wrap.wrap-at-text-width` enabled, for a document line that wraps
        // there will only be space to render inline blame at the last doc line
        if !pos.is_last_visual_line {
            return Position::new(0, 0);
        }

        let blame = match &self.lines {
            LineBlame::OneLine((line, blame)) => {
                if line == &pos.doc_line {
                    // do not draw inline blame for lines that have no content in them
                    blame
                } else {
                    return Position::new(0, 0);
                }
            }
            LineBlame::ManyLines { first_line, lines } => {
                let blame = pos
                    .doc_line
                    .checked_sub(*first_line)
                    .and_then(|i| lines.get(i));
                if let Some(Some(blame)) = blame {
                    blame
                } else {
                    // do not draw inline blame for lines that have no content in them
                    return Position::new(0, 0);
                }
            }
        };

        // where the line in the document ends
        let end_of_line = virt_off.col;
        // draw the git blame 6 spaces after the end of the line
        let start_drawing_at_virtual = end_of_line + 6;

        let amount_of_characters_drawn = if renderer.column_in_bounds(start_drawing_at_virtual, 1) {
            // In bounds, so the start is at or right of the horizontal scroll and
            // within the viewport: it fits in a u16 and cannot underflow. The line
            // itself may end left of the scroll, hence the saturating length.
            let start_drawing_at = (start_drawing_at_virtual - renderer.offset.col) as u16;
            let line_length = end_of_line.saturating_sub(renderer.offset.col) as u16;
            // the column where we stop drawing the blame
            let stopped_drawing_at = renderer
                .set_string_truncated(
                    renderer.viewport.x + start_drawing_at,
                    pos.visual_line,
                    blame,
                    renderer.viewport.width.saturating_sub(start_drawing_at) as usize,
                    |_| self.style,
                    true,
                    false,
                )
                .0;

            stopped_drawing_at.saturating_sub(line_length)
        } else {
            0
        };

        Position::new(0, amount_of_characters_drawn as usize)
    }
}
