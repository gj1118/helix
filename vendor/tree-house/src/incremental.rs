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
//!
//! # A node made anew over children that were kept
//!
//! Some edits change a node with many children without changing the children: typing `let x =`
//! between two functions makes the root of a Rust file an `ERROR` node, until the statement is
//! whole again. Tree-sitter then reports the whole node as changed (the whole file), though the
//! parser kept nearly all of its children as they were. Querying all of it again costs the size of
//! the file, on a key.
//!
//! [`refine`] looks at such a node (the *shell*) and tells its children that are what they were
//! from the others: the subtrees the parser kept (see `Node::subtree_id`), and the ones it made
//! anew just as they were, node for node (after an error, the parser reads the rest of the file
//! again and makes the same nodes: [`same_structure`] compares them with the old ones, which costs
//! a fraction of querying them). What can differ is then:
//!
//! - matches anchored at the shell or above it: found by querying one byte at the shell's start,
//!   which every one of those nodes intersects;
//! - matches anchored in a child made anew: found by querying the child's range;
//! - matches anchored at a kept child (a pattern rooted at it, or a pattern of siblings among its
//!   children: the child is the same, but what was matched at it before may have been matched at
//!   a node of the same range above it): found by a query of the shell that starts patterns at
//!   its children and their children only (`max_start_depth`), which goes over the children
//!   without going into them.
//!
//! The matches anchored *below* a kept child are kept: nothing a pattern sees there changed.

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

/// A node made anew by the parser over children it mostly kept (see the module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shell {
    /// The node's range: it is the smallest node spanning it.
    pub range: Range<u32>,
    /// The ranges of its children that are the subtrees they were before the edits, in order.
    pub kept: Vec<Range<u32>>,
}

impl Shell {
    /// Whether a match anchored at `anchor` is anchored below one of the kept children (in it,
    /// and not at it): such a match is the one it was.
    ///
    /// An anchor of no width at the start of a child is not told from one before the child: it
    /// counts as outside.
    fn below_kept(&self, anchor: &Range<u32>) -> bool {
        let i = self.kept.partition_point(|child| child.end <= anchor.start);
        self.kept.get(i).is_some_and(|child| {
            child.start <= anchor.start
                && anchor.end <= child.end
                && (anchor.start > child.start || (anchor.end < child.end && !anchor.is_empty()))
        })
    }
}

/// How many updates queried a shell instead of all that tree-sitter reported as changed (for the
/// tests of the embedding editor: they check that their documents get there).
#[doc(hidden)]
pub static SHELL_UPDATES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Changed ranges shorter than this are queried as they are.
const REFINE_MIN_BYTES: u32 = 4096;
/// A shell has at least this many children: under a node with a few, the change is in one of
/// them.
const REFINE_MIN_CHILDREN: u32 = 16;
/// Children looked up in the old tree one by one (the ones not where a child of the old shell
/// was), each a walk from the root.
const REFINE_MAX_LOOKUPS: usize = 64;
/// A query of a changed child goes over the shell's children to get to it: no more of those
/// steps than this, in all.
const REFINE_MAX_STEPS: usize = 200_000;
/// Nodes compared with the old ones, at most, to tell the children made anew as they were.
const REFINE_MAX_COMPARED: usize = 150_000;

/// Whether the subtree of `new` is the subtree of `old`, node for node: kinds, ranges, fields.
/// The text is not compared (the caller knows no edit is in them). `budget` is the nodes that
/// may still be compared: with none left, the answer is no.
fn same_structure(old: &Node, new: &Node, budget: &mut usize) -> bool {
    let (mut old, mut new) = (old.walk(), new.walk());
    let mut depth = 0u32;
    loop {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let (a, b) = (old.node(), new.node());
        if a.kind_id() != b.kind_id() || a.byte_range() != b.byte_range() {
            return false;
        }
        // (the subtree the parser kept: nothing to compare in it)
        let kept = a.subtree_id() == b.subtree_id();
        if !kept {
            if a.is_named() != b.is_named()
                || a.is_missing() != b.is_missing()
                || a.is_extra() != b.is_extra()
                || (depth > 0 && old.field_name() != new.field_name())
            {
                return false;
            }
            match (old.goto_first_child(), new.goto_first_child()) {
                (true, true) => {
                    depth += 1;
                    continue;
                }
                (false, false) => (),
                _ => return false,
            }
        } else if depth > 0 && old.field_name() != new.field_name() {
            return false;
        }
        // On to the next sibling, of the node or of the nearest ancestor that has one.
        loop {
            if depth == 0 {
                return true;
            }
            match (old.goto_next_sibling(), new.goto_next_sibling()) {
                (true, true) => break,
                (false, false) => {
                    old.goto_parent();
                    new.goto_parent();
                    depth -= 1;
                }
                _ => return false,
            }
        }
    }
}

/// Looks at what tree-sitter reports as `changed` between the tree of `old_root` (moved by the
/// edits) and the tree of `new_root` parsed from it: when it is a large range under a node whose
/// children are mostly what they were, the node as a [`Shell`], and the ranges to query instead
/// of `changed` (see the module). `None` when `changed` is to be queried as it is. `edited` are
/// the ranges of the new text the edits replaced ([`changed_pieces`] of the edits alone).
pub(crate) fn refine(
    old_root: &Node,
    new_root: &Node,
    changed: &[Range<u32>],
    edited: &[Range<u32>],
) -> Option<(Shell, Vec<Range<u32>>)> {
    let start = changed.iter().map(|range| range.start).min()?;
    let end = changed.iter().map(|range| range.end).max()?;
    let end = end.min(new_root.end_byte());
    if end.saturating_sub(start) < REFINE_MIN_BYTES {
        return None;
    }
    let shell = new_root.descendant_for_byte_range(start, end)?;
    let children = shell.child_count();
    if children < REFINE_MIN_CHILDREN {
        return None;
    }
    // The children of the node that spanned the range before, by where they start: most
    // children of the shell were among them.
    let old_shell = old_root
        .descendant_for_byte_range(start, end.min(old_root.end_byte()))
        .unwrap_or_else(|| old_root.clone());
    let old_children: Vec<(Range<u32>, u16, u64)> = old_shell
        .children()
        .map(|child| (child.byte_range(), child.kind_id(), child.subtree_id()))
        .collect();

    let shell_range = shell.byte_range();
    let mut kept = Vec::new();
    // One byte at the start of the shell: what is anchored at it and above it.
    let mut pieces = Vec::with_capacity(8);
    pieces.push(shell_range.start..shell_range.start.saturating_add(1));
    let mut anew_bytes = 0u32;
    let mut lookups = 0;
    let mut compared = REFINE_MAX_COMPARED;
    // Whether the last child (tokens aside) changed: changed children in a row are one piece.
    let mut last_changed = false;
    for child in shell.children() {
        let range = child.byte_range();
        // A token: nothing is anchored below it (and the shell's query finds what is anchored
        // at it).
        if range.is_empty() || child.child_count() == 0 {
            continue;
        }
        // The node that was there: a child of the old shell, or (a statement of a function
        // that fell apart, say) a node somewhere else in the old tree.
        let first = old_children.partition_point(|old| old.0.start < range.start);
        let among_children = old_children[first..]
            .iter()
            .take_while(|old| old.0.start == range.start)
            .position(|old| old.0 == range && old.1 == child.kind_id())
            .map(|i| first + i);
        let mut is_kept = among_children.is_some_and(|i| old_children[i].2 == child.subtree_id());
        if !is_kept && !edited.iter().any(|edit| intersects(&range, edit)) {
            let old = match among_children {
                Some(i) => old_shell.child(i as u32),
                None if lookups < REFINE_MAX_LOOKUPS => {
                    lookups += 1;
                    old_root
                        .descendant_for_byte_range(range.start, range.end)
                        .filter(|old| old.byte_range() == range && old.kind_id() == child.kind_id())
                }
                None => None,
            };
            is_kept = old.is_some_and(|old| {
                old.subtree_id() == child.subtree_id()
                    || same_structure(&old, &child, &mut compared)
            });
        }
        // With a child of its own range, what is anchored at the node's range may be anchored
        // deeper than the shell's query starts patterns: such a node is queried whole.
        if is_kept
            && child
                .child(0)
                .is_some_and(|first| first.byte_range() == range)
        {
            is_kept = false;
        }
        if is_kept {
            kept.push(range);
        } else {
            anew_bytes = anew_bytes.saturating_add(range.end - range.start);
            match pieces.last_mut().filter(|_| last_changed) {
                Some(last) => last.end = range.end,
                None => pieces.push(range),
            }
        }
        last_changed = !is_kept;
    }
    let shell_bytes = shell_range.end - shell_range.start;
    if anew_bytes > shell_bytes / 2 || pieces.len() * children as usize > REFINE_MAX_STEPS {
        return None;
    }
    SHELL_UPDATES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some((
        Shell {
            range: shell_range,
            kept,
        },
        pieces,
    ))
}

/// A layer's matches after an edit: the old ones (moved by the edits) anchored away from the
/// `pieces` of the layer that changed, and the ones `query` finds in the pieces. With a `shell`
/// (see [`refine`]), an old match anchored in the shell is kept only below a child that was
/// kept, and `query_shell` gives the matches anchored at the shell's children. `None` when an
/// anchor is not known, so the layer has to be queried whole.
pub(crate) fn update_matches<T>(
    old: Vec<T>,
    pieces: &[Range<u32>],
    tree_range: &Range<u32>,
    shell: Option<&Shell>,
    anchor: impl Fn(&T) -> Option<&Range<u32>>,
    mut query: impl FnMut(Range<u32>) -> Vec<T>,
    query_shell: impl FnOnce(&Shell) -> Vec<T>,
) -> Option<Vec<T>> {
    let touched = |anchor: &Range<u32>, pieces: &[Range<u32>]| {
        pieces.iter().any(|piece| intersects(anchor, piece))
    };
    if old.iter().any(|mat| anchor(mat).is_none()) {
        return None;
    }
    let mut matches = old;
    matches.retain(|mat| {
        let anchor = anchor(mat).unwrap();
        !touched(anchor, pieces)
            && shell
                .is_none_or(|shell| !intersects(anchor, &shell.range) || shell.below_kept(anchor))
    });
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
    if let Some(shell) = shell {
        for mat in query_shell(shell) {
            let anchor = anchor(&mat)?;
            // Anchored at a child of the shell: not in a piece (its query found the match), not
            // above the shell, and not below a kept child (the old match was kept).
            let in_shell = shell.range.start <= anchor.start && anchor.end <= shell.range.end;
            if in_shell && !touched(anchor, pieces) && !shell.below_kept(anchor) {
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
