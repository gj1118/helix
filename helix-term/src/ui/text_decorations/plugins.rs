use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;
use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::Position;
use helix_view::{Document, Theme, ViewId};

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
        let mut virt_lines_drawn = 0;
        let mut inline_col_used: u16 = 0;

        if let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) {
            let line_start = self.doc.text().line_to_char(pos.doc_line);
            let line_end = self.doc.text().line_to_char(pos.doc_line + 1);

            // First pass: draw inline annotations (is_line = false)
            // These appear at the end of the current line, to the right of the code
            for annot in annots.iter().filter(|a| !a.is_line) {
                if annot.char_idx >= line_start && annot.char_idx < line_end {
                    let style = self.build_style(annot);

                    // Position: end of line content + offset + some padding
                    let x = renderer.viewport.x + virt_off.col as u16 + annot.offset + 2;
                    let y = pos.visual_line;

                    // Check viewport bounds
                    if x < renderer.viewport.x + renderer.viewport.width {
                        renderer.set_string(x, y, &annot.text, style);
                        // Track how much horizontal space we used
                        inline_col_used =
                            inline_col_used.max(annot.offset + annot.text.len() as u16 + 2);
                    }
                }
            }

            // Second pass: draw virtual lines (is_line = true)
            // These appear below the current line
            for annot in annots.iter().filter(|a| a.is_line) {
                if annot.char_idx >= line_start && annot.char_idx < line_end {
                    let style = self.build_style(annot);

                    renderer.set_string(
                        renderer.viewport.x + annot.offset,
                        pos.visual_line + virt_off.row as u16 + virt_lines_drawn as u16,
                        &annot.text,
                        style,
                    );
                    virt_lines_drawn += 1;
                }
            }
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
