//! What an edit changed, and which query matches of the old tree can be kept (PATCH (helix fork)).
//!
//! A query cursor limited to a byte range starts a rooted pattern at the nodes intersecting the
//! range, and a pattern of siblings (without a root) at the nodes whose parent intersects it; it
//! then follows each started pattern wherever its steps lead, in the range or not. So a match is
//! found by such a query exactly when its *anchor* intersects the range: the node of the pattern's
//! root, or the parent of a pattern of siblings. A match whose anchor is away from every change
//! saw none of it (everything a pattern looks at is in the anchor's subtree) and is still a match
//! of the new tree, moved by the edits; a match whose anchor touches a change is found again (or
//! not) by querying the changed ranges. That is how a layer's matches are updated by querying only
//! what changed: keep the old matches anchored away from the changes, add what a query of the
//! changed ranges finds.
//!
//! The anchor of a match is found from one of its captured nodes: the ancestor as many levels up as
//! the capture is deep in the pattern (one more for a pattern of siblings).

use std::cmp::Reverse;
use std::ops::Range;

use tree_sitter::{Capture, InputEdit, MatchedNode, Node, Query};

/// For each pattern of `query` (written in `source`), the captures it has at one depth only, each
/// with how many levels the pattern's anchor is above the captured node: one more than the
/// capture's depth for a pattern of siblings, whose anchor is their parent. A match's anchor is
/// found from any of them.
pub(crate) fn anchor_captures(query: &Query, source: &str) -> Vec<Vec<(Capture, u32)>> {
    query
        .patterns()
        .map(|pattern| {
            let text =
                &source[query.start_byte_for_pattern(pattern)..query.end_byte_for_pattern(pattern)];
            let rootless = u32::from(!query.is_pattern_rooted(pattern));
            query
                .captures()
                .filter_map(|(capture, name)| {
                    let depths = depths_in_pattern(text, name);
                    let depth = *depths.first()?;
                    depths
                        .iter()
                        .all(|&d| d == depth)
                        .then_some((capture, depth + rootless))
                })
                .collect()
        })
        .collect()
}

/// The levels between the root of a pattern (its top-level nodes for a pattern of siblings) and
/// each place `capture` is captured. Groups `( ... )` and alternations `[ ... ]` are not levels,
/// predicates `(#...)` are skipped.
fn depths_in_pattern(text: &str, capture: &str) -> Vec<u32> {
    #[derive(Clone, Copy)]
    enum Open {
        Node(u32),
        Group,
        Predicate,
        Alternation,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut depth = 0u32; // nodes open
    let mut last = 0u32; // the depth of the element a capture would apply to
    let mut depths = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let in_predicate = |stack: &[Open]| stack.iter().any(|open| matches!(open, Open::Predicate));
    while i < bytes.len() {
        match bytes[i] {
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                last = depth;
            }
            b'(' => {
                let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
                let open = match next {
                    Some(b'#') => Open::Predicate,
                    Some(b) if b.is_ascii_alphabetic() || *b == b'_' => {
                        depth += 1;
                        Open::Node(depth - 1)
                    }
                    _ => Open::Group,
                };
                stack.push(open);
            }
            b')' => match stack.pop() {
                Some(Open::Node(d)) => {
                    depth -= 1;
                    last = d;
                }
                Some(Open::Group) => last = depth,
                _ => (),
            },
            b'[' => stack.push(Open::Alternation),
            b']' => {
                stack.pop();
                last = depth;
            }
            b'@' => {
                let start = i + 1;
                while i + 1 < bytes.len()
                    && (bytes[i + 1].is_ascii_alphanumeric() || b"_.-".contains(&bytes[i + 1]))
                {
                    i += 1;
                }
                if !in_predicate(&stack) && &text[start..i + 1] == capture {
                    depths.push(last);
                }
            }
            b'_' if !bytes
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b':')
                && !bytes[..i]
                    .last()
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') =>
            {
                // the wildcard `_`
                last = depth;
            }
            _ => (),
        }
        i += 1;
    }
    depths
}

/// The byte range of the ancestor `depth` levels up of each node (the node itself for 0, the root
/// for more levels than there are), for nodes in document order (an ancestor before its
/// descendants), found with one walk of the tree instead of a walk from the root for each.
pub(crate) fn ancestors<'tree>(
    root: &Node<'tree>,
    nodes: impl Iterator<Item = (Node<'tree>, u32)>,
) -> Vec<Range<u32>> {
    let contains = |outer: &Node, inner: &Node| {
        outer.start_byte() <= inner.start_byte() && inner.end_byte() <= outer.end_byte()
    };
    // The path from the root to the node the walk is at.
    let mut path: Vec<Node<'tree>> = vec![root.clone()];
    nodes
        .map(|(node, depth)| {
            // Up, to an ancestor of the node, moving on to later siblings first.
            while path.len() > 1 && !contains(path.last().unwrap(), &node) {
                let parent = &path[path.len() - 2];
                let next = path.last().unwrap().next_sibling();
                match next.filter(|next| {
                    contains(parent, &node) && next.start_byte() <= node.start_byte()
                }) {
                    Some(next) => *path.last_mut().unwrap() = next,
                    None => {
                        path.pop();
                    }
                }
            }
            // Down, to the node.
            while path.last().unwrap().id() != node.id() {
                let current = path.last().unwrap().clone();
                let child = current.children().find(|child| {
                    contains(child, &node)
                        && (child.id() == node.id()
                            || child.byte_range() != (node.start_byte()..node.start_byte()))
                });
                match child {
                    Some(child) => path.push(child),
                    // Not reached by the walk (a node of no width, say): from the root.
                    None => return ancestor(node, depth),
                }
            }
            let index = path.len().saturating_sub(1 + depth as usize);
            path[index].byte_range()
        })
        .collect()
}

/// The ancestor `depth` levels up of `node`, walking up from it.
fn ancestor(mut node: Node, depth: u32) -> Range<u32> {
    for _ in 0..depth {
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    node.byte_range()
}

/// The capture a match's anchor is found from: the first of its nodes captured with one of the
/// pattern's `anchor_captures`, with how many levels up the anchor is.
pub(crate) fn anchor_source<'a, 'tree: 'a>(
    nodes: impl Iterator<Item = &'a MatchedNode<'tree>>,
    anchor_captures: &[(Capture, u32)],
) -> Option<(Node<'tree>, u32)> {
    nodes.into_iter().find_map(|matched| {
        let (_, depth) = anchor_captures
            .iter()
            .find(|(capture, _)| *capture == matched.capture)?;
        Some((matched.node.clone(), *depth))
    })
}

/// The anchors of matches from their `anchor_source`, found with one walk of the tree.
pub(crate) fn anchors<'tree>(
    root: &Node<'tree>,
    sources: &[Option<(Node<'tree>, u32)>],
) -> Vec<Option<Range<u32>>> {
    let mut order: Vec<usize> = (0..sources.len())
        .filter(|&i| sources[i].is_some())
        .collect();
    order.sort_by_key(|&i| {
        let node = &sources[i].as_ref().unwrap().0;
        (node.start_byte(), Reverse(node.end_byte()))
    });
    let ranges = ancestors(root, order.iter().map(|&i| sources[i].clone().unwrap()));
    let mut anchors = vec![None; sources.len()];
    for (i, range) in order.into_iter().zip(ranges) {
        anchors[i] = Some(range);
    }
    anchors
}

/// A layer's matches after an edit: the old ones (moved by the edits) anchored away from the
/// `pieces` of the layer that changed, and the ones `query` finds in the pieces. `None` when an
/// anchor is not known, so the layer has to be queried whole.
pub(crate) fn update_matches<T>(
    old: Vec<T>,
    pieces: &[Range<u32>],
    tree_range: &Range<u32>,
    anchor: impl Fn(&T) -> Option<&Range<u32>>,
    mut query: impl FnMut(Range<u32>) -> Vec<T>,
) -> Option<Vec<T>> {
    let touched = |anchor: &Range<u32>, pieces: &[Range<u32>]| {
        pieces.iter().any(|piece| intersects(anchor, piece))
    };
    if old.iter().any(|mat| anchor(mat).is_none()) {
        return None;
    }
    let mut matches = old;
    matches.retain(|mat| !touched(anchor(mat).unwrap(), pieces));
    for (i, piece) in pieces.iter().enumerate() {
        if !intersects(tree_range, piece) {
            continue;
        }
        for mat in query(piece.clone()) {
            // A match anchored in an earlier piece was found by its query too.
            if !touched(anchor(&mat)?, &pieces[..i]) {
                matches.push(mat);
            }
        }
    }
    Some(matches)
}

/// Whether a query limited to `range` would look at a node of `node_range` (tree-sitter's rule).
pub(crate) fn intersects(node_range: &Range<u32>, range: &Range<u32>) -> bool {
    (node_range.end > range.start || (node_range.is_empty() && node_range.end == range.start))
        && node_range.start < range.end
}

/// The ranges of the new document where matches can differ from the old ones: where the syntax
/// tree's structure changed and where text was replaced, each a byte wider on both sides (a node
/// ending or starting right at an edit counts as touched). Sorted and disjoint.
pub(crate) fn changed_pieces(changed: &[Range<u32>], edits: &[InputEdit]) -> Vec<Range<u32>> {
    let mut pieces: Vec<Range<u32>> = changed.to_vec();
    // The edits are in the old document's positions, each as if the ones before it were not made.
    let mut shift = 0i64;
    for edit in edits {
        let start = (edit.start_byte as i64 + shift) as u32;
        let end = (edit.new_end_byte as i64 + shift) as u32;
        pieces.push(start..end);
        shift += edit.offset() as i64;
    }
    for piece in &mut pieces {
        *piece = piece.start.saturating_sub(1)..piece.end.saturating_add(1);
    }
    pieces.sort_unstable_by_key(|piece| piece.start);
    let mut merged: Vec<Range<u32>> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match merged.last_mut() {
            Some(last) if piece.start <= last.end => last.end = last.end.max(piece.end),
            _ => merged.push(piece),
        }
    }
    merged
}

/// Moves positions of the old document to the new one. A position in an edit moves to the start
/// of what replaced it when it starts a range, to its end when it ends one.
pub(crate) struct Shift<'a> {
    edits: &'a [InputEdit],
    /// How much the edits before each one moved the positions after them.
    offsets: Vec<i64>,
}

impl<'a> Shift<'a> {
    pub(crate) fn new(edits: &'a [InputEdit]) -> Self {
        let mut offset = 0;
        let offsets = edits
            .iter()
            .map(|edit| {
                let before = offset;
                offset += edit.offset() as i64;
                before
            })
            .collect();
        Self { edits, offsets }
    }

    fn pos(&self, pos: u32, end: bool) -> u32 {
        let i = self.edits.partition_point(|edit| edit.old_end_byte < pos);
        let Some(edit) = self.edits.get(i).filter(|edit| edit.start_byte <= pos) else {
            let offset = self.offsets.get(i).copied().unwrap_or_else(|| self.total());
            return (pos as i64 + offset) as u32;
        };
        let pos = if end {
            edit.new_end_byte
        } else {
            edit.start_byte
        };
        (pos as i64 + self.offsets[i]) as u32
    }

    fn total(&self) -> i64 {
        self.edits.last().map_or(0, |edit| {
            self.offsets[self.offsets.len() - 1] + edit.offset() as i64
        })
    }

    pub(crate) fn range(&self, range: &Range<u32>) -> Range<u32> {
        self.pos(range.start, false)..self.pos(range.end, true)
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn depths() {
        assert_eq!(
            depths_in_pattern("((comment) @c (#set! injection.language \"x\"))", "c"),
            [0]
        );
        assert_eq!(
            depths_in_pattern(
                "(call_expression (selector_expression) @_f (#any-of? @_f \"a\") (argument_list . [ (raw (content) @c) (interp (content) @c) ]))",
                "c"
            ),
            [3, 3]
        );
        assert_eq!(
            depths_in_pattern("( ((comment) @_c) . [ (string (fragment) @c) ])", "c"),
            [1]
        );
        assert_eq!(
            depths_in_pattern("(source_file (comment) @c . (comment)* . [ (a) (b) ])", "c"),
            [1]
        );
        assert_eq!(depths_in_pattern("(x \"(\" @c _ @d (y) @e)", "c"), [1]);
        assert_eq!(depths_in_pattern("(x \"(\" @c _ @d (y) @e)", "d"), [1]);
        assert_eq!(depths_in_pattern("(_ (y)* @e)", "e"), [1]);
        assert_eq!(
            depths_in_pattern("((a) @x ; comment @x\n (#eq? @x \"@x\"))", "x"),
            [0]
        );
    }

    #[test]
    fn pieces() {
        let edit = |start, old_end, new_end| InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_point: tree_sitter::Point::ZERO,
            old_end_point: tree_sitter::Point::ZERO,
            new_end_point: tree_sitter::Point::ZERO,
        };
        // Typing 2 bytes at 10 and deleting 5..8 after it moves the deletion by 2.
        let edits = [edit(10, 10, 12), edit(20, 25, 20)];
        assert_eq!(changed_pieces(&[], &edits), [9..13, 21..23]);
        assert_eq!(changed_pieces(&[11..30], &edits), [9..31]);
        let shift = Shift::new(&edits);
        assert_eq!(shift.range(&(5..15)), 5..17);
        assert_eq!(shift.range(&(30..31)), 27..28);
        // Ranges around or in an edit cover what replaced it.
        assert_eq!(shift.range(&(10..10)), 10..12);
        assert_eq!(shift.range(&(15..22)), 17..22);
        assert_eq!(shift.range(&(22..40)), 22..37);
    }
}
