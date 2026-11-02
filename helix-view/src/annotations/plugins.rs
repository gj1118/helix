use crate::document::PluginAnnotation;
use crate::Document;
use crate::ViewId;
use helix_core::doc_formatter::{FormattedGrapheme, TextFormat};
use helix_core::text_annotations::LineAnnotation;
use helix_core::{softwrapped_dimensions, Position, RopeSlice};
use std::collections::BTreeMap;

/// The annotations anchored on `doc_line`.
///
/// `annots` must be sorted by `char_idx` (the plugin API stores them sorted), so the
/// line's range is found with two binary searches instead of a scan per line.
pub fn plugin_annotations_on_line<'a>(
    annots: &'a [PluginAnnotation],
    text: RopeSlice,
    doc_line: usize,
) -> &'a [PluginAnnotation] {
    if doc_line >= text.len_lines() {
        return &[];
    }
    let line_start = text.line_to_char(doc_line);
    let line_end = text.line_to_char(doc_line + 1);
    let lo = annots.partition_point(|a| a.char_idx < line_start);
    let hi = lo + annots[lo..].partition_point(|a| a.char_idx < line_end);
    &annots[lo..hi]
}

pub struct PluginLineAnnotations<'a> {
    doc: &'a Document,
    view_id: ViewId,
    width: u16,
}

impl<'a> PluginLineAnnotations<'a> {
    pub fn new(doc: &'a Document, view_id: ViewId, width: u16) -> Self {
        Self {
            doc,
            view_id,
            width,
        }
    }
}

impl LineAnnotation for PluginLineAnnotations<'_> {
    fn reset_pos(&mut self, _char_idx: usize) -> usize {
        usize::MAX
    }

    fn skip_concealed_anchors(&mut self, _conceal_end_char_idx: usize) -> usize {
        usize::MAX
    }

    fn process_anchor(&mut self, _grapheme: &FormattedGrapheme) -> usize {
        usize::MAX
    }
    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position {
        let mut inline_extra_rows: u16 = 0;
        let mut virt_annots_by_row: BTreeMap<u16, Vec<_>> = BTreeMap::new();
        let mut max_virt_idx: i32 = -1;
        let mut next_auto_idx: u16 = 0;

        if let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) {
            let line_annots =
                plugin_annotations_on_line(annots, self.doc.text().slice(..), doc_line);

            // Collect inline annotations
            let inline_annots: Vec<_> = line_annots.iter().filter(|a| !a.is_line).collect();

            // First pass: determine if ANY inline annotation needs to drop
            // If any needs to drop, ALL should drop together for consistent rendering
            // Calculate total width by summing all annotation character counts
            let mut should_drop_all = false;
            if let Some(first_annot) = inline_annots.first() {
                let start_col = line_end_visual_pos.col as u16 + first_annot.offset;
                let available_width = self.width.saturating_sub(start_col);
                // Drop if less than 40 columns available for the first annotation
                if available_width < 40 {
                    should_drop_all = true;
                }
            }
            // Check if all annotations fit - use conservative estimate for visual width
            if !should_drop_all {
                let total_chars: u16 = inline_annots
                    .iter()
                    .map(|a| a.text.chars().count() as u16)
                    .sum();
                let start_col = line_end_visual_pos.col as u16
                    + inline_annots.first().map(|a| a.offset).unwrap_or(0);
                // Conservative estimate: some chars may be 2 columns wide
                let estimated_end_col = start_col + total_chars + total_chars / 4;
                // If estimated total extends beyond viewport, drop all
                if estimated_end_col > self.width {
                    should_drop_all = true;
                }
            }

            // Second pass: calculate height for all inline annotations
            // Track where the last annotation ended so we can position the next one correctly
            let mut current_col: u16 = if should_drop_all {
                // For dropped: start from the first annotation's offset
                inline_annots.first().map(|a| a.offset).unwrap_or(0)
            } else {
                // For inline: start after line content + first annotation's offset
                line_end_visual_pos.col as u16
                    + inline_annots.first().map(|a| a.offset).unwrap_or(0)
            };

            for annot in &inline_annots {
                let dropped = should_drop_all;
                let start_col = current_col;
                let available_width = self.width.saturating_sub(start_col);

                if available_width > 0 {
                    let text_fmt = TextFormat {
                        soft_wrap: true,
                        tab_width: self.doc.tab_width() as u16,
                        max_wrap: available_width.saturating_div(4).max(20),
                        max_indent_retain: 0,
                        wrap_indicator_highlight: None,
                        viewport_width: available_width,
                        soft_wrap_at_text_width: true,
                    };
                    let (height, last_line_width) =
                        softwrapped_dimensions(annot.text.as_str().into(), &text_fmt);

                    // Update current_col based on where this annotation visually ends
                    if height == 1 {
                        // Single row: next annotation starts after this one's visual width
                        current_col = start_col + last_line_width;
                    } else {
                        // Multi-row: use the text length
                        current_col = start_col + annot.text.chars().count() as u16;
                    }

                    // Track max height among all inline annotations (they share same row)
                    if dropped {
                        // Dropped: all annotations share virtual line, track max wrapped height
                        inline_extra_rows = inline_extra_rows.max(height as u16);
                    } else {
                        // Non-dropped: row 0 on code line, wrapped rows on virtual lines
                        inline_extra_rows = inline_extra_rows.max(height as u16 - 1);
                    }
                }
            }

            // 2. Group virtual line annotations
            let virt_annots: Vec<_> = line_annots.iter().filter(|a| a.is_line).collect();
            for annot in &virt_annots {
                if let Some(idx) = annot.virt_line_idx {
                    max_virt_idx = max_virt_idx.max(idx as i32);
                }
            }

            for annot in &virt_annots {
                let row_idx = if let Some(idx) = annot.virt_line_idx {
                    idx
                } else {
                    let idx = (max_virt_idx + 1) as u16 + next_auto_idx;
                    next_auto_idx += 1;
                    idx
                };
                virt_annots_by_row.entry(row_idx).or_default().push(annot);
            }

            // 3. Calculate total height
            let mut cumulative_row_offset: u16 = inline_extra_rows;
            for (_logical_row, annots_in_row) in virt_annots_by_row {
                let mut max_height_in_row: u16 = 0;
                for annot in annots_in_row {
                    let text_col = annot.offset;
                    let available_width = self.width.saturating_sub(text_col);
                    if available_width > 0 {
                        let text_fmt = TextFormat {
                            soft_wrap: true,
                            tab_width: self.doc.tab_width() as u16,
                            max_wrap: available_width.saturating_div(4).max(20),
                            max_indent_retain: 0,
                            wrap_indicator_highlight: None,
                            viewport_width: available_width,
                            soft_wrap_at_text_width: true,
                        };
                        let height =
                            softwrapped_dimensions(annot.text.as_str().into(), &text_fmt).0;
                        max_height_in_row = max_height_in_row.max(height as u16);
                    }
                }
                cumulative_row_offset += max_height_in_row;
            }

            return Position::new(cumulative_row_offset as usize, 0);
        }

        Position::new(0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Rope;

    fn annot(char_idx: usize) -> PluginAnnotation {
        PluginAnnotation {
            char_idx,
            text: char_idx.to_string(),
            style: None,
            fg: None,
            bg: None,
            offset: 0,
            is_line: false,
            virt_line_idx: None,
            dropped_text: None,
        }
    }

    #[test]
    fn annotations_on_line_uses_line_ranges() {
        // lines: "ab\n" [0,3), "cd\n" [3,6), "" [6,6)
        let text = Rope::from("ab\ncd\n");
        let annots: Vec<_> = [0, 2, 3, 3, 5, 6, 100].into_iter().map(annot).collect();
        let idx = |line| {
            plugin_annotations_on_line(&annots, text.slice(..), line)
                .iter()
                .map(|a| a.char_idx)
                .collect::<Vec<_>>()
        };
        assert_eq!(idx(0), vec![0, 2]);
        assert_eq!(idx(1), vec![3, 3, 5]);
        assert_eq!(idx(2), Vec::<usize>::new());
        assert_eq!(idx(3), Vec::<usize>::new());
    }
}
