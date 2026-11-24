//! The syntax tree of a buffer for plugins (`buffer:syntax_*`).
//!
//! Tree-sitter nodes borrow the tree, which changes with every edit, so a node given to Lua
//! is a snapshot (a table): its kind, its range in chars, bytes and lines. Navigating from
//! a node (`syntax_parent`, `syntax_children`) finds it again in the current tree by its byte
//! range and kind; a node that is no longer there is an error.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use helix_core::{
    ropey::RopeSlice,
    syntax::Syntax,
    tree_sitter::{InactiveQueryCursor, Node, Query, RopeInput, Tree},
};
use mlua::prelude::*;

/// A node as given to Lua.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NodeInfo {
    pub kind: String,
    pub named: bool,
    pub missing: bool,
    pub extra: bool,
    /// Whether it is or contains a syntax error.
    pub has_error: bool,
    /// chars, `to` exclusive
    pub from: usize,
    pub to: usize,
    pub start_byte: u32,
    pub end_byte: u32,
    /// 1-based lines, 0-based char columns
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub child_count: u32,
    pub named_child_count: u32,
}

impl NodeInfo {
    pub(crate) fn new(node: &Node, text: RopeSlice) -> Self {
        let len = text.len_bytes();
        let start_byte = node.start_byte();
        let end_byte = node.end_byte();
        let from = text.byte_to_char((start_byte as usize).min(len));
        let to = text.byte_to_char((end_byte as usize).min(len));
        let position = |char: usize| {
            let line = text.char_to_line(char);
            (line + 1, char - text.line_to_char(line))
        };
        Self {
            kind: node.kind().to_string(),
            named: node.is_named(),
            missing: node.is_missing(),
            extra: node.is_extra(),
            has_error: node.has_error(),
            from,
            to,
            start_byte,
            end_byte,
            start: position(from),
            end: position(to),
            child_count: node.child_count(),
            named_child_count: node.named_child_count(),
        }
    }

    pub(crate) fn into_lua(self, lua: &Lua) -> LuaResult<LuaTable> {
        let table = lua.create_table_with_capacity(0, 15)?;
        table.raw_set("type", self.kind)?;
        table.raw_set("named", self.named)?;
        table.raw_set("missing", self.missing)?;
        table.raw_set("extra", self.extra)?;
        table.raw_set("has_error", self.has_error)?;
        table.raw_set("from", self.from)?;
        table.raw_set("to", self.to)?;
        table.raw_set("start_byte", self.start_byte)?;
        table.raw_set("end_byte", self.end_byte)?;
        table.raw_set("start_line", self.start.0)?;
        table.raw_set("start_col", self.start.1)?;
        table.raw_set("end_line", self.end.0)?;
        table.raw_set("end_col", self.end.1)?;
        table.raw_set("child_count", self.child_count)?;
        table.raw_set("named_child_count", self.named_child_count)?;
        Ok(table)
    }
}

/// The byte range and kind of a node table given back by a plugin.
pub(crate) struct NodeRef {
    pub kind: String,
    pub start_byte: u32,
    pub end_byte: u32,
}

impl NodeRef {
    pub(crate) fn from_lua(table: &LuaTable) -> LuaResult<Self> {
        let field = |name: &str| -> LuaResult<u32> {
            table.get::<Option<u32>>(name)?.ok_or_else(|| {
                LuaError::RuntimeError(format!("Not a syntax node: it has no `{name}`"))
            })
        };
        let kind: Option<String> = table.get("type")?;
        Ok(Self {
            kind: kind.ok_or_else(|| {
                LuaError::RuntimeError("Not a syntax node: it has no `type`".into())
            })?,
            start_byte: field("start_byte")?,
            end_byte: field("end_byte")?,
        })
    }

    /// Whether `node` has this kind and range.
    fn is(&self, node: &Node) -> bool {
        node.start_byte() == self.start_byte
            && node.end_byte() == self.end_byte
            && node.kind() == self.kind
    }

    /// This (empty) node among the nodes of `root` that touch its place: few, as at most two
    /// children of a node touch a place besides the empty ones there.
    fn touching<'a>(&self, root: Node<'a>) -> Option<Node<'a>> {
        let mut left = vec![root];
        while let Some(node) = left.pop() {
            if self.is(&node) {
                return Some(node);
            }
            left.extend(
                node.children()
                    .take_while(|child| child.start_byte() <= self.start_byte)
                    .filter(|child| child.end_byte() >= self.end_byte),
            );
        }
        None
    }

    /// The node in the current tree: the deepest node of the range, then up through the
    /// nodes of the same range (a statement and its expression) to the one of this kind.
    pub(crate) fn find<'a>(&self, syntax: &'a Syntax) -> LuaResult<Node<'a>> {
        self.place(syntax).map(|place| place.node)
    }

    /// The node and the trees around the one it is in. It is looked for in the smallest
    /// injected language that holds its range first: the root of the code in a Markdown fence
    /// has the range of the fence's content, a node of the Markdown tree.
    fn place<'a>(&self, syntax: &'a Syntax) -> LuaResult<Place<'a>> {
        // The layers of a range are those that hold its last byte too.
        let last = self.end_byte.saturating_sub(1).max(self.start_byte);
        let mut outer: Vec<&Tree> = syntax
            .layers_for_byte_range(self.start_byte, last)
            .filter_map(|layer| syntax.layer(layer).tree())
            .collect();
        while let Some(tree) = outer.pop() {
            let mut node = tree
                .root_node()
                .descendant_for_byte_range(self.start_byte, self.end_byte);
            while let Some(same) = node
                .filter(|node| node.start_byte() == self.start_byte)
                .filter(|node| node.end_byte() == self.end_byte)
            {
                if same.kind() == self.kind {
                    return Ok(Place { outer, node: same });
                }
                node = same.parent();
            }
            // A missing node (the `;` that is not there) is empty, and the lookup by range
            // gives the node before or after it.
            if self.start_byte == self.end_byte {
                if let Some(node) = self.touching(tree.root_node()) {
                    return Ok(Place { outer, node });
                }
            }
        }
        Err(LuaError::RuntimeError(format!(
            "The syntax node {} ({}..{}) is no longer in the tree",
            self.kind, self.start_byte, self.end_byte
        )))
    }
}

/// A node and the trees around its own: the document's first, then one per injected language
/// down to the one that holds the node's.
struct Place<'a> {
    outer: Vec<&'a Tree>,
    node: Node<'a>,
}

impl Place<'_> {
    /// The node around this one: its parent, or for the root of an injected language the
    /// smallest node of the language around it that spans it. `None` at the document's root.
    fn parent(mut self) -> Option<Self> {
        if let Some(parent) = self.node.parent() {
            self.node = parent;
            return Some(self);
        }
        let (start, end) = (self.node.start_byte(), self.node.end_byte());
        while let Some(tree) = self.outer.pop() {
            if let Some(node) = tree.root_node().descendant_for_byte_range(start, end) {
                self.node = node;
                return Some(self);
            }
        }
        None
    }
}

/// The buffer's syntax tree, or an error naming why there is none.
pub(crate) fn syntax_of(doc: &helix_view::Document) -> LuaResult<&Syntax> {
    doc.syntax().ok_or_else(|| {
        LuaError::RuntimeError(format!(
            "The buffer has no syntax tree (language: {})",
            doc.language_name().unwrap_or("none")
        ))
    })
}

/// The smallest node spanning the chars `from..to` (named only, unless `named` is off),
/// across injected languages (the code in a Markdown fence).
pub(crate) fn node_at(
    doc: &helix_view::Document,
    from: usize,
    to: usize,
    named: bool,
) -> LuaResult<NodeInfo> {
    let syntax = syntax_of(doc)?;
    let text = doc.text().slice(..);
    let to = to.min(text.len_chars());
    let from = from.min(to);
    let (start, end) = (text.char_to_byte(from) as u32, text.char_to_byte(to) as u32);
    let node = if named {
        syntax.named_descendant_for_byte_range(start, end)
    } else {
        syntax.descendant_for_byte_range(start, end)
    };
    let node = node.unwrap_or_else(|| syntax.tree().root_node());
    Ok(NodeInfo::new(&node, text))
}

pub(crate) fn root(doc: &helix_view::Document) -> LuaResult<NodeInfo> {
    let syntax = syntax_of(doc)?;
    Ok(NodeInfo::new(
        &syntax.tree().root_node(),
        doc.text().slice(..),
    ))
}

/// The nearest node around `node` that a plugin can tell from it, across injected languages:
/// the parent of the root of the code in a Markdown fence is the fence's content.
///
/// Broken text gives error nodes inside error nodes of the same range. A node table holds a
/// kind and a byte range only, so the outer one would be found as the inner one again and
/// a plugin walking up (`while node do node = buffer:syntax_parent(node) end`) would never
/// reach the root.
pub(crate) fn parent(doc: &helix_view::Document, node: &NodeRef) -> LuaResult<Option<NodeInfo>> {
    let syntax = syntax_of(doc)?;
    let text = doc.text().slice(..);
    let mut parent = node.place(syntax)?.parent();
    while let Some(same) = parent.take_if(|parent| node.is(&parent.node)) {
        parent = same.parent();
    }
    Ok(parent.map(|parent| NodeInfo::new(&parent.node, text)))
}

pub(crate) fn children(
    doc: &helix_view::Document,
    node: &NodeRef,
    named: bool,
) -> LuaResult<Vec<NodeInfo>> {
    let syntax = syntax_of(doc)?;
    let text = doc.text().slice(..);
    Ok(node
        .find(syntax)?
        .children()
        .filter(|child| !named || child.is_named())
        .map(|child| NodeInfo::new(&child, text))
        .collect())
}

/// The text of the node, from the current buffer.
pub(crate) fn node_text(doc: &helix_view::Document, node: &NodeRef) -> LuaResult<String> {
    let syntax = syntax_of(doc)?;
    let found = node.find(syntax)?;
    let text = doc.text().slice(..);
    Ok(text
        .byte_slice(found.start_byte() as usize..found.end_byte() as usize)
        .to_string())
}

/// One match of a query: its pattern (1-based, in the order of the query's source) and its
/// captures (name and node) in the order of the query.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QueryMatchInfo {
    pub pattern: usize,
    pub captures: Vec<(String, NodeInfo)>,
}

/// How many matches a query gives without a `max`, and at most.
pub(crate) const QUERY_DEFAULT_MAX: usize = 10_000;
pub(crate) const QUERY_MAX: usize = 1_000_000;
/// Compiled queries kept (per language and source), so a plugin can run one on every change.
const QUERY_CACHE_SIZE: usize = 64;

thread_local! {
    static QUERIES: RefCell<HashMap<(String, String), Arc<Query>>> = RefCell::new(HashMap::new());
}

/// The query `source` compiled for the buffer's language (its root layer). Predicates other
/// than the text ones (`#eq?`, `#match?`, `#any-of?`...) are accepted and ignored.
fn compile(doc: &helix_view::Document, syntax: &Syntax, source: &str) -> LuaResult<Arc<Query>> {
    let language = doc.language_name().unwrap_or("").to_string();
    let key = (language, source.to_string());
    if let Some(query) = QUERIES.with(|cache| cache.borrow().get(&key).cloned()) {
        return Ok(query);
    }
    let grammar = syntax.tree().root_node().grammar();
    let query = Query::new(grammar, source, |_, _| Ok(()))
        .map_err(|err| LuaError::RuntimeError(format!("Invalid query: {err}")))?;
    let query = Arc::new(query);
    QUERIES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= QUERY_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(key, query.clone());
    });
    Ok(query)
}

/// The matches of the query in the chars `from..to` of the buffer (the whole buffer without
/// them), at most `max`, in the order of the document.
pub(crate) fn query(
    doc: &helix_view::Document,
    source: &str,
    range: Option<(usize, usize)>,
    max: usize,
) -> LuaResult<Vec<QueryMatchInfo>> {
    let syntax = syntax_of(doc)?;
    let query = compile(doc, syntax, source)?;
    let text = doc.text().slice(..);
    let bytes = match range {
        Some((from, to)) => {
            let to = to.min(text.len_chars());
            let from = from.min(to);
            text.char_to_byte(from) as u32..text.char_to_byte(to) as u32
        }
        None => 0..u32::MAX,
    };
    let root = syntax.tree().root_node();
    let mut cursor = InactiveQueryCursor::new(bytes, helix_core::syntax::TREE_SITTER_MATCH_LIMIT)
        .execute_query(&query, &root, RopeInput::new(text));
    let mut matches = Vec::new();
    while matches.len() < max {
        let Some(found) = cursor.next_match() else {
            break;
        };
        let captures = found
            .matched_nodes()
            .map(|matched| {
                (
                    query.capture_name(matched.capture).to_string(),
                    NodeInfo::new(&matched.node, text),
                )
            })
            .collect();
        matches.push(QueryMatchInfo {
            pattern: found.pattern().idx() + 1,
            captures,
        });
    }
    Ok(matches)
}

/// Builds the Lua list of matches: `{ { pattern = n, captures = { { name = .., node = .. } } } }`.
pub(crate) fn matches_into_lua(lua: &Lua, matches: Vec<QueryMatchInfo>) -> LuaResult<LuaTable> {
    let list = lua.create_table_with_capacity(matches.len(), 0)?;
    for found in matches {
        let item = lua.create_table_with_capacity(0, 2)?;
        item.raw_set("pattern", found.pattern)?;
        let captures = lua.create_table_with_capacity(found.captures.len(), 0)?;
        for (name, node) in found.captures {
            let capture = lua.create_table_with_capacity(0, 2)?;
            capture.raw_set("name", name)?;
            capture.raw_set("node", node.into_lua(lua)?)?;
            captures.push(capture)?;
        }
        item.raw_set("captures", captures)?;
        list.push(item)?;
    }
    Ok(list)
}

/// Builds the Lua list of nodes.
pub(crate) fn nodes_into_lua(lua: &Lua, nodes: Vec<NodeInfo>) -> LuaResult<LuaTable> {
    let list = lua.create_table_with_capacity(nodes.len(), 0)?;
    for node in nodes {
        list.push(node.into_lua(lua)?)?;
    }
    Ok(list)
}
