use crate::Document;
use crate::ViewId;
use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::text_annotations::LineAnnotation;
use helix_core::Position;

pub struct PluginLineAnnotations<'a> {
    doc: &'a Document,
    view_id: ViewId,
    anchor_idx: usize,
    anchors: Vec<usize>,
    hit_anchors: Vec<usize>,
}

impl<'a> PluginLineAnnotations<'a> {
    pub fn new(doc: &'a Document, view_id: ViewId) -> Self {
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
            view_id,
            anchor_idx: 0,
            anchors,
            hit_anchors: Vec::new(),
        }
    }
}

impl LineAnnotation for PluginLineAnnotations<'_> {
    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.hit_anchors.clear();
        self.anchor_idx = self.anchors.partition_point(|&a| a < char_idx);
        self.anchors
            .get(self.anchor_idx)
            .cloned()
            .unwrap_or(usize::MAX)
    }

    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.reset_pos(conceal_end_char_idx)
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        if self.anchors.get(self.anchor_idx) == Some(&grapheme.char_idx) {
            self.hit_anchors.push(grapheme.char_idx);
            self.anchor_idx += 1;
        }
        self.anchors
            .get(self.anchor_idx)
            .cloned()
            .unwrap_or(usize::MAX)
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        _line_end_visual_pos: Position,
        _doc_line: usize,
    ) -> Position {
        let mut line_count = 0;
        if let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) {
            for &anchor in &self.hit_anchors {
                for annot in annots {
                    if annot.is_line && annot.char_idx == anchor {
                        line_count += 1;
                    }
                }
            }
        }
        self.hit_anchors.clear();
        Position::new(line_count, 0)
    }
}
