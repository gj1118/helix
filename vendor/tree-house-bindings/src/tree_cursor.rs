use ::std::os::raw;
use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::{c_char, CStr};
use std::marker::PhantomData;
use std::{fmt, mem};

use crate::node::NodeRaw;
use crate::{Node, Tree};

thread_local! {
    static CACHE: Cell<Option<TreeCursorGuard>> = const { Cell::new(None) };
}

#[repr(C)]
#[derive(Clone)]
struct TreeCursorRaw {
    tree: *const raw::c_void,
    id: *const raw::c_void,
    context: [u32; 3usize],
}

#[repr(C)]
struct TreeCursorGuard(TreeCursorRaw);

impl Drop for TreeCursorGuard {
    fn drop(&mut self) {
        unsafe { ts_tree_cursor_delete(&mut self.0) }
    }
}

pub struct TreeCursor<'a> {
    inner: TreeCursorRaw,
    tree: PhantomData<&'a Tree>,
}

impl<'tree> TreeCursor<'tree> {
    pub(crate) fn new(node: &Node<'tree>) -> Self {
        Self {
            inner: match CACHE.take() {
                Some(guard) => unsafe {
                    let mut cursor = guard.0.clone();
                    mem::forget(guard);
                    ts_tree_cursor_reset(&mut cursor, node.as_raw());
                    cursor
                },
                None => unsafe { ts_tree_cursor_new(node.as_raw()) },
            },
            tree: PhantomData,
        }
    }

    pub fn goto_parent(&mut self) -> bool {
        unsafe { ts_tree_cursor_goto_parent(&mut self.inner) }
    }

    pub fn goto_next_sibling(&mut self) -> bool {
        unsafe { ts_tree_cursor_goto_next_sibling(&mut self.inner) }
    }

    pub fn goto_previous_sibling(&mut self) -> bool {
        unsafe { ts_tree_cursor_goto_previous_sibling(&mut self.inner) }
    }

    pub fn goto_first_child(&mut self) -> bool {
        unsafe { ts_tree_cursor_goto_first_child(&mut self.inner) }
    }

    pub fn goto_last_child(&mut self) -> bool {
        unsafe { ts_tree_cursor_goto_last_child(&mut self.inner) }
    }

    pub fn goto_first_child_for_byte(&mut self, byte_idx: u32) -> Option<u32> {
        match unsafe { ts_tree_cursor_goto_first_child_for_byte(&mut self.inner, byte_idx) } {
            -1 => None,
            n => Some(n as u32),
        }
    }

    pub fn reset(&mut self, node: &Node<'tree>) {
        unsafe { ts_tree_cursor_reset(&mut self.inner, node.as_raw()) }
    }

    pub fn node(&self) -> Node<'tree> {
        unsafe { Node::from_raw(ts_tree_cursor_current_node(&self.inner)).unwrap_unchecked() }
    }

    pub fn field_name(&self) -> Option<&'tree str> {
        unsafe {
            let ptr = ts_tree_cursor_current_field_name(&self.inner);
            (!ptr.is_null()).then(|| CStr::from_ptr(ptr).to_str().unwrap())
        }
    }
}

impl fmt::Debug for TreeCursorRaw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InactiveTreeCursor").finish_non_exhaustive()
    }
}

impl Drop for TreeCursor<'_> {
    fn drop(&mut self) {
        CACHE.set(Some(TreeCursorGuard(self.inner.clone())))
    }
}

impl Clone for TreeCursor<'_> {
    fn clone(&self) -> Self {
        TreeCursor {
            inner: unsafe { ts_tree_cursor_copy(&self.inner) },
            tree: PhantomData,
        }
    }
}

impl<'cursor, 'tree: 'cursor> IntoIterator for &'cursor mut TreeCursor<'tree> {
    type Item = Node<'tree>;
    type IntoIter = TreeRecursiveWalker<'cursor, 'tree>;

    fn into_iter(self) -> Self::IntoIter {
        let mut queue = VecDeque::new();
        let root = self.node();
        queue.push_back(root.clone());

        TreeRecursiveWalker {
            cursor: self,
            queue,
            root,
        }
    }
}

pub struct TreeRecursiveWalker<'cursor, 'tree: 'cursor> {
    cursor: &'cursor mut TreeCursor<'tree>,
    queue: VecDeque<Node<'tree>>,
    root: Node<'tree>,
}

impl<'tree> Iterator for TreeRecursiveWalker<'_, 'tree> {
    type Item = Node<'tree>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.cursor.node();

        if current != self.root && self.cursor.goto_next_sibling() {
            self.queue.push_back(current);
            return Some(self.cursor.node());
        }

        while let Some(queued) = self.queue.pop_front() {
            self.cursor.reset(&queued);

            if !self.cursor.goto_first_child() {
                continue;
            }

            return Some(self.cursor.node());
        }

        None
    }
}

extern "C" {
    /// Create a new tree cursor starting from the given node.
    ///
    /// A tree cursor allows you to walk a syntax tree more efficiently than is
    /// possible using the `TSNode` functions. It is a mutable object that is always
    /// on a certain syntax node, and can be moved imperatively to different nodes.
    ///
    /// Note that the given node is considered the root of the cursor,
    /// and the cursor cannot walk outside this node.
    fn ts_tree_cursor_new(node: NodeRaw) -> TreeCursorRaw;
    /// Delete a tree cursor, freeing all of the memory that it used.
    fn ts_tree_cursor_delete(self_: *mut TreeCursorRaw);
    /// Re-initialize a tree cursor to start at a different node.
    fn ts_tree_cursor_reset(self_: *mut TreeCursorRaw, node: NodeRaw);
    // /// Re-initialize a tree cursor to the same position as another cursor.
    // /// Unlike [`ts_tree_cursor_reset`], this will not lose parent information and
    // /// allows reusing already created cursors.
    // fn ts_tree_cursor_reset_to(dst: *mut TreeCursorRaw, src: *const TreeCursorRaw);
    /// Get the tree cursor's current node.
    fn ts_tree_cursor_current_node(self_: *const TreeCursorRaw) -> NodeRaw;
    // /// Get the field name of the tree cursor's current node.
    // /// This returns `NULL` if the current node doesn't have a field.
    // /// See also [`ts_node_child_by_field_name`].
    // fn ts_tree_cursor_current_field_name(self_: *const TreeCursorRaw) -> *const raw::c_char;
    // /// Get the field id of the tree cursor's current node.
    // /// This returns zero if the current node doesn't have a field.
    // /// See also [`ts_node_child_by_field_id`], [`ts_language_field_id_for_name`].
    // fn ts_tree_cursor_current_field_id(self_: *const TreeCursorRaw) -> TSFieldId;
    /// Move the cursor to the parent of its current node.
    /// This returns `true` if the cursor successfully moved, and returns `false`
    /// if there was no parent node (the cursor was already on the root node).
    fn ts_tree_cursor_goto_parent(self_: *mut TreeCursorRaw) -> bool;
    /// Move the cursor to the next sibling of its current node.
    /// This returns `true` if the cursor successfully moved, and returns `false`
    /// if there was no next sibling node.
    fn ts_tree_cursor_goto_next_sibling(self_: *mut TreeCursorRaw) -> bool;
    /// Move the cursor to the previous sibling of its current node.
    /// This returns `true` if the cursor successfully moved, and returns `false` if
    /// there was no previous sibling node.
    /// Note, that this function may be slower than
    /// [`ts_tree_cursor_goto_next_sibling`] due to how node positions are stored. In
    /// the worst case, this will need to iterate through all the children upto the
    /// previous sibling node to recalculate its position.
    fn ts_tree_cursor_goto_previous_sibling(self_: *mut TreeCursorRaw) -> bool;
    /// Move the cursor to the first child of its current node.
    /// This returns `true` if the cursor successfully moved, and returns `false`
    /// if there were no children.
    fn ts_tree_cursor_goto_first_child(self_: *mut TreeCursorRaw) -> bool;
    /// Move the cursor to the last child of its current node.
    /// This returns `true` if the cursor successfully moved, and returns `false` if
    /// there were no children.
    /// Note that this function may be slower than [`ts_tree_cursor_goto_first_child`]
    /// because it needs to iterate through all the children to compute the child's
    /// position.
    fn ts_tree_cursor_goto_last_child(self_: *mut TreeCursorRaw) -> bool;
    /*
    /// Move the cursor to the node that is the nth descendant of
    /// the original node that the cursor was constructed with, where
    /// zero represents the original node itself.
    fn ts_tree_cursor_goto_descendant(self_: *mut TreeCursorRaw, goal_descendant_index: u32);
    /// Get the index of the cursor's current node out of all of the
    /// descendants of the original node that the cursor was constructed with.
    fn ts_tree_cursor_current_descendant_index(self_: *const TreeCursorRaw) -> u32;
    /// Get the depth of the cursor's current node relative to the original
    /// node that the cursor was constructed with.
    fn ts_tree_cursor_current_depth(self_: *const TreeCursorRaw) -> u32;
    */
    /// Move the cursor to the first child of its current node that extends beyond
    /// the given byte offset or point.
    /// This returns the index of the child node if one was found, and returns -1
    /// if no such child was found.
    fn ts_tree_cursor_goto_first_child_for_byte(self_: *mut TreeCursorRaw, goal_byte: u32) -> i64;
    fn ts_tree_cursor_copy(cursor: *const TreeCursorRaw) -> TreeCursorRaw;
    /// Get the field name of the tree cursor's curren tnode.
    ///
    /// This returns `NULL` if the current node doesn't have a field. See also
    /// `ts_node_child_by_field_name`.
    fn ts_tree_cursor_current_field_name(cursor: *const TreeCursorRaw) -> *const c_char;
}
