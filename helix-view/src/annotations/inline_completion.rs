//! Layout of inline completion (ghost text) previews.

use helix_core::{
    doc_formatter::{DocumentFormatter, FormattedGrapheme, TextFormat},
    graphemes::Grapheme,
    text_annotations::{LineAnnotation, TextAnnotations},
    Position, Rope,
};

/// Reserves the rows of a preview that do not fit in the document line it starts on.
pub struct InlineCompletionLines<'a> {
    cursor: usize,
    lines: &'a [String],
    format: TextFormat,
    line_end: usize,
    anchor: Option<Position>,
    height: usize,
    at_end: bool,
    reserved: bool,
}

impl<'a> InlineCompletionLines<'a> {
    pub fn new(cursor: usize, lines: &'a [String], format: TextFormat, line_end: usize) -> Self {
        Self {
            cursor,
            lines,
            format,
            line_end,
            anchor: None,
            height: 0,
            at_end: false,
            reserved: false,
        }
    }
}

impl LineAnnotation for InlineCompletionLines<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.anchor = None;
        self.at_end = false;
        self.reserved = false;
        if pos <= self.cursor {
            self.cursor
        } else {
            usize::MAX
        }
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        if grapheme.char_idx == self.cursor {
            self.anchor = Some(grapheme.visual_pos);
            self.height = layout(self.lines, &self.format, grapheme.visual_pos.col).len();
        }
        self.at_end = grapheme.char_idx >= self.line_end;
        if self.at_end {
            usize::MAX
        } else {
            self.line_end
        }
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        line_end_visual_pos: Position,
        _doc_line: usize,
    ) -> Position {
        let Some(anchor) = self.anchor else {
            return Position::new(0, 0);
        };
        if !self.at_end || self.reserved {
            return Position::new(0, 0);
        }
        self.reserved = true;
        let existing_rows = line_end_visual_pos.row.saturating_sub(anchor.row) + 1;
        Position::new(self.height.saturating_sub(existing_rows), 0)
    }
}

/// A visual row of a preview, produced by the formatter used for document text.
pub struct InlineCompletionRow {
    pub col: usize,
    pub text: String,
}

/// Formats preview lines starting at column `col`, so tabs and soft wrapping
/// line up with the document text.
pub fn layout(lines: &[String], format: &TextFormat, col: usize) -> Vec<InlineCompletionRow> {
    // Padding supplies the anchor column to the formatter for tab stops and wrapping.
    // A non-whitespace sentinel keeps the padding from becoming retained indentation.
    let prefix = if col == 0 {
        String::new()
    } else {
        format!("x{}", " ".repeat(col - 1))
    };
    let text = Rope::from_str(&format!("{prefix}{}", lines.join("\n")));
    let annotations = TextAnnotations::default();
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.slice(..), format, &annotations, 0);
    let mut rows: Vec<InlineCompletionRow> = Vec::new();
    for grapheme in formatter {
        if grapheme.char_idx < col {
            continue;
        }
        while rows.len() <= grapheme.visual_pos.row {
            rows.push(InlineCompletionRow {
                col: 0,
                text: String::new(),
            });
        }
        let row = &mut rows[grapheme.visual_pos.row];
        if row.text.is_empty() {
            row.col = grapheme.visual_pos.col;
        }
        if !grapheme.source.is_eof() && !matches!(grapheme.raw, Grapheme::Newline) {
            row.text.push_str(&grapheme.raw.to_string());
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_rows_expand_tabs_at_the_anchor_and_preserve_unicode() {
        let rows = layout(
            &["\t界e\u{301}".into(), "".into()],
            &TextFormat::default(),
            2,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].col, rows[0].text.as_str()), (2, "  界e\u{301}"));
        assert_eq!((rows[1].col, rows[1].text.as_str()), (0, ""));
    }

    #[test]
    fn preview_rows_wrap_from_the_document_anchor_column() {
        let format = TextFormat {
            soft_wrap: true,
            viewport_width: 6,
            max_wrap: 0,
            max_indent_retain: 0,
            ..TextFormat::default()
        };
        let rows = layout(&["abcd界x".into()], &format, 2);
        let rows: Vec<_> = rows
            .iter()
            .map(|row| (row.col, row.text.as_str()))
            .collect();
        assert_eq!(rows, [(2, "abcd"), (0, "界x")]);
    }
}
