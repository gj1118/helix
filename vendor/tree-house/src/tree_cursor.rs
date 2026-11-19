use std::collections::VecDeque;

use crate::tree_sitter::Node;
use crate::{Layer, Syntax};

pub struct TreeCursor<'tree> {
    syntax: &'tree Syntax,
    current: Layer,
    cursor: tree_sitter::TreeCursor<'tree>,
}

impl<'tree> TreeCursor<'tree> {
    pub(crate) fn new(syntax: &'tree Syntax) -> Self {
        let cursor = syntax.tree().walk();

        Self {
            syntax,
            current: syntax.root,
            cursor,
        }
    }

    pub fn node(&self) -> Node<'tree> {
        self.cursor.node()
    }

    pub fn goto_parent(&mut self) -> bool {
        if self.cursor.goto_parent() {
            return true;
        };

        loop {
            // Ascend to the parent layer if one exists.
            let Some(parent) = self.syntax.layer(self.current).parent else {
                return false;
            };

            self.current = parent;
            if let Some(tree) = self.syntax.layer(self.current).tree() {
                self.cursor = tree.walk();
                break;
            }
        }

        true
    }

    pub fn goto_parent_with<P>(&mut self, predicate: P) -> bool
    where
        P: Fn(&Node) -> bool,
    {
        while self.goto_parent() {
            if predicate(&self.node()) {
                return true;
            }
        }

        false
    }

    pub fn goto_first_child(&mut self) -> bool {
        let range = self.cursor.node().byte_range();
        let layer = self.syntax.layer(self.current);
        if let Some((layer, tree)) = layer
            .injection_at_byte_idx(range.start)
            .filter(|injection| injection.range.end >= range.end)
            .and_then(|injection| {
                Some((injection.layer, self.syntax.layer(injection.layer).tree()?))
            })
        {
            // Switch to the child layer.
            self.current = layer;
            self.cursor = tree.walk();
            return true;
        }

        self.cursor.goto_first_child()
    }

    pub fn goto_next_sibling(&mut self) -> bool {
        self.cursor.goto_next_sibling()
    }

    pub fn goto_previous_sibling(&mut self) -> bool {
        self.cursor.goto_previous_sibling()
    }

    pub fn reset_to_byte_range(&mut self, start: u32, end: u32) {
        let (layer, tree) = self.syntax.layer_and_tree_for_byte_range(start, end);
        self.current = layer;
        self.cursor = tree.walk();

        loop {
            let node = self.cursor.node();
            if start < node.start_byte() || end > node.end_byte() {
                self.cursor.goto_parent();
                break;
            }
            if self.cursor.goto_first_child_for_byte(start).is_none() {
                break;
            }
        }
    }

    /// Returns an iterator over the children of the node the TreeCursor is on
    /// at the time this is called.
    pub fn children<'a>(&'a mut self) -> ChildIter<'a, 'tree> {
        let parent = self.node();

        ChildIter {
            cursor: self,
            parent,
        }
    }
}

pub struct ChildIter<'a, 'tree> {
    cursor: &'a mut TreeCursor<'tree>,
    parent: Node<'tree>,
}

impl<'tree> Iterator for ChildIter<'_, 'tree> {
    type Item = Node<'tree>;

    fn next(&mut self) -> Option<Self::Item> {
        // first iteration, just visit the first child
        if self.cursor.node() == self.parent {
            self.cursor.goto_first_child().then(|| self.cursor.node())
        } else {
            self.cursor.goto_next_sibling().then(|| self.cursor.node())
        }
    }
}

impl<'cursor, 'tree> IntoIterator for &'cursor mut TreeCursor<'tree> {
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

pub struct TreeRecursiveWalker<'cursor, 'tree> {
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
            self.cursor.cursor.reset(&queued);

            if !self.cursor.goto_first_child() {
                continue;
            }

            return Some(self.cursor.node());
        }

        None
    }
}
