use std::{
    borrow::Cow,
    cmp::Reverse,
    mem::take,
    ops::{Index, IndexMut},
};

use hashbrown::HashMap;
use kstring::KString;
use ropey::RopeSlice;
use tree_sitter::{Capture, Node, Pattern};

use crate::highlighter::Highlight;
use crate::incremental::{self, Shift};
use crate::injections_query::Within;
use crate::{InjectionsQuery, LanguageConfig, LanguageLoader, Layer, Range, Syntax};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Scope(u32);

impl Scope {
    const ROOT: Scope = Scope(0);
    fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
pub struct Locals {
    scopes: Vec<ScopeData>,
}

impl Default for Locals {
    fn default() -> Self {
        let mut scopes = Vec::with_capacity(4);
        scopes.push(ScopeData {
            definitions: HashMap::new(),
            range: 0..u32::MAX,
            inherit: false,
            children: Vec::new(),
            parent: None,
        });

        Self { scopes }
    }
}

impl Locals {
    fn push(&mut self, scope: ScopeData) -> Scope {
        let new_scope_id = Scope(self.scopes.len() as u32);
        let parent = scope
            .parent
            .expect("push cannot be used for the root layer");
        self[parent].children.push(new_scope_id);
        self.scopes.push(scope);
        new_scope_id
    }

    pub fn lookup_reference(&self, mut scope: Scope, name: &str) -> Option<&Definition> {
        loop {
            let scope_data = &self[scope];
            if let Some(def) = scope_data.definitions.get(name) {
                return Some(def);
            }
            if !scope_data.inherit {
                break;
            }
            scope = scope_data.parent?;
        }

        None
    }

    /// The scopes and their definitions, for `Syntax::structure` (PATCH (helix fork)).
    pub(crate) fn describe(&self, out: &mut String) {
        fn scope(locals: &Locals, id: Scope, depth: usize, out: &mut String) {
            use std::fmt::Write;
            let data = &locals[id];
            let mut definitions: Vec<_> = data.definitions.iter().collect();
            definitions.sort_by_key(|(name, definition)| (definition.range.start, name.as_str()));
            let _ = write!(
                out,
                "{:depth$}scope {:?} inherit {}:",
                "", data.range, data.inherit
            );
            for (name, definition) in definitions {
                let capture = definition.capture.idx();
                let _ = write!(out, " {name}={capture}@{:?}", definition.range);
            }
            out.push('\n');
            for &child in &data.children {
                scope(locals, child, depth + 1, out);
            }
        }
        scope(self, Scope::ROOT, 1, out);
    }

    pub fn scope_cursor(&self, pos: u32) -> ScopeCursor<'_> {
        let mut scope = Scope::ROOT;
        let mut scope_stack = Vec::with_capacity(8);
        loop {
            let scope_data = &self[scope];
            let child_idx = scope_data
                .children
                .partition_point(|&child| self[child].range.end < pos);
            scope_stack.push((scope, child_idx as u32));
            let Some(&child) = scope_data.children.get(child_idx) else {
                break;
            };
            if pos < self[child].range.start {
                break;
            }
            scope = child;
        }
        ScopeCursor {
            locals: self,
            scope_stack,
        }
    }
}

impl Index<Scope> for Locals {
    type Output = ScopeData;

    fn index(&self, scope: Scope) -> &Self::Output {
        &self.scopes[scope.idx()]
    }
}

impl IndexMut<Scope> for Locals {
    fn index_mut(&mut self, scope: Scope) -> &mut Self::Output {
        &mut self.scopes[scope.idx()]
    }
}

#[derive(Debug)]
pub struct ScopeCursor<'a> {
    pub locals: &'a Locals,
    scope_stack: Vec<(Scope, u32)>,
}

impl ScopeCursor<'_> {
    pub fn advance(&mut self, to: u32) -> Scope {
        let (mut active_scope, mut child_idx) = self.scope_stack.pop().unwrap();
        loop {
            let scope_data = &self.locals[active_scope];
            if to < scope_data.range.end {
                break;
            }
            (active_scope, child_idx) = self.scope_stack.pop().unwrap();
            child_idx += 1;
        }
        'outer: loop {
            let scope_data = &self.locals[active_scope];
            loop {
                let Some(&child) = scope_data.children.get(child_idx as usize) else {
                    break 'outer;
                };
                if self.locals[child].range.start > to {
                    break 'outer;
                }
                if to < self.locals[child].range.end {
                    self.scope_stack.push((active_scope, child_idx));
                    active_scope = child;
                    child_idx = 0;
                    break;
                }
                child_idx += 1;
            }
        }
        self.scope_stack.push((active_scope, child_idx));
        active_scope
    }

    pub fn current_scope(&self) -> Scope {
        // The root scope is always active so `scope_stack` is never empty.
        self.scope_stack.last().unwrap().0
    }
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub capture: Capture,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct ScopeData {
    definitions: HashMap<KString, Definition>,
    range: Range,
    inherit: bool,
    /// A list of sorted, non-overlapping child scopes.
    ///
    /// See the docs of the `Locals` type: locals information is laid out like a tree - similar
    /// to injections - per injection layer.
    children: Vec<Scope>,
    parent: Option<Scope>,
}

impl Syntax {
    pub(crate) fn run_local_query(
        &mut self,
        layer: Layer,
        edits: &[tree_sitter::InputEdit],
        source: RopeSlice<'_>,
        loader: &impl LanguageLoader,
        full: bool,
    ) {
        let layer_data = &mut self.layer_mut(layer);
        let Some(LanguageConfig {
            ref injection_query,
            ..
        }) = loader.get_config(layer_data.language)
        else {
            return;
        };
        let definition_captures = injection_query.local_definition_captures.load();
        if definition_captures.is_empty() {
            return;
        }

        // PATCH (helix fork): the captures of the last update, moved by the edits. A layer not
        // changed by them has the same ones; a changed layer is queried where the edits changed
        // it (see `crate::incremental`), or all of it. The scopes are built from the captures.
        let mut captures = take(&mut layer_data.local_captures);
        let shift = Shift::new(edits);
        for capture in &mut captures {
            capture.shift(&shift);
        }
        if layer_data.flags.modified || !layer_data.locals_queried {
            layer_data.locals_queried = true;
            let root = layer_data.parse_tree.as_ref().unwrap().root_node();
            let query = |within: Within| {
                injection_query.execute_locals(&root, within, source, &definition_captures)
            };
            let changed = layer_data
                .changed_ranges
                .as_ref()
                .filter(|_| !full && !layer_data.flags.ranges_changed);
            let updated = changed.and_then(|changed| {
                let pieces = incremental::changed_pieces(changed, edits);
                let mut updated = incremental::update_matches(
                    take(&mut captures),
                    &pieces,
                    &root.byte_range(),
                    layer_data.shell.as_ref(),
                    |capture: &LocalCapture| capture.anchor.as_ref(),
                    |range| query(Within::Range(range)),
                    |shell| query(Within::Shell(shell)),
                )?;
                updated.sort_by_key(LocalCapture::order);
                Some(updated)
            });
            captures = updated.unwrap_or_else(|| query(Within::Range(0..u32::MAX)));
        }

        let mut locals = Locals::default();
        let mut scope = Scope::ROOT;
        for capture in &captures {
            let range = capture.range.clone();
            while range.start >= locals[scope].range.end {
                scope = locals[scope].parent.expect("root node covers entire range");
            }
            match &capture.kind {
                LocalKind::Scope { inherit } => {
                    scope = locals.push(ScopeData {
                        definitions: HashMap::new(),
                        range,
                        inherit: *inherit,
                        children: Vec::new(),
                        parent: Some(scope),
                    });
                }
                LocalKind::Definition { name } => {
                    let capture = capture.capture;
                    locals[scope]
                        .definitions
                        .insert(name.clone(), Definition { capture, range });
                }
            }
        }
        // NOTE: `local.reference` captures are handled by the highlighter and are not
        // considered during parsing.

        let layer_data = self.layer_mut(layer);
        layer_data.locals = locals;
        layer_data.local_captures = captures;
    }
}

/// A capture of the locals query as a layer keeps it between updates (PATCH (helix fork)).
#[derive(Debug, Clone)]
pub(crate) struct LocalCapture {
    /// Where a query limited to a range finds this capture's match again (see
    /// `crate::incremental`), if that can be told.
    anchor: Option<Range>,
    range: Range,
    pattern: Pattern,
    capture: Capture,
    kind: LocalKind,
}

#[derive(Debug, Clone)]
enum LocalKind {
    Scope { inherit: bool },
    Definition { name: KString },
}

impl LocalCapture {
    fn shift(&mut self, shift: &Shift) {
        if let Some(anchor) = &mut self.anchor {
            *anchor = shift.range(anchor);
        }
        self.range = shift.range(&self.range);
    }

    /// The order a query yields captures in: by node, of nodes starting together the one of the
    /// first pattern, then the outer one.
    fn order(&self) -> (u32, Pattern, Reverse<u32>, usize) {
        (
            self.range.start,
            self.pattern,
            Reverse(self.range.end),
            self.capture.idx(),
        )
    }
}

impl InjectionsQuery {
    /// The scope and definition captures of the locals query on the tree of `root`, limited to
    /// `range` (see `crate::incremental` for which matches that finds), in the order of
    /// `LocalCapture::order`.
    fn execute_locals(
        &self,
        root: &Node,
        within: Within,
        source: RopeSlice,
        definition_captures: &HashMap<Capture, Highlight>,
    ) -> Vec<LocalCapture> {
        let Some(mut cursor) = within.cursor(&self.local_query, root, source) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        while let Some(query_match) = cursor.next_match() {
            let pattern = query_match.pattern();
            let anchor = incremental::anchor_source(
                query_match.matched_nodes(),
                &self.local_anchor_captures[pattern.idx()],
            );
            for matched in query_match.matched_nodes() {
                let range = matched.node.byte_range();
                let kind = if Some(matched.capture) == self.local_scope_capture {
                    let inherit = !self.not_scope_inherits.contains(&pattern);
                    LocalKind::Scope { inherit }
                } else if definition_captures.contains_key(&matched.capture) {
                    let name = match source
                        .byte_slice(range.start as usize..range.end as usize)
                        .into()
                    {
                        Cow::Borrowed(inner) => KString::from_ref(inner),
                        Cow::Owned(inner) => KString::from_string(inner),
                    };
                    LocalKind::Definition { name }
                } else {
                    continue;
                };
                found.push((
                    LocalCapture {
                        anchor: None,
                        range,
                        pattern,
                        capture: matched.capture,
                        kind,
                    },
                    anchor.clone(),
                ));
            }
        }
        let sources: Vec<_> = found.iter().map(|(_, source)| source.clone()).collect();
        let anchors = incremental::anchors(root, &sources);
        let mut captures: Vec<_> = found
            .into_iter()
            .zip(anchors)
            .map(|((capture, _), anchor)| LocalCapture { anchor, ..capture })
            .collect();
        captures.sort_by_key(LocalCapture::order);
        captures
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cursor() {
        let mut locals = Locals::default();
        let scope1 = locals.push(ScopeData {
            definitions: Default::default(),
            range: 5..105,
            inherit: true,
            // NOTE: the subsequent call to `push` below will add scope2 to scope1's children.
            children: Default::default(),
            parent: Some(Scope::ROOT),
        });
        let scope2 = locals.push(ScopeData {
            definitions: Default::default(),
            range: 10..100,
            inherit: true,
            children: Default::default(),
            parent: Some(scope1),
        });

        let mut cursor = locals.scope_cursor(0);
        assert_eq!(cursor.current_scope(), Scope::ROOT);
        assert_eq!(cursor.advance(3), Scope::ROOT);
        assert_eq!(cursor.advance(5), scope1);
        assert_eq!(cursor.advance(8), scope1);
        assert_eq!(cursor.advance(10), scope2);
        assert_eq!(cursor.advance(50), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(105), Scope::ROOT);
        assert_eq!(cursor.advance(110), Scope::ROOT);

        let mut cursor = locals.scope_cursor(8);
        assert_eq!(cursor.current_scope(), scope1);
        assert_eq!(cursor.advance(10), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(110), Scope::ROOT);

        let mut cursor = locals.scope_cursor(10);
        assert_eq!(cursor.current_scope(), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(110), Scope::ROOT);
    }
}
