use std::ffi::{c_char, c_void, CStr};
use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;
use std::ptr::NonNull;

use crate::tree::Tree;
use crate::tree_cursor::TreeCursor;
use crate::Grammar;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NodeRaw {
    context: [u32; 4],
    id: *const c_void,
    tree: *const c_void,
}

impl From<Node<'_>> for NodeRaw {
    fn from(node: Node) -> NodeRaw {
        NodeRaw {
            context: node.context,
            id: node.id.as_ptr(),
            tree: node.tree.as_ptr(),
        }
    }
}

#[derive(Clone)]
#[repr(C)]
pub struct Node<'tree> {
    context: [u32; 4],
    id: NonNull<c_void>,
    tree: NonNull<c_void>,
    _phantom: PhantomData<&'tree Tree>,
}

impl fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let range = self.byte_range();
        write!(f, "{{Node {} {range:?}}}", self.kind())
    }
}

impl<'tree> Node<'tree> {
    #[inline]
    pub(super) unsafe fn from_raw(raw: NodeRaw) -> Option<Self> {
        Some(Node {
            context: raw.context,
            id: NonNull::new(raw.id as *mut _)?,
            tree: unsafe { NonNull::new_unchecked(raw.tree as *mut _) },
            _phantom: PhantomData,
        })
    }

    #[inline]
    pub(crate) fn as_raw(&self) -> NodeRaw {
        NodeRaw {
            context: self.context,
            id: self.id.as_ptr(),
            tree: self.tree.as_ptr(),
        }
    }

    pub fn id(&self) -> usize {
        self.id.as_ptr() as usize
    }

    /// Get this node's type as a string
    #[inline]
    pub fn kind(&self) -> &'tree str {
        unsafe { CStr::from_ptr(ts_node_type(self.as_raw())) }
            .to_str()
            .unwrap()
    }

    /// Get this node's type as a numerical id.
    #[inline]
    pub fn kind_id(&self) -> u16 {
        unsafe { ts_node_symbol(self.as_raw()) }
    }

    /// Get the [`Grammar`] that was used to parse this node's syntax tree.
    #[inline]
    pub fn grammar(&self) -> Grammar {
        unsafe { ts_node_language(self.as_raw()) }
    }

    /// Check if this node is *named*.
    ///
    /// Named nodes correspond to named rules in the grammar, whereas
    /// *anonymous* nodes correspond to string literals in the grammar.
    #[inline]
    pub fn is_named(&self) -> bool {
        unsafe { ts_node_is_named(self.as_raw()) }
    }

    /// Returns true if and only if this node is contained "inside" the given
    /// input range, i.e. either start_new > start_old and end_new <= end_old OR
    /// start_new >= start_old and end_new < end_old
    pub fn is_contained_within(&self, range: Range<u32>) -> bool {
        (self.start_byte() > range.start && self.end_byte() <= range.end)
            || (self.start_byte() >= range.start && self.end_byte() < range.end)
    }

    /// Check if this node is *missing*.
    ///
    /// Missing nodes are inserted by the parser in order to recover from
    /// certain kinds of syntax errors.
    #[inline]
    pub fn is_missing(&self) -> bool {
        unsafe { ts_node_is_missing(self.as_raw()) }
    }

    /// Check if this node is *extra*.
    ///
    /// Extra nodes represent things like comments, which are not required by the
    /// grammar, but can appear anywhere.
    #[inline]
    pub fn is_extra(&self) -> bool {
        unsafe { ts_node_is_extra(self.as_raw()) }
    }

    /// Get the byte offsets where this node starts.
    #[inline(always)]
    pub fn start_byte(&self) -> u32 {
        // Normally we would implement this method like so:
        //
        //     extern "C" {
        //         /// Get the node's start byte.
        //         fn ts_node_start_byte(self_: NodeRaw) -> u32;
        //     }
        //     unsafe { ts_node_start_byte(self.as_raw()) }
        //
        // However this method has a trivial implementation which is unlikely to change (though
        // there is no guarantee) and this method can be called often, in tight loops, on a hot
        // code path (for example the highlighter's `next_event_offset` method). So we inline the
        // implementation directly from `node.c` in the C library to minimize overhead:
        self.context[0]
    }

    /// Get the byte offsets where this node end.
    #[inline]
    pub fn end_byte(&self) -> u32 {
        unsafe { ts_node_end_byte(self.as_raw()) }
    }

    /// Get the byte range of source code that this node represents.
    #[inline]
    pub fn byte_range(&self) -> Range<u32> {
        self.start_byte()..self.end_byte()
    }

    /// Get the node's child at the given index, where zero represents the first
    /// child.
    ///
    /// This method is fairly fast, but its cost is technically log(i), so if
    /// you might be iterating over a long list of children, you should use
    /// [`Node::children`] instead.
    #[inline]
    pub fn child(&self, i: u32) -> Option<Node<'tree>> {
        unsafe { Node::from_raw(ts_node_child(self.as_raw(), i)) }
    }

    /// Get this node's number of children.
    #[inline]
    pub fn child_count(&self) -> u32 {
        unsafe { ts_node_child_count(self.as_raw()) }
    }

    /// Get this node's *named* child at the given index.
    ///
    /// See also [`Node::is_named`].
    /// This method is fairly fast, but its cost is technically log(i), so if
    /// you might be iterating over a long list of children, you should use
    /// `Node::named_children` instead.
    #[inline]
    pub fn named_child(&self, i: u32) -> Option<Node<'tree>> {
        unsafe { Node::from_raw(ts_node_named_child(self.as_raw(), i)) }
    }

    /// Get this node's number of *named* children.
    ///
    /// See also [`Node::is_named`].
    #[inline]
    pub fn named_child_count(&self) -> u32 {
        unsafe { ts_node_named_child_count(self.as_raw()) }
    }

    #[inline]
    unsafe fn map(&self, f: unsafe extern "C" fn(NodeRaw) -> NodeRaw) -> Option<Node<'tree>> {
        Node::from_raw(f(self.as_raw()))
    }

    /// Get this node's immediate parent.
    #[inline]
    pub fn parent(&self) -> Option<Self> {
        unsafe { self.map(ts_node_parent) }
    }

    /// Get this node's next sibling.
    #[inline]
    pub fn next_sibling(&self) -> Option<Self> {
        unsafe { self.map(ts_node_next_sibling) }
    }

    /// Get this node's previous sibling.
    #[inline]
    pub fn prev_sibling(&self) -> Option<Self> {
        unsafe { self.map(ts_node_prev_sibling) }
    }

    /// Get this node's next named sibling.
    #[inline]
    pub fn next_named_sibling(&self) -> Option<Self> {
        unsafe { self.map(ts_node_next_named_sibling) }
    }

    /// Get this node's previous named sibling.
    #[inline]
    pub fn prev_named_sibling(&self) -> Option<Self> {
        unsafe { self.map(ts_node_prev_named_sibling) }
    }

    /// Get the smallest node within this node that spans the given range.
    #[inline]
    pub fn descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Self> {
        unsafe { Self::from_raw(ts_node_descendant_for_byte_range(self.as_raw(), start, end)) }
    }

    /// Get the smallest named node within this node that spans the given range.
    #[inline]
    pub fn named_descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Self> {
        unsafe {
            Self::from_raw(ts_node_named_descendant_for_byte_range(
                self.as_raw(),
                start,
                end,
            ))
        }
    }

    /// Iterate over this node's children.
    ///
    /// A [`TreeCursor`] is used to retrieve the children efficiently. Obtain
    /// a [`TreeCursor`] by calling [`Tree::walk`] or [`Node::walk`]. To avoid
    /// unnecessary allocations, you should reuse the same cursor for
    /// subsequent calls to this method.
    ///
    /// If you're walking the tree recursively, you may want to use the
    /// [`TreeCursor`] APIs directly instead.
    pub fn children(&self) -> impl ExactSizeIterator<Item = Node<'tree>> {
        let mut cursor = TreeCursor::new(self);
        cursor.goto_first_child();
        (0..self.child_count()).map(move |_| {
            let result = cursor.node();
            cursor.goto_next_sibling();
            result
        })
    }

    pub fn walk(&self) -> TreeCursor<'tree> {
        TreeCursor::new(self)
    }
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Node<'_> {}

unsafe impl Send for Node<'_> {}
unsafe impl Sync for Node<'_> {}

extern "C" {
    /// Get the node's type as a null-terminated string.
    fn ts_node_type(node: NodeRaw) -> *const c_char;

    /// Get the node's type as a numerical id.
    fn ts_node_symbol(node: NodeRaw) -> u16;

    /// Get the node's language.
    fn ts_node_language(node: NodeRaw) -> Grammar;

    /// Check if the node is *named*. Named nodes correspond to named rules in
    /// the grammar, whereas *anonymous* nodes correspond to string literals in
    /// the grammar
    fn ts_node_is_named(node: NodeRaw) -> bool;

    /// Check if the node is *missing*. Missing nodes are inserted by the parser
    /// in order to recover from certain kinds of syntax errors
    fn ts_node_is_missing(node: NodeRaw) -> bool;

    /// Check if this node is *extra*.
    ///
    /// Extra nodes represent things like comments, which are not required by the
    /// grammar, but can appear anywhere.
    fn ts_node_is_extra(node: NodeRaw) -> bool;

    /// Get the node's immediate parent
    fn ts_node_parent(node: NodeRaw) -> NodeRaw;

    /// Get the node's child at the given index, where zero represents the first
    /// child
    fn ts_node_child(node: NodeRaw, child_index: u32) -> NodeRaw;

    /// Get the node's number of children
    fn ts_node_child_count(node: NodeRaw) -> u32;

    /// Get the node's *named* child at the given index. See also
    /// [`ts_node_is_named`]
    fn ts_node_named_child(node: NodeRaw, child_index: u32) -> NodeRaw;

    /// Get the node's number of *named* children. See also [`ts_node_is_named`]
    fn ts_node_named_child_count(node: NodeRaw) -> u32;

    /// Get the node's next sibling
    fn ts_node_next_sibling(node: NodeRaw) -> NodeRaw;

    fn ts_node_prev_sibling(node: NodeRaw) -> NodeRaw;

    /// Get the node's next *named* sibling
    fn ts_node_next_named_sibling(node: NodeRaw) -> NodeRaw;

    fn ts_node_prev_named_sibling(node: NodeRaw) -> NodeRaw;

    /// Get the smallest node within this node that spans the given range of
    /// bytes or (row, column) positions
    fn ts_node_descendant_for_byte_range(node: NodeRaw, start: u32, end: u32) -> NodeRaw;

    /// Get the smallest named node within this node that spans the given range
    /// of bytes or (row, column) positions
    fn ts_node_named_descendant_for_byte_range(node: NodeRaw, start: u32, end: u32) -> NodeRaw;

    /// Get the node's end byte.
    fn ts_node_end_byte(node: NodeRaw) -> u32;
}
