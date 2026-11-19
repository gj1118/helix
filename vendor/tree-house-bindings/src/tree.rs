use std::fmt;
use std::ptr::NonNull;

use crate::node::{Node, NodeRaw};
use crate::{Point, TreeCursor};

// opaque pointers
pub(super) enum SyntaxTreeData {}

pub struct Tree {
    ptr: NonNull<SyntaxTreeData>,
}

impl Tree {
    pub(super) unsafe fn from_raw(raw: NonNull<SyntaxTreeData>) -> Tree {
        Tree { ptr: raw }
    }

    pub(super) fn as_raw(&self) -> NonNull<SyntaxTreeData> {
        self.ptr
    }

    pub fn root_node(&self) -> Node<'_> {
        unsafe { Node::from_raw(ts_tree_root_node(self.ptr)).unwrap() }
    }

    pub fn edit(&mut self, edit: &InputEdit) {
        unsafe { ts_tree_edit(self.ptr, edit) }
    }

    pub fn walk(&self) -> TreeCursor<'_> {
        self.root_node().walk()
    }

    /// The ranges whose syntactic structure differs between this tree (edited with the same
    /// edits) and `new`, a tree parsed from it (PATCH (helix fork)). Text replaced without
    /// changing the structure is not in them: the edits themselves are.
    #[doc(alias = "ts_tree_get_changed_ranges")]
    pub fn changed_ranges(&self, new: &Tree) -> Vec<crate::Range> {
        let mut len = 0u32;
        unsafe {
            let ranges = ts_tree_get_changed_ranges(self.ptr, new.ptr, &mut len);
            if ranges.is_null() {
                return Vec::new();
            }
            let vec = std::slice::from_raw_parts(ranges, len as usize).to_vec();
            free(ranges.cast());
            vec
        }
    }
}

impl fmt::Debug for Tree {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{Tree {:?}}}", self.root_node())
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        unsafe { ts_tree_delete(self.ptr) }
    }
}

impl Clone for Tree {
    fn clone(&self) -> Self {
        unsafe {
            Tree {
                ptr: ts_tree_copy(self.ptr),
            }
        }
    }
}

unsafe impl Send for Tree {}
unsafe impl Sync for Tree {}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct InputEdit {
    pub start_byte: u32,
    pub old_end_byte: u32,
    pub new_end_byte: u32,
    pub start_point: Point,
    pub old_end_point: Point,
    pub new_end_point: Point,
}

impl InputEdit {
    /// returns the offset between the old end of the edit and the new end of
    /// the edit. This offset needs to be added to every position that occurs
    /// after `self.old_end_byte` to may it to its old position
    ///
    /// This function assumes that the the source-file is smaller than 2GiB
    pub fn offset(&self) -> i32 {
        self.new_end_byte as i32 - self.old_end_byte as i32
    }
}

extern "C" {
    /// Create a shallow copy of the syntax tree. This is very fast. You need to
    /// copy a syntax tree in order to use it on more than one thread at a time,
    /// as syntax trees are not thread safe.
    fn ts_tree_copy(self_: NonNull<SyntaxTreeData>) -> NonNull<SyntaxTreeData>;
    /// Delete the syntax tree, freeing all of the memory that it used.
    fn ts_tree_delete(self_: NonNull<SyntaxTreeData>);
    /// Get the root node of the syntax tree.
    fn ts_tree_root_node<'tree>(self_: NonNull<SyntaxTreeData>) -> NodeRaw;
    /// Edit the syntax tree to keep it in sync with source code that has been
    /// edited.
    ///
    /// You must describe the edit both in terms of byte offsets and in terms of
    /// row/column coordinates.
    fn ts_tree_edit(self_: NonNull<SyntaxTreeData>, edit: &InputEdit);
    /// The ranges whose structure changed from `old_tree` to `new_tree`, in an array to free.
    fn ts_tree_get_changed_ranges(
        old_tree: NonNull<SyntaxTreeData>,
        new_tree: NonNull<SyntaxTreeData>,
        length: &mut u32,
    ) -> *mut crate::Range;
    /// The C library's `free`: tree-sitter allocates with `malloc` unless an allocator is set,
    /// which nothing here does.
    fn free(ptr: *mut std::ffi::c_void);
}
