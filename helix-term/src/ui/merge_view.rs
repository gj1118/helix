//! Drawing the merge conflict resolver (see [`helix_view::merge_view`]): in the result, the
//! conflict blocks (your side, the base, their side, the markers); in your and their
//! versions, the lines of each conflict's side.

use std::ops::Range;

use helix_core::merge_conflicts::Conflict;
use helix_view::diff_view::DiffStyles;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::merge_view::Pane;
use helix_view::{Document, Editor, View};

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// The backgrounds of the lines of a window of the merge, by line.
pub struct MergeDecoration {
    /// Line ranges and their style, in order, not overlapping.
    lines: Vec<(Range<usize>, Style)>,
    inner: Rect,
}

impl MergeDecoration {
    /// The decoration of `view`, when it is a window of the merge.
    pub fn new(editor: &Editor, doc: &Document, view: &View) -> Option<Self> {
        let merge = editor.merge_of(view.id)?;
        let pane = merge.pane_of(view.id)?;
        if merge.doc(pane) != doc.id() {
            return None;
        }
        let styles = DiffStyles::new(&editor.theme);
        // yours in the color of additions, theirs in that of changes, the base in that of
        // removals; the markers stronger
        let (ours, theirs, base) = (styles.add, styles.change, styles.delete);
        let marker = styles.change_emphasis.add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        for conflict in merge.conflicts.iter() {
            match pane {
                Pane::Result => lines.extend(result_lines(conflict, ours, theirs, base, marker)),
                Pane::Local | Pane::Remote => {
                    let (side, style) = match pane {
                        Pane::Local => (&conflict.ours, ours),
                        _ => (&conflict.theirs, theirs),
                    };
                    let start = merge.from_result(pane, side.start);
                    let end = merge.from_result(pane, side.end).max(start);
                    if start < end {
                        lines.push((start..end, style));
                    }
                }
            }
        }
        lines.sort_by_key(|(range, _)| range.start);
        Some(Self {
            lines,
            inner: view.inner_area(doc),
        })
    }
}

/// The lines of a conflict block of the result, with their styles.
fn result_lines(
    conflict: &Conflict,
    ours: Style,
    theirs: Style,
    base: Style,
    marker: Style,
) -> Vec<(Range<usize>, Style)> {
    let mut lines = vec![(conflict.lines.start..conflict.lines.start + 1, marker)];
    lines.push((conflict.ours.clone(), ours));
    if let Some(base_lines) = &conflict.base {
        lines.push((base_lines.start - 1..base_lines.start, marker));
        lines.push((base_lines.clone(), base));
    }
    lines.push((conflict.theirs.start - 1..conflict.theirs.start, marker));
    lines.push((conflict.theirs.clone(), theirs));
    lines.push((conflict.lines.end - 1..conflict.lines.end, marker));
    lines.retain(|(range, _)| !range.is_empty());
    lines
}

impl Decoration for MergeDecoration {
    fn decorate_line(&mut self, renderer: &mut TextRenderer, pos: LinePos) {
        let line = pos.doc_line;
        let index = self.lines.partition_point(|(range, _)| range.end <= line);
        if let Some((range, style)) = self.lines.get(index) {
            if range.contains(&line) {
                renderer.set_style(
                    Rect::new(self.inner.x, pos.visual_line, self.inner.width, 1),
                    *style,
                );
            }
        }
    }
}
