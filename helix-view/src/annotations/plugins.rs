use crate::document::{AnnotationAlign, PluginAnnotation};
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

// Layout shared by the space reservation below and the renderer in helix-term
// (`ui::text_decorations::plugins`): both must agree on how many rows a line's annotations
// take, or the virtual text is drawn over the following lines.

/// Inline annotations move to a virtual line below ("drop") when fewer columns than this
/// remain after the end of the line.
const MIN_INLINE_WIDTH: u16 = 40;

/// The inline (not `is_line`) annotations of a line drawn after its end (`align = "eol"`):
/// they may drop to a virtual line below and take rows.
pub fn inline_annotations(
    line_annots: &[PluginAnnotation],
) -> impl Iterator<Item = &PluginAnnotation> + Clone {
    line_annots
        .iter()
        .filter(|a| !a.is_line && a.align == AnnotationAlign::Eol)
}

/// The inline annotations of a line pinned to the right edge of the window (`"right"`), or
/// there when the end of the line leaves no room (`"eol_or_right"`): always on the line's
/// row, cut to the window, never taking rows.
pub fn pinned_annotations(
    line_annots: &[PluginAnnotation],
) -> impl Iterator<Item = &PluginAnnotation> {
    line_annots
        .iter()
        .filter(|a| !a.is_line && a.align != AnnotationAlign::Eol)
}

/// Whether the inline annotations of a line all drop to a virtual line below it: when
/// the first one would start with less than [`MIN_INLINE_WIDTH`] columns left, or when
/// they would (conservatively estimated) not fit before the edge of the view.
pub fn inline_annotations_drop(
    line_annots: &[PluginAnnotation],
    line_end_col: usize,
    width: u16,
) -> bool {
    let mut inline = inline_annotations(line_annots);
    let Some(first) = inline.clone().next() else {
        return false;
    };
    let start_col = line_end_col.saturating_add(first.offset as usize);
    if (width as usize).saturating_sub(start_col) < MIN_INLINE_WIDTH as usize {
        return true;
    }
    // Some chars may be 2 columns wide.
    let total_chars: usize = inline.by_ref().map(|a| a.text.chars().count()).sum();
    start_col
        .saturating_add(total_chars)
        .saturating_add(total_chars / 4)
        > width as usize
}

/// The text shown for an annotation (`dropped_text`, if it has one, once dropped).
pub fn annotation_text(annot: &PluginAnnotation, dropped: bool) -> &str {
    match &annot.dropped_text {
        Some(text) if dropped => text,
        _ => &annot.text,
    }
}

/// The view column an inline annotation starts at (`line_end_col` is where the line's
/// text ends; dropped annotations start at the left edge).
pub fn inline_start_col(annot: &PluginAnnotation, dropped: bool, line_end_col: usize) -> usize {
    if dropped {
        annot.offset as usize
    } else {
        line_end_col.saturating_add(annot.offset as usize)
    }
}

/// Where the inline annotations of a line end up on its row: `placed` has the start column of
/// each pinned one (`"right"`, `"eol_or_right"`), `eol_end` the column just after the last
/// one drawn after the end of the line (`line_end_col` when there is none). Columns are
/// absolute (the horizontal scroll included); `view_start` is the first one in view and
/// `width` the window's width. The renderer draws by this, and a plugin's panel starts at
/// `eol_end`, after them.
pub struct EolLayout<'a> {
    pub dropped: bool,
    pub placed: Vec<(&'a PluginAnnotation, usize)>,
    pub eol_end: usize,
}

pub fn eol_layout(
    line_annots: &[PluginAnnotation],
    line_end_col: usize,
    width: u16,
    view_start: usize,
) -> EolLayout<'_> {
    use helix_core::unicode::width::UnicodeWidthChar;
    let dropped = inline_annotations_drop(line_annots, line_end_col, width);
    // `eol` annotations each start at their offset after the end of the line.
    let inline_used = if dropped {
        0
    } else {
        inline_annotations(line_annots)
            .filter(|annot| {
                annotation_text_format(width, inline_start_col(annot, false, line_end_col), 4)
                    .is_some()
            })
            .map(|annot| {
                (annot.offset as usize)
                    .saturating_add(annot.text.chars().count())
                    .saturating_add(2)
            })
            .max()
            .unwrap_or(0)
    };
    let mut eol_next = line_end_col.saturating_add(inline_used);
    let mut right_next = view_start.saturating_add(width as usize);
    let mut placed = Vec::new();
    for annot in pinned_annotations(line_annots) {
        let text_width: usize = annot.text.chars().filter_map(|c| c.width()).sum();
        if text_width == 0 {
            continue;
        }
        let eol_start = eol_next.saturating_add(annot.offset as usize);
        let start = if annot.align == AnnotationAlign::EolOrRight
            && eol_start >= view_start
            && eol_start.saturating_add(text_width) <= right_next
        {
            eol_next = eol_start.saturating_add(text_width);
            eol_start
        } else {
            let start = right_next.saturating_sub(text_width).max(view_start);
            right_next = start.saturating_sub(1);
            start
        };
        placed.push((annot, start));
    }
    EolLayout {
        dropped,
        placed,
        eol_end: eol_next,
    }
}

/// How annotation text starting at `start_col` wraps in a view `width` columns wide, or
/// `None` if it starts beyond the right edge (it is then neither drawn nor given rows).
pub fn annotation_text_format(
    width: u16,
    start_col: usize,
    tab_width: usize,
) -> Option<TextFormat> {
    let available_width = (width as usize).checked_sub(start_col).filter(|&w| w > 0)? as u16;
    Some(TextFormat {
        soft_wrap: true,
        tab_width: tab_width.min(u16::MAX as usize) as u16,
        max_wrap: available_width.saturating_div(4).max(20),
        max_indent_retain: 0,
        wrap_indicator_highlight: None,
        viewport_width: available_width,
        soft_wrap_at_text_width: true,
    })
}

/// The virtual line (`is_line`) annotations of a line grouped by row: annotations with the
/// same `virt_line_idx` share a row; those without one each get a row after the last
/// explicit one.
pub fn virtual_line_rows(
    line_annots: &[PluginAnnotation],
) -> BTreeMap<u32, Vec<&PluginAnnotation>> {
    let virt_annots = line_annots.iter().filter(|a| a.is_line);
    let next_free = virt_annots
        .clone()
        .filter_map(|a| a.virt_line_idx)
        .max()
        .map_or(0, |idx| idx as u32 + 1);
    let mut rows: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    let mut next_auto = next_free;
    for annot in virt_annots {
        let row = match annot.virt_line_idx {
            Some(idx) => idx as u32,
            None => {
                next_auto += 1;
                next_auto - 1
            }
        };
        rows.entry(row).or_default().push(annot);
    }
    rows
}

fn rows_of(text: &str, text_fmt: &TextFormat) -> usize {
    softwrapped_dimensions(text.into(), text_fmt).0
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
        // Called for every line laid out (rendering, but also cursor movement and
        // scrolling), so lines without annotations must stay cheap.
        let Some(annots) = self.doc.plugin_annotations.get(&self.view_id) else {
            return Position::new(0, 0);
        };
        let line_annots = plugin_annotations_on_line(annots, self.doc.text().slice(..), doc_line);
        if line_annots.is_empty() {
            return Position::new(0, 0);
        }
        let tab_width = self.doc.tab_width();
        let line_end_col = line_end_visual_pos.col;

        // Inline annotations: row 0 is the code line itself, unless they drop to a virtual
        // line; rows they wrap onto are virtual lines.
        let dropped = inline_annotations_drop(line_annots, line_end_col, self.width);
        let mut rows = 0usize;
        for annot in inline_annotations(line_annots) {
            let start_col = inline_start_col(annot, dropped, line_end_col);
            if let Some(text_fmt) = annotation_text_format(self.width, start_col, tab_width) {
                let height = rows_of(annotation_text(annot, dropped), &text_fmt);
                rows = rows.max(if dropped { height } else { height - 1 });
            }
        }

        // Virtual line annotations, one group of rows after the other.
        for annots_in_row in virtual_line_rows(line_annots).values() {
            let height = annots_in_row
                .iter()
                .filter_map(|annot| {
                    let text_fmt =
                        annotation_text_format(self.width, annot.offset as usize, tab_width)?;
                    Some(rows_of(&annot.text, &text_fmt))
                })
                .max()
                .unwrap_or(0);
            rows = rows.saturating_add(height);
        }

        Position::new(rows, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Rope;

    fn annot(char_idx: usize) -> PluginAnnotation {
        PluginAnnotation {
            plugin: String::new(),
            char_idx,
            text: char_idx.to_string(),
            style: None,
            fg: None,
            bg: None,
            offset: 0,
            is_line: false,
            virt_line_idx: None,
            dropped_text: None,
            align: Default::default(),
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

    #[test]
    fn layout_handles_extreme_values() {
        // offsets and indices come from plugins: no overflow anywhere
        let mut wide = annot(0);
        wide.offset = u16::MAX;
        wide.text = "x".repeat(70_000);
        assert!(inline_annotations_drop(
            std::slice::from_ref(&wide),
            usize::MAX,
            80
        ));
        assert!(annotation_text_format(80, usize::MAX, 4).is_none());
        assert!(annotation_text_format(80, 80, 4).is_none());
        assert_eq!(annotation_text_format(80, 79, 4).unwrap().viewport_width, 1);

        let line = |idx: Option<u16>| PluginAnnotation {
            plugin: String::new(),
            is_line: true,
            virt_line_idx: idx,
            ..annot(0)
        };
        let annots = vec![line(Some(u16::MAX)), line(None), line(None), line(Some(0))];
        let rows: Vec<_> = virtual_line_rows(&annots)
            .into_iter()
            .map(|(row, annots)| (row, annots.len()))
            .collect();
        assert_eq!(
            rows,
            vec![(0, 1), (u16::MAX as u32, 1), (65536, 1), (65537, 1)]
        );
    }

    #[test]
    fn dropped_annotations_use_their_dropped_text() {
        let mut a = annot(0);
        a.dropped_text = Some("dropped".into());
        assert_eq!(annotation_text(&a, true), "dropped");
        assert_eq!(annotation_text(&a, false), "0");
        assert_eq!(inline_start_col(&a, true, 50), 0);
        assert_eq!(inline_start_col(&a, false, 50), 50);
    }
}
