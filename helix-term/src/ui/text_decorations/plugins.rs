use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;
use helix_core::doc_formatter::{DocumentFormatter, FormattedGrapheme, TextFormat};
use helix_core::text_annotations::TextAnnotations;
use helix_core::Position;
use helix_view::{Document, Theme, ViewId};
use std::collections::BTreeMap;

pub struct PluginDecoration<'a> {
    doc: &'a Document,
    theme: &'a Theme,
    view_id: ViewId,
    anchor_idx: usize,
    anchors: Vec<usize>,
}

impl<'a> PluginDecoration<'a> {
    pub fn new(doc: &'a Document, theme: &'a Theme, view_id: ViewId) -> Self {
        let mut anchors = Vec::new();
        if let Some(annots) = doc.plugin_annotations.get(&view_id) {
            for annot in annots {
                if annot.is_line {
                    anchors.push(annot.char_idx);
                }
            }
        }
        anchors.sort_unstable();
        anchors.dedup();

        Self {
            doc,
            theme,
            view_id,
            anchor_idx: 0,
            anchors,
        }
    }

    fn build_style(
        &self,
        annot: &helix_view::document::PluginAnnotation,
    ) -> helix_view::theme::Style {
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

        let mut virt_lines_drawn = 0;
        let mut inline_col_used: u16 = 0;

        if let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) {
            let line_start = self.doc.text().line_to_char(pos.doc_line);
            let line_end = self.doc.text().line_to_char(pos.doc_line + 1);

            let line_annots: Vec<_> = annots
                .iter()
                .filter(|a| a.char_idx >= line_start && a.char_idx < line_end)
                .collect();

            // Track overflow inline annotations separately (matches PluginLineAnnotations logic)
            let mut inline_extra_rows: u16 = 0;

            // Collect inline annotations
            let inline_annots: Vec<_> = line_annots.iter().filter(|a| !a.is_line).collect();

            // First pass: determine if ANY inline annotation needs to drop
            // If any needs to drop, ALL should drop together for consistent rendering
            // Calculate total width by summing all annotation character counts
            let mut should_drop_all = false;
            if let Some(first_annot) = inline_annots.first() {
                let start_col = virt_off.col + first_annot.offset as usize;
                let available_width = renderer.viewport.width.saturating_sub(start_col as u16);
                // Drop if less than 40 columns available for the first annotation
                if available_width < 40 {
                    should_drop_all = true;
                }
            }
            // Check if all annotations fit - use conservative estimate for visual width
            // (chars * 2 accounts for potentially wide characters like arrows/bullets)
            if !should_drop_all {
                let total_chars: usize = inline_annots.iter().map(|a| a.text.chars().count()).sum();
                let start_col = virt_off.col
                    + inline_annots
                        .first()
                        .map(|a| a.offset as usize)
                        .unwrap_or(0);
                // Conservative estimate: some chars may be 2 columns wide
                let estimated_end_col = start_col + total_chars + total_chars / 4;
                // If estimated total extends beyond viewport, drop all
                if estimated_end_col > renderer.viewport.width as usize {
                    should_drop_all = true;
                }
            }

            // Second pass: render all inline annotations with consistent drop decision
            for annot in &inline_annots {
                let style = self.build_style(annot);
                let dropped = should_drop_all;

                // Use dropped_text if available and annotation is dropped, otherwise use text
                let display_text = if dropped {
                    annot.dropped_text.as_ref().unwrap_or(&annot.text)
                } else {
                    &annot.text
                };

                // Use plugin's offset directly for positioning
                let abs_start_col = if dropped {
                    // For dropped: use annotation's offset directly
                    annot.offset as usize
                } else {
                    // For inline: add to line content width
                    virt_off.col + annot.offset as usize
                };

                // Calculate available width, minimum 1 to allow single-char annotations like caps
                let available_width = renderer
                    .viewport
                    .width
                    .saturating_sub(abs_start_col as u16)
                    .max(1);

                // Check if the annotation start is within the visible viewport (skip if scrolled past)
                if !renderer.column_in_bounds(abs_start_col, 1) && !dropped {
                    continue;
                }

                // Calculate viewport-relative drawing position (subtract horizontal scroll offset)
                let draw_col = abs_start_col.saturating_sub(renderer.offset.col) as u16;

                // TextFormat must match space reservation for consistent height calculation
                let text_fmt = TextFormat {
                    soft_wrap: true,
                    tab_width: self.doc.tab_width() as u16,
                    max_wrap: available_width.saturating_div(4).max(20),
                    max_indent_retain: 0,
                    wrap_indicator_highlight: None,
                    viewport_width: available_width,
                    soft_wrap_at_text_width: true,
                };

                let annotations = TextAnnotations::default();
                let rope = helix_core::Rope::from(display_text.as_str());
                let formatter = DocumentFormatter::new_at_prev_checkpoint(
                    rope.slice(..),
                    &text_fmt,
                    &annotations,
                    0,
                );

                // All inline annotations (dropped or not) render on the same row
                // base_row is always 0 - they share the same starting line
                let mut last_row = 0;
                for grapheme in formatter {
                    last_row = grapheme.visual_pos.row;
                    let render_row = if dropped {
                        // Dropped: render on first virtual line (row 0 = first virtual line)
                        pos.visual_line + virt_off.row as u16 + grapheme.visual_pos.row as u16
                    } else if grapheme.visual_pos.row == 0 {
                        // Non-dropped row 0: render on code line
                        pos.visual_line
                    } else {
                        // Non-dropped rows 1+: render on virtual lines
                        pos.visual_line + virt_off.row as u16 + (grapheme.visual_pos.row as u16 - 1)
                    };
                    renderer.draw_decoration_grapheme(
                        grapheme.raw,
                        style,
                        render_row,
                        draw_col + grapheme.visual_pos.col as u16,
                    );
                }

                // Track max height among all inline annotations
                if dropped {
                    // Dropped: all annotations share virtual line, track max wrapped height
                    inline_extra_rows = inline_extra_rows.max(last_row as u16 + 1);
                } else {
                    // Non-dropped: row 0 on code line, wrapped rows on virtual lines
                    inline_extra_rows = inline_extra_rows.max(last_row as u16);
                    inline_col_used =
                        inline_col_used.max(annot.offset + annot.text.len() as u16 + 2);
                }
            }

            // Second pass: draw virtual lines (is_line = true)
            // Group by logical row so multiple annotations can share the same starting row
            let mut virt_annots_by_row: BTreeMap<
                u16,
                Vec<&helix_view::document::PluginAnnotation>,
            > = BTreeMap::new();
            let mut max_virt_idx: i32 = -1;
            let mut next_auto_idx: u16 = 0;

            // Collect all virtual line annotations
            let virt_annots: Vec<_> = line_annots.iter().filter(|a| a.is_line).copied().collect();

            // Find the max explicit virt_line_idx
            for annot in &virt_annots {
                if let Some(idx) = annot.virt_line_idx {
                    max_virt_idx = max_virt_idx.max(idx as i32);
                }
            }

            // Add virtual line annotations to the map
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

            // Virtual lines start after inline_extra_rows
            let mut cumulative_row_offset: u16 = inline_extra_rows;
            for (_logical_row, annots_in_row) in virt_annots_by_row {
                let mut max_height_in_row: u16 = 0;

                for annot in annots_in_row {
                    let style = self.build_style(annot);
                    let abs_text_col = annot.offset as usize;
                    let available_width = renderer.viewport.width.saturating_sub(annot.offset);

                    // Skip rendering if viewport is too narrow (match space reservation: available_width > 0)
                    if available_width == 0 {
                        continue;
                    }

                    // Calculate viewport-relative drawing position
                    let draw_col = abs_text_col.saturating_sub(renderer.offset.col) as u16;

                    let text_fmt = TextFormat {
                        soft_wrap: true,
                        tab_width: self.doc.tab_width() as u16,
                        max_wrap: available_width.saturating_div(4).max(20), // Match space reservation
                        max_indent_retain: 0,
                        wrap_indicator_highlight: None,
                        viewport_width: available_width,
                        soft_wrap_at_text_width: true,
                    };

                    let annotations = TextAnnotations::default();
                    let rope = helix_core::Rope::from(annot.text.as_str());
                    let formatter = DocumentFormatter::new_at_prev_checkpoint(
                        rope.slice(..),
                        &text_fmt,
                        &annotations,
                        0,
                    );

                    let mut last_row = 0;
                    for grapheme in formatter {
                        last_row = grapheme.visual_pos.row;
                        renderer.draw_decoration_grapheme(
                            grapheme.raw,
                            style,
                            pos.visual_line
                                + virt_off.row as u16
                                + cumulative_row_offset
                                + grapheme.visual_pos.row as u16,
                            draw_col + grapheme.visual_pos.col as u16,
                        );
                    }
                    max_height_in_row = max_height_in_row.max(last_row as u16 + 1);
                }
                cumulative_row_offset += max_height_in_row;
            }
            virt_lines_drawn = cumulative_row_offset as usize;
        }

        Position::new(virt_lines_drawn, inline_col_used as usize)
    }

    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.anchor_idx = self.anchors.partition_point(|&a| a < char_idx);
        self.anchors
            .get(self.anchor_idx)
            .cloned()
            .unwrap_or(usize::MAX)
    }

    fn decorate_grapheme(
        &mut self,
        _renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        if self.anchors.get(self.anchor_idx) == Some(&grapheme.char_idx) {
            self.anchor_idx += 1;
        }
        self.anchors
            .get(self.anchor_idx)
            .cloned()
            .unwrap_or(usize::MAX)
    }
}
