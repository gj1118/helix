use core::slice;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ops::Range;
use std::ptr::NonNull;

use crate::node::NodeRaw;
use crate::query::{Capture, Pattern, Query, QueryData};
use crate::{Input, IntoInput, Node, Tree};

enum QueryCursorData {}

thread_local! {
    static CURSOR_CACHE: UnsafeCell<Vec<InactiveQueryCursor>> = UnsafeCell::new(Vec::with_capacity(8));
}

/// SAFETY: must not call itself recursively
unsafe fn with_cache<T>(f: impl FnOnce(&mut Vec<InactiveQueryCursor>) -> T) -> T {
    CURSOR_CACHE.with(|cache| f(&mut *cache.get()))
}

pub struct QueryCursor<'a, 'tree, I: Input> {
    query: &'a Query,
    ptr: NonNull<QueryCursorData>,
    tree: PhantomData<&'tree Tree>,
    input: I,
}

impl<'tree, I: Input> QueryCursor<'_, 'tree, I> {
    pub fn next_match(&mut self) -> Option<QueryMatch<'_, 'tree>> {
        let mut query_match = TSQueryMatch {
            id: 0,
            pattern_index: 0,
            capture_count: 0,
            captures: None,
        };
        loop {
            let success =
                unsafe { ts_query_cursor_next_match(self.ptr.as_ptr(), &mut query_match) };
            if !success {
                return None;
            }
            let matched_nodes: &[_] = match query_match.captures {
                None => &[],
                Some(ptr) => unsafe {
                    slice::from_raw_parts(ptr.cast().as_ptr(), query_match.capture_count as usize)
                },
            };
            let satisfies_predicates = self
                .query
                .pattern_text_predicates(query_match.pattern_index)
                .iter()
                .all(|predicate| predicate.satisfied(&mut self.input, matched_nodes, self.query));
            if satisfies_predicates {
                let res = QueryMatch {
                    id: query_match.id,
                    pattern: Pattern(query_match.pattern_index as u32),
                    matched_nodes,
                    query_cursor: unsafe { self.ptr.as_mut() },
                    _tree: PhantomData,
                };
                return Some(res);
            }
        }
    }

    pub fn next_matched_node(&mut self) -> Option<(QueryMatch<'_, 'tree>, MatchedNodeIdx)> {
        let mut query_match = TSQueryMatch {
            id: 0,
            pattern_index: 0,
            capture_count: 0,
            captures: None,
        };
        let mut capture_idx = 0;
        loop {
            let success = unsafe {
                ts_query_cursor_next_capture(self.ptr.as_ptr(), &mut query_match, &mut capture_idx)
            };
            if !success {
                return None;
            }
            let matched_nodes: &[_] = match query_match.captures {
                None => &[],
                Some(ptr) => unsafe {
                    slice::from_raw_parts(ptr.cast().as_ptr(), query_match.capture_count as usize)
                },
            };
            let satisfies_predicates = self
                .query
                .pattern_text_predicates(query_match.pattern_index)
                .iter()
                .all(|predicate| predicate.satisfied(&mut self.input, matched_nodes, self.query));
            if satisfies_predicates {
                let res = QueryMatch {
                    id: query_match.id,
                    pattern: Pattern(query_match.pattern_index as u32),
                    matched_nodes,
                    query_cursor: unsafe { self.ptr.as_mut() },
                    _tree: PhantomData,
                };
                return Some((res, capture_idx));
            } else {
                unsafe {
                    ts_query_cursor_remove_match(self.ptr.as_ptr(), query_match.id);
                }
            }
        }
    }

    pub fn set_byte_range(&mut self, range: Range<u32>) {
        unsafe {
            ts_query_cursor_set_byte_range(self.ptr.as_ptr(), range.start, range.end);
        }
    }

    pub fn reuse(self) -> InactiveQueryCursor {
        let res = InactiveQueryCursor { ptr: self.ptr };
        mem::forget(self);
        res
    }
}

impl<I: Input> Drop for QueryCursor<'_, '_, I> {
    fn drop(&mut self) {
        unsafe { with_cache(|cache| cache.push(InactiveQueryCursor { ptr: self.ptr })) }
    }
}

/// A query cursor that is not actively associated with a query
pub struct InactiveQueryCursor {
    ptr: NonNull<QueryCursorData>,
}

impl InactiveQueryCursor {
    #[must_use]
    pub fn new(range: Range<u32>, limit: u32) -> Self {
        let mut this = unsafe {
            with_cache(|cache| {
                cache.pop().unwrap_or_else(|| InactiveQueryCursor {
                    ptr: NonNull::new_unchecked(ts_query_cursor_new()),
                })
            })
        };
        this.set_byte_range(range);
        this.set_match_limit(limit);
        this
    }

    /// Return the maximum number of in-progress matches for this cursor.
    #[doc(alias = "ts_query_cursor_match_limit")]
    #[must_use]
    pub fn match_limit(&self) -> u32 {
        unsafe { ts_query_cursor_match_limit(self.ptr.as_ptr()) }
    }

    /// Set the maximum number of in-progress matches for this cursor.  The
    /// limit must be > 0 and <= 65536.
    #[doc(alias = "ts_query_cursor_set_match_limit")]
    pub fn set_match_limit(&mut self, limit: u32) {
        unsafe {
            ts_query_cursor_set_match_limit(self.ptr.as_ptr(), limit);
        }
    }

    /// Check if, on its last execution, this cursor exceeded its maximum number
    /// of in-progress matches.
    #[doc(alias = "ts_query_cursor_did_exceed_match_limit")]
    #[must_use]
    pub fn did_exceed_match_limit(&self) -> bool {
        unsafe { ts_query_cursor_did_exceed_match_limit(self.ptr.as_ptr()) }
    }

    pub fn set_byte_range(&mut self, range: Range<u32>) {
        unsafe {
            ts_query_cursor_set_byte_range(self.ptr.as_ptr(), range.start, range.end);
        }
    }

    pub fn execute_query<'a, 'tree, I: IntoInput>(
        self,
        query: &'a Query,
        node: &Node<'tree>,
        input: I,
    ) -> QueryCursor<'a, 'tree, I::Input> {
        let ptr = self.ptr;
        unsafe { ts_query_cursor_exec(ptr.as_ptr(), query.raw.as_ref(), node.as_raw()) };
        mem::forget(self);
        QueryCursor {
            query,
            ptr,
            tree: PhantomData,
            input: input.into_input(),
        }
    }
}

impl Default for InactiveQueryCursor {
    fn default() -> Self {
        Self::new(0..u32::MAX, u32::MAX)
    }
}

impl Drop for InactiveQueryCursor {
    fn drop(&mut self) {
        unsafe { ts_query_cursor_delete(self.ptr.as_ptr()) }
    }
}

pub type MatchedNodeIdx = u32;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct MatchedNode<'tree> {
    pub node: Node<'tree>,
    pub capture: Capture,
}

pub struct QueryMatch<'cursor, 'tree> {
    id: u32,
    pattern: Pattern,
    matched_nodes: &'cursor [MatchedNode<'tree>],
    query_cursor: &'cursor mut QueryCursorData,
    _tree: PhantomData<&'tree super::Tree>,
}

impl std::fmt::Debug for QueryMatch<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryMatch")
            .field("id", &self.id)
            .field("pattern", &self.pattern)
            .field("matched_nodes", &self.matched_nodes)
            .finish_non_exhaustive()
    }
}

impl<'tree> QueryMatch<'_, 'tree> {
    pub fn matched_nodes(&self) -> impl Iterator<Item = &MatchedNode<'tree>> {
        self.matched_nodes.iter()
    }

    pub fn nodes_for_capture(&self, capture: Capture) -> impl Iterator<Item = &Node<'tree>> {
        self.matched_nodes
            .iter()
            .filter(move |mat| mat.capture == capture)
            .map(|mat| &mat.node)
    }

    pub fn matched_node(&self, i: MatchedNodeIdx) -> &MatchedNode<'tree> {
        &self.matched_nodes[i as usize]
    }

    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    #[must_use]
    pub const fn pattern(&self) -> Pattern {
        self.pattern
    }

    #[doc(alias = "ts_query_cursor_remove_match")]
    /// removes this match from the cursor so that further captures
    /// from its cursor so that future captures that belong to this match
    /// are no longer returned by capture iterators
    pub fn remove(self) {
        unsafe {
            ts_query_cursor_remove_match(self.query_cursor, self.id);
        }
    }
}

#[repr(C)]
#[derive(Debug)]
struct TSQueryCapture {
    node: NodeRaw,
    index: u32,
}

#[repr(C)]
#[derive(Debug)]
struct TSQueryMatch {
    id: u32,
    pattern_index: u16,
    capture_count: u16,
    captures: Option<NonNull<TSQueryCapture>>,
}

extern "C" {
    /// Advance to the next capture of the currently running query.
    /// If there is a capture, write its match to `*match` and its index within
    /// the matche's capture list to `*capture_index`. Otherwise, return `false`.
    fn ts_query_cursor_next_capture(
        self_: *mut QueryCursorData,
        match_: &mut TSQueryMatch,
        capture_index: &mut u32,
    ) -> bool;

    /// Advance to the next match of the currently running query.
    ///
    /// If there is a match, write it to `*match` and return `true`.
    /// Otherwise, return `false`.
    fn ts_query_cursor_next_match(self_: *mut QueryCursorData, match_: &mut TSQueryMatch) -> bool;
    fn ts_query_cursor_remove_match(self_: *mut QueryCursorData, match_id: u32);
    /// Delete a query cursor, freeing all of the memory that it used
    fn ts_query_cursor_delete(self_: *mut QueryCursorData);
    /// Create a new cursor for executing a given query.
    /// The cursor stores the state that is needed to iteratively search
    /// for matches. To use the query cursor, first call [`ts_query_cursor_exec`]
    /// to start running a given query on a given syntax node. Then, there are
    /// two options for consuming the results of the query:
    ///
    /// 1. Repeatedly call [`ts_query_cursor_next_match`] to iterate over all of the
    ///    *matches* in the order that they were found. Each match contains the
    ///    index of the pattern that matched, and an array of captures. Because
    ///    multiple patterns can match the same set of nodes, one match may contain
    ///    captures that appear *before* some of the captures from a previous match.
    /// 2. Repeatedly call [`ts_query_cursor_next_capture`] to iterate over all of the
    ///    individual *captures* in the order that they appear. This is useful if
    ///    don't care about which pattern matched, and just want a single ordered
    ///    sequence of captures.
    ///
    /// If you don't care about consuming all of the results, you can stop calling
    /// [`ts_query_cursor_next_match`] or [`ts_query_cursor_next_capture`] at any point.
    /// You can then start executing another query on another node by calling
    /// [`ts_query_cursor_exec`] again."]
    fn ts_query_cursor_new() -> *mut QueryCursorData;

    /// Start running a given query on a given node.
    fn ts_query_cursor_exec(self_: *mut QueryCursorData, query: &QueryData, node: NodeRaw);
    /// Manage the maximum number of in-progress matches allowed by this query
    /// cursor.
    ///
    /// Query cursors have an optional maximum capacity for storing lists of
    /// in-progress captures. If this capacity is exceeded, then the
    /// earliest-starting match will silently be dropped to make room for further
    /// matches. This maximum capacity is optional — by default, query cursors allow
    /// any number of pending matches, dynamically allocating new space for them as
    /// needed as the query is executed.
    fn ts_query_cursor_did_exceed_match_limit(self_: *const QueryCursorData) -> bool;
    fn ts_query_cursor_match_limit(self_: *const QueryCursorData) -> u32;
    fn ts_query_cursor_set_match_limit(self_: *mut QueryCursorData, limit: u32);
    /// Set the range of bytes or (row, column) positions in which the query
    /// will be executed.
    fn ts_query_cursor_set_byte_range(self_: *mut QueryCursorData, start_byte: u32, end_byte: u32);

}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{Grammar, InactiveQueryCursor, Input, Parser, Query};

    struct StrInput<'a> {
        src: &'a str,
        cursor: &'a str,
    }

    impl<'a> StrInput<'a> {
        fn new(src: &'a str) -> Self {
            Self { src, cursor: src }
        }
    }

    impl<'a> Input for StrInput<'a> {
        type Cursor = &'a str;

        fn cursor_at(&mut self, _offset: u32) -> &mut &'a str {
            // &str Cursor has offset()=0 and chunk()=self, so the parser reads
            // src.as_ptr() + byte_index, which is correct for a contiguous string.
            self.cursor = self.src;
            &mut self.cursor
        }

        fn eq(&mut self, r1: std::ops::Range<u32>, r2: std::ops::Range<u32>) -> bool {
            let b = self.src.as_bytes();
            b[r1.start as usize..r1.end as usize] == b[r2.start as usize..r2.end as usize]
        }
    }

    fn python_grammar() -> Grammar {
        // PATCH (helix fork): Helix's own grammar (tree-house's test grammars are not here).
        let so = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/grammars/python.so");
        unsafe { Grammar::new("python", &so) }.expect("python grammar")
    }

    /// Regression test: when all captures in a pattern are disabled via
    /// `Query::disable_capture`, tree-sitter returns `capture_count=0` with a
    /// null `captures` pointer (valid C convention for an empty array).
    /// `TSQueryMatch.captures` must be `Option<NonNull<_>>` so the Rust binding
    /// handles `None` safely instead of passing a null pointer to
    /// `slice::from_raw_parts`.
    #[test]
    fn next_match_with_all_captures_disabled() {
        let grammar = python_grammar();
        let mut query = Query::new(grammar, "(identifier) @name", |_, _| Ok(())).unwrap();
        query.disable_capture("name");

        let src = "x = 1";
        let mut parser = Parser::new();
        parser.set_grammar(grammar).unwrap();
        let tree = parser.parse(StrInput::new(src), None).unwrap();

        let root = tree.root_node();
        let cursor = InactiveQueryCursor::new(0..src.len() as u32, u32::MAX);
        let mut cursor = cursor.execute_query(&query, &root, StrInput::new(src));

        // Consuming matches must not panic even though captures is NULL.
        let mut count = 0;
        while let Some(mat) = cursor.next_match() {
            assert!(mat.matched_nodes().count() == 0);
            count += 1;
        }
        assert!(count > 0, "expected at least one match");
    }
}
