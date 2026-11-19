// TODO: rework using query iter

use std::iter;

use ropey::RopeSlice;

use crate::TREE_SITTER_MATCH_LIMIT;
use tree_sitter::{InactiveQueryCursor, Node, Query, RopeInput};

#[derive(Debug)]
pub enum CapturedNode<'a> {
    Single(Node<'a>),
    /// Guaranteed to be not empty
    Grouped(Vec<Node<'a>>),
}

impl CapturedNode<'_> {
    pub fn start_byte(&self) -> usize {
        match self {
            Self::Single(n) => n.start_byte() as usize,
            Self::Grouped(ns) => ns[0].start_byte() as usize,
        }
    }

    pub fn end_byte(&self) -> usize {
        match self {
            Self::Single(n) => n.end_byte() as usize,
            Self::Grouped(ns) => ns.last().unwrap().end_byte() as usize,
        }
    }
}

#[derive(Debug)]
pub struct TextObjectQuery {
    pub query: Query,
}

impl TextObjectQuery {
    /// Run the query on the given node and return sub nodes which match given
    /// capture ("function.inside", "class.around", etc).
    ///
    /// Captures may contain multiple nodes by using quantifiers (+, *, etc),
    /// and support for this is partial and could use improvement.
    ///
    /// ```query
    /// (comment)+ @capture
    ///
    /// ; OR
    /// (
    ///   (comment)*
    ///   .
    ///   (function)
    /// ) @capture
    /// ```
    pub fn capture_nodes<'a>(
        &'a self,
        capture_name: &str,
        node: Node<'a>,
        slice: RopeSlice<'a>,
        cursor: InactiveQueryCursor,
    ) -> Option<impl Iterator<Item = CapturedNode<'a>>> {
        self.capture_nodes_any(&[capture_name], node, slice, cursor)
    }

    /// Find the first capture that exists out of all given `capture_names`
    /// and return sub nodes that match this capture.
    pub fn capture_nodes_any<'a>(
        &'a self,
        capture_names: &[&str],
        node: Node<'a>,
        slice: RopeSlice<'a>,
        mut cursor: InactiveQueryCursor,
    ) -> Option<impl Iterator<Item = CapturedNode<'a>>> {
        let capture = capture_names
            .iter()
            .find_map(|cap| self.query.get_capture(cap))?;

        cursor.set_match_limit(TREE_SITTER_MATCH_LIMIT);
        let mut cursor = cursor.execute_query(&self.query, &node, RopeInput::new(slice));
        let capture_node = iter::from_fn(move || {
            let (mat, _) = cursor.next_matched_node()?;
            Some(mat.nodes_for_capture(capture).cloned().collect())
        })
        .filter_map(move |nodes: Vec<_>| {
            if nodes.len() > 1 {
                Some(CapturedNode::Grouped(nodes))
            } else {
                nodes.into_iter().map(CapturedNode::Single).next()
            }
        });
        Some(capture_node)
    }
}
