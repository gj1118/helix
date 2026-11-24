use std::cmp::Reverse;
use std::iter::{from_fn, Peekable};
use std::mem::take;
use std::sync::Arc;

use arc_swap::ArcSwap;
use hashbrown::{HashMap, HashSet};
use once_cell::sync::Lazy;
use regex_cursor::engines::meta::Regex;
use ropey::RopeSlice;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::highlighter::Highlight;
use crate::incremental::{self, Shift};
use crate::locals::Locals;
use crate::parse::LayerUpdateFlags;
use crate::{Injection, Language, Layer, LayerData, Range, Syntax, TREE_SITTER_MATCH_LIMIT};
use tree_sitter::{
    query::{self, InvalidPredicateError, UserPredicate},
    Capture, Grammar, InactiveQueryCursor, MatchedNodeIdx, Node, Pattern, Query, QueryMatch,
};

const SHEBANG: &str = r"#!\s*(?:\S*[/\\](?:env\s+(?:\-\S+\s+)*)?)?([^\s\.\d]+)";
static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(SHEBANG).unwrap());

#[derive(Clone, Default, Debug)]
pub struct InjectionProperties {
    include_children: IncludedChildren,
    language: Option<Box<str>>,
    combined: bool,
}

/// An indicator in the document or query source file which used by the loader to know which
/// language an injection should use.
///
/// For example if a query sets a property `(#set! injection.language "rust")` then the loader
/// should load the Rust language. Alternatively the loader might be asked to load a language
/// based on some text in the document, for example a markdown code fence language name.
#[derive(Debug, Clone, Copy)]
pub enum InjectionLanguageMarker<'a> {
    /// The language is specified by name in the injection query itself.
    ///
    /// For example `(#set! injection.language "rust")`. These names should match exactly and so
    /// they can be looked up by equality - very efficiently.
    Name(&'a str),
    /// The language is specified by name - or similar - within the parsed document.
    ///
    /// This is slightly different than the `ExactName` variant: within a document you might
    /// specify Markdown as "md" or "markdown" for example. The loader should look up the language
    /// name by longest matching regex.
    Match(RopeSlice<'a>),
    Filename(RopeSlice<'a>),
    Shebang(RopeSlice<'a>),
}

#[derive(Clone, Debug)]
pub struct InjectionQueryMatch<'tree> {
    include_children: IncludedChildren,
    language: Language,
    scope: Option<InjectionScope>,
    node: Node<'tree>,
    last_match: bool,
    pattern: Pattern,
}

/// A match of the injections query as a layer keeps it between updates: what building the
/// layer's injections takes, without the nodes of the tree (PATCH (helix fork)).
#[derive(Clone, Debug)]
pub(crate) struct InjectionMatch {
    /// Where a query limited to a range finds this match again (see `crate::incremental`), if
    /// that can be told: a layer with a match whose anchor cannot is queried whole.
    anchor: Option<Range>,
    /// The `injection.content` node.
    node: Range,
    /// The node within the layer's ranges, minus the children it excludes: what is injected.
    ranges: Vec<Range>,
    language: Language,
    scope: Option<InjectionScope>,
    include_children: IncludedChildren,
    pattern: Pattern,
    last_match: bool,
}

impl InjectionMatch {
    fn shift(&mut self, shift: &Shift) {
        if let Some(anchor) = &mut self.anchor {
            *anchor = shift.range(anchor);
        }
        self.node = shift.range(&self.node);
        for range in &mut self.ranges {
            *range = shift.range(range);
        }
    }

    /// The order of a full query's matches: by node, an outer node before the ones in it, then
    /// by pattern (the last pattern matching a node wins).
    fn order(&self) -> (u32, Reverse<u32>, Pattern, u32, Reverse<u32>) {
        let (node, anchor) = (&self.node, self.anchor.clone().unwrap_or(0..u32::MAX));
        (
            node.start,
            Reverse(node.end),
            self.pattern,
            anchor.start,
            Reverse(anchor.end),
        )
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum InjectionScope {
    Match {
        id: u32,
    },
    Pattern {
        pattern: Pattern,
        language: Language,
    },
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum IncludedChildren {
    #[default]
    None,
    All,
    Unnamed,
}

#[derive(Debug)]
pub struct InjectionsQuery {
    injection_query: Query,
    injection_properties: HashMap<Pattern, InjectionProperties>,
    injection_content_capture: Option<Capture>,
    injection_language_capture: Option<Capture>,
    injection_filename_capture: Option<Capture>,
    injection_shebang_capture: Option<Capture>,
    /// For each pattern, the captures its anchor is found from, with how many levels it is above
    /// them (PATCH (helix fork), see `crate::incremental`).
    anchor_captures: Vec<Vec<(Capture, u32)>>,
    // Note that the injections query is concatenated with the locals query.
    pub(crate) local_query: Query,
    // TODO: Use a Vec<bool> instead?
    pub(crate) not_scope_inherits: HashSet<Pattern>,
    pub(crate) local_scope_capture: Option<Capture>,
    pub(crate) local_definition_captures: ArcSwap<HashMap<Capture, Highlight>>,
    /// `anchor_captures` of the locals query.
    pub(crate) local_anchor_captures: Vec<Vec<(Capture, u32)>>,
}

impl InjectionsQuery {
    pub fn new(
        grammar: Grammar,
        injection_query_text: &str,
        local_query_text: &str,
    ) -> Result<Self, query::ParseError> {
        let mut query_source =
            String::with_capacity(injection_query_text.len() + local_query_text.len());
        query_source.push_str(injection_query_text);
        query_source.push_str(local_query_text);

        let mut injection_properties: HashMap<Pattern, InjectionProperties> = HashMap::new();
        let mut not_scope_inherits = HashSet::new();
        let injection_query = Query::new(grammar, injection_query_text, |pattern, predicate| {
            match predicate {
                // injections
                UserPredicate::SetProperty {
                    key: "injection.include-unnamed-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::Unnamed
                }
                UserPredicate::SetProperty {
                    key: "injection.include-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::All
                }
                UserPredicate::SetProperty {
                    key: "injection.language",
                    val: Some(lang),
                } => injection_properties.entry(pattern).or_default().language = Some(lang.into()),
                UserPredicate::SetProperty {
                    key: "injection.combined",
                    val: None,
                } => injection_properties.entry(pattern).or_default().combined = true,
                predicate => {
                    return Err(InvalidPredicateError::unknown(predicate));
                }
            }
            Ok(())
        })?;
        let mut local_query = Query::new(grammar, local_query_text, |pattern, predicate| {
            match predicate {
                UserPredicate::SetProperty {
                    key: "local.scope-inherits",
                    val,
                } => {
                    if val.is_some_and(|val| val != "true") {
                        not_scope_inherits.insert(pattern);
                    }
                }
                predicate => {
                    return Err(InvalidPredicateError::unknown(predicate));
                }
            }
            Ok(())
        })?;

        // The injection queries do not track references - these are read by the highlight
        // query instead.
        local_query.disable_capture("local.reference");

        let anchor_captures = incremental::anchor_captures(&injection_query, injection_query_text);
        Ok(InjectionsQuery {
            anchor_captures,
            injection_properties,
            injection_content_capture: injection_query.get_capture("injection.content"),
            injection_language_capture: injection_query.get_capture("injection.language"),
            injection_filename_capture: injection_query.get_capture("injection.filename"),
            injection_shebang_capture: injection_query.get_capture("injection.shebang"),
            injection_query,
            not_scope_inherits,
            local_scope_capture: local_query.get_capture("local.scope"),
            local_definition_captures: ArcSwap::from_pointee(HashMap::new()),
            local_anchor_captures: incremental::anchor_captures(&local_query, local_query_text),
            local_query,
        })
    }

    pub(crate) fn configure(&self, f: &mut impl FnMut(&str) -> Option<Highlight>) {
        let local_definition_captures = self
            .local_query
            .captures()
            .filter_map(|(capture, name)| {
                let suffix = name.strip_prefix("local.definition.")?;
                Some((capture, f(suffix)?))
            })
            .collect();
        self.local_definition_captures
            .store(Arc::new(local_definition_captures));
    }

    fn process_match<'a, 'tree>(
        &self,
        query_match: &QueryMatch<'a, 'tree>,
        node_idx: MatchedNodeIdx,
        source: RopeSlice<'a>,
        loader: impl LanguageLoader,
    ) -> Option<InjectionQueryMatch<'tree>> {
        let properties = self.injection_properties.get(&query_match.pattern());

        let mut marker = None;
        let mut last_content_node = 0;
        let mut content_nodes = 0;
        for (i, matched_node) in query_match.matched_nodes().enumerate() {
            let capture = Some(matched_node.capture);
            if capture == self.injection_language_capture {
                let range = matched_node.node.byte_range();
                marker = Some(InjectionLanguageMarker::Match(
                    source.byte_slice(range.start as usize..range.end as usize),
                ));
            } else if capture == self.injection_filename_capture {
                let range = matched_node.node.byte_range();
                marker = Some(InjectionLanguageMarker::Filename(
                    source.byte_slice(range.start as usize..range.end as usize),
                ));
            } else if capture == self.injection_shebang_capture {
                let range = matched_node.node.byte_range();
                let node_slice = source.byte_slice(range.start as usize..range.end as usize);

                // some languages allow space and newlines before the actual string content
                // so a shebang could be on either the first or second line
                let lines = if let Ok(end) = node_slice.try_line_to_byte(2) {
                    node_slice.byte_slice(..end)
                } else {
                    node_slice
                };

                marker = SHEBANG_REGEX
                    .captures_iter(regex_cursor::Input::new(lines))
                    .map(|cap| {
                        let cap = lines.byte_slice(cap.get_group(1).unwrap().range());
                        InjectionLanguageMarker::Shebang(cap)
                    })
                    .next()
            } else if capture == self.injection_content_capture {
                content_nodes += 1;

                last_content_node = i as u32;
            }
        }
        let marker = marker.or(properties
            .and_then(|p| p.language.as_deref())
            .map(InjectionLanguageMarker::Name))?;

        let language = loader.language_for_marker(marker)?;
        let scope = if properties.is_some_and(|p| p.combined) {
            Some(InjectionScope::Pattern {
                pattern: query_match.pattern(),
                language,
            })
        } else if content_nodes != 1 {
            Some(InjectionScope::Match {
                id: query_match.id(),
            })
        } else {
            None
        };

        Some(InjectionQueryMatch {
            language,
            scope,
            include_children: properties.map(|p| p.include_children).unwrap_or_default(),
            node: query_match.matched_node(node_idx).node.clone(),
            last_match: last_content_node == node_idx,
            pattern: query_match.pattern(),
        })
    }

    /// The matches of the query on the tree of `root`, limited to `range` (see
    /// `crate::incremental` for which matches that finds), in the order of `InjectionMatch::order`.
    /// `parent_ranges` are the ranges of the layer of the tree, `next_id` numbers the matches
    /// injecting several nodes into one layer.
    fn execute(
        &self,
        root: &Node,
        within: Within,
        source: RopeSlice,
        loader: &impl LanguageLoader,
        parent_ranges: &[tree_sitter::Range],
        next_id: &mut u32,
    ) -> Vec<InjectionMatch> {
        let Some(mut cursor) = within.cursor(&self.injection_query, root, source) else {
            return Vec::new();
        };
        let injection_content_capture = self.injection_content_capture.unwrap();
        let mut found = Vec::new();
        // Matches, not captures: a query limited to a range skips the captures outside it, but a
        // match anchored in the range is all there, wherever its nodes are.
        while let Some(query_match) = cursor.next_match() {
            *next_id = next_id.wrapping_add(1);
            let anchor = incremental::anchor_source(
                query_match.matched_nodes(),
                &self.anchor_captures[query_match.pattern().idx()],
            );
            for node_idx in 0..query_match.matched_nodes().count() as MatchedNodeIdx {
                if query_match.matched_node(node_idx).capture != injection_content_capture {
                    continue;
                }
                let Some(mut mat) = self.process_match(&query_match, node_idx, source, loader)
                else {
                    break;
                };
                if mat.node.byte_range().is_empty() {
                    continue;
                }
                if let Some(InjectionScope::Match { id }) = &mut mat.scope {
                    *id = *next_id;
                }
                found.push((mat, anchor.clone()));
            }
        }
        let sources: Vec<_> = found.iter().map(|(_, source)| source.clone()).collect();
        let anchors = incremental::anchors(root, &sources);
        let mut matches: Vec<_> = found
            .into_iter()
            .zip(anchors)
            .map(|((mat, _), anchor)| InjectionMatch {
                anchor,
                node: mat.node.byte_range(),
                ranges: intersect_ranges(mat.include_children, mat.node, parent_ranges).collect(),
                language: mat.language,
                scope: mat.scope,
                include_children: mat.include_children,
                pattern: mat.pattern,
                last_match: mat.last_match,
            })
            .collect();
        matches.sort_by_key(InjectionMatch::order);
        matches
    }
}

/// Where a query looks for matches (PATCH (helix fork)).
pub(crate) enum Within<'a> {
    /// The matches anchored in a range of the tree (all of it: `0..u32::MAX`).
    Range(Range),
    /// The matches anchored at the children of a node (see `incremental::Shell`), and what
    /// else starts no deeper than their children.
    Shell(&'a incremental::Shell),
}

impl Within<'_> {
    /// A cursor of `query` over the tree of `root` that finds those matches. `None` when the
    /// shell is not in the tree.
    pub(crate) fn cursor<'a, 'tree>(
        &self,
        query: &'a Query,
        root: &Node<'tree>,
        source: RopeSlice<'a>,
    ) -> Option<tree_sitter::QueryCursor<'a, 'tree, tree_sitter::RopeInput<'a>>> {
        let cursor = match self {
            Within::Range(range) => {
                InactiveQueryCursor::new(range.clone(), TREE_SITTER_MATCH_LIMIT)
                    .execute_query(query, root, source)
            }
            Within::Shell(shell) => {
                let node = root
                    .descendant_for_byte_range(shell.range.start, shell.range.end)
                    .filter(|node| node.byte_range() == shell.range)?;
                let mut cursor = InactiveQueryCursor::new(0..u32::MAX, TREE_SITTER_MATCH_LIMIT);
                // the node's children, and theirs (a pattern of siblings anchored at a child
                // starts at the child's children)
                cursor.set_max_start_depth(2);
                cursor.execute_query(query, &node, source)
            }
        };
        Some(cursor)
    }
}

/// The matches to inject, in order, from all of a layer's matches (in the order of
/// `InjectionMatch::order`): of matches of one node the last one, and of nested matches including
/// their children the one of the last pattern first.
///
/// The ranges can be nested: `| range 2 |` in `|   range 1  |` is possible and always comes after
/// it. This case is handled by the caller.
fn resolve(matches: &[InjectionMatch]) -> Vec<&InjectionMatch> {
    let mut resolved = Vec::with_capacity(matches.len());
    let mut buf: Vec<&InjectionMatch> = Vec::new();
    let mut iter = matches.iter().peekable();
    loop {
        if let Some(mat) = buf.pop() {
            resolved.push(mat);
            continue;
        }
        let Some(mut res) = iter.next() else {
            return resolved;
        };
        // if children are not included then nested injections don't
        // interfere with each other unless exactly identical. Since
        // this is the default setting we have a fastpath for it
        if res.include_children == IncludedChildren::None {
            let mut fast_return = true;
            while let Some(overlap) = iter.next_if(|mat| mat.node == res.node) {
                if overlap.include_children != IncludedChildren::None {
                    buf.push(overlap);
                    fast_return = false;
                    break;
                }
                // Prefer the last capture which matches this exact node.
                res = overlap;
            }
            if fast_return {
                resolved.push(res);
                continue;
            }
        }

        // we if can't use the fastpath we accumulate all overlapping matches
        // and then sort them according to precedence rules...
        while let Some(overlap) = iter.next_if(|mat| mat.node.end <= res.node.end) {
            buf.push(overlap)
        }
        if buf.is_empty() {
            resolved.push(res);
            continue;
        }
        buf.push(res);
        buf.sort_unstable_by_key(|mat| (mat.pattern, Reverse(mat.node.start)));
        resolved.extend(buf.pop());
    }
}

impl Syntax {
    pub(crate) fn run_injection_query(
        &mut self,
        layer: Layer,
        edits: &[tree_sitter::InputEdit],
        source: RopeSlice<'_>,
        loader: &impl LanguageLoader,
        full: bool,
        mut parse_layer: impl FnMut(Layer),
    ) {
        self.map_injections(layer, None, edits);
        let layer_data = &mut self.layer_mut(layer);
        let Some(LanguageConfig {
            injection_query: ref injections_query,
            ..
        }) = loader.get_config(layer_data.language)
        else {
            return;
        };
        if injections_query.injection_content_capture.is_none() {
            return;
        }

        // work around borrow checker
        let parent_ranges = take(&mut layer_data.ranges);
        let parse_tree = layer_data.parse_tree.take().unwrap();
        let mut injections: Vec<Injection> = Vec::with_capacity(layer_data.injections.len());
        let old_injections = take(&mut layer_data.injections);
        let old_list: Vec<Injection> = old_injections.clone();

        // PATCH (helix fork): the matches of the last update, moved by the edits. A layer not
        // changed by them has the same ones; a changed layer is queried where the edits changed
        // it (see `crate::incremental`), or all of it.
        let mut matches = take(&mut layer_data.injection_matches);
        let shift = Shift::new(edits);
        for mat in &mut matches {
            mat.shift(&shift);
        }
        // A layer never queried yet (just injected) is queried whole.
        if layer_data.flags.modified || !layer_data.queried {
            layer_data.queried = true;
            let root = parse_tree.root_node();
            let mut next_id = layer_data.next_match_id;
            let changed = layer_data
                .changed_ranges
                .clone()
                .filter(|_| !full && !layer_data.flags.ranges_changed);
            let shell = layer_data.shell.clone();
            // (a cell: the queries of the pieces and of the shell both count the matches)
            let next_id_cell = std::cell::Cell::new(next_id);
            let query = |within: Within| {
                let mut id = next_id_cell.get();
                let found = injections_query.execute(
                    &root,
                    within,
                    source,
                    loader,
                    &parent_ranges,
                    &mut id,
                );
                next_id_cell.set(id);
                found
            };
            let updated = changed.and_then(|changed| {
                let pieces = incremental::changed_pieces(&changed, edits);
                let mut updated = incremental::update_matches(
                    take(&mut matches),
                    &pieces,
                    &root.byte_range(),
                    shell.as_ref(),
                    |mat: &InjectionMatch| mat.anchor.as_ref(),
                    |range| query(Within::Range(range)),
                    |shell| query(Within::Shell(shell)),
                )?;
                updated.sort_by_key(InjectionMatch::order);
                Some(updated)
            });
            matches = updated.unwrap_or_else(|| query(Within::Range(0..u32::MAX)));
            next_id = next_id_cell.get();
            self.layer_mut(layer).next_match_id = next_id;
        }

        let mut old_injections = old_injections.into_iter().peekable();

        let mut combined_injections: HashMap<InjectionScope, Layer> = HashMap::with_capacity(32);
        for mat in resolve(&matches) {
            let matched_node_range = mat.node.clone();
            let mut insert_position = injections.len();
            // if a parent node already has an injection ignore this injection
            // in theory the first condition would be enough to detect that
            // however in case the parent node does not include children it
            // is possible that one of these children is another separate
            // injection. In these cases we cannot skip the injection
            //
            // also the precedence sorting (and rare intersection) means that
            // overlapping injections may be sorted not by position but by
            // precedence (highest precedence first). the code here ensures
            // that injections get sorted to the correct position
            if let Some(last_injection) = injections
                .last()
                .filter(|injection| ranges_intersect(&injection.range, &matched_node_range))
            {
                // this condition is not needed but serves as fast path
                // for common cases
                if last_injection.range.start <= matched_node_range.start {
                    continue;
                } else {
                    insert_position = injections.partition_point(|injection| {
                        injection.range.end <= matched_node_range.start
                    });
                    if injections[insert_position].range.start < matched_node_range.end {
                        continue;
                    }
                }
            }

            let language = mat.language;
            let reused_injection =
                self.reuse_injection(language, matched_node_range.clone(), &mut old_injections);
            let layer = match &mat.scope {
                Some(scope @ InjectionScope::Match { .. }) if mat.last_match => {
                    combined_injections.remove(scope).unwrap_or_else(|| {
                        self.init_injection(layer, mat.language, reused_injection.clone())
                    })
                }
                Some(scope) => *combined_injections.entry(scope.clone()).or_insert_with(|| {
                    self.init_injection(layer, mat.language, reused_injection.clone())
                }),
                None => self.init_injection(layer, mat.language, reused_injection.clone()),
            };
            let mut layer_data = self.layer_mut(layer);
            if !layer_data.flags.touched {
                layer_data.flags.touched = true;
                parse_layer(layer)
            }
            if layer_data.flags.reused {
                layer_data.flags.modified |= reused_injection.as_ref().is_none_or(|injection| {
                    injection.matched_node_range != matched_node_range || injection.layer != layer
                });
            } else if let Some(reused_injection) = reused_injection {
                layer_data.flags.reused = true;
                layer_data.flags.modified = true;
                // The tree of another layer: its matches are not this layer's (PATCH (helix fork)).
                layer_data.flags.ranges_changed = true;
                let reused_parse_tree = self.layer(reused_injection.layer).tree().cloned();
                layer_data = self.layer_mut(layer);
                layer_data.parse_tree = reused_parse_tree;
            }

            let old_len = injections.len();
            for range in mat.ranges.iter().cloned() {
                layer_data.ranges.push(tree_sitter::Range::new(
                    tree_sitter::Point::ZERO,
                    tree_sitter::Point::ZERO,
                    range.start,
                    range.end,
                ));
                injections.push(Injection {
                    range,
                    layer,
                    matched_node_range: matched_node_range.clone(),
                });
            }
            if old_len != insert_position {
                let inserted = injections.len() - old_len;
                injections[insert_position..].rotate_right(inserted);
                layer_data.ranges[insert_position..].rotate_right(inserted);
            }
        }

        // Any remaining injections which were not reused should have their layers marked as
        // modified. These layers might have a new set of ranges (if they were visited) and so
        // their trees need to be re-parsed.
        for old_injection in old_injections {
            self.layer_mut(old_injection.layer).flags.modified = true;
        }

        // A layer whose ranges changed (more than the edits moved them) is queried whole: one with
        // a range without the same range of the same layer before (both lists are sorted). A
        // layer is parsed again only when its ranges or the text in them changed: a layer of many
        // ranges (comments injected together) moved by an edit before some of them is the same.
        let (mut old, mut new) = (old_list.iter().peekable(), injections.iter().peekable());
        loop {
            let changed = match (old.peek(), new.peek()) {
                (None, None) => break,
                (Some(o), Some(n)) if o.range == n.range => {
                    let changed = (o.layer != n.layer).then_some([o.layer, n.layer]);
                    old.next();
                    new.next();
                    changed
                }
                (Some(o), Some(n))
                    if (o.range.start, o.range.end) < (n.range.start, n.range.end) =>
                {
                    old.next().map(|o| [o.layer, o.layer])
                }
                (Some(o), None) => Some([o.layer, old.next().unwrap().layer]),
                (_, Some(n)) => Some([n.layer, new.next().unwrap().layer]),
            };
            for layer in changed.into_iter().flatten() {
                if let Some(layer) = self.layers.get_mut(layer.idx()) {
                    layer.flags.ranges_changed = true;
                }
            }
        }
        for injection in &injections {
            let flags = &mut self.layer_mut(injection.layer).flags;
            flags.modified = flags.ranges_changed || flags.edited;
        }

        let layer_data = &mut self.layer_mut(layer);
        layer_data.ranges = parent_ranges;
        layer_data.parse_tree = Some(parse_tree);
        layer_data.injections = injections;
        layer_data.injection_matches = matches;
    }

    /// Maps the layers injection ranges through edits to enable incremental re-parsing.
    fn map_injections(
        &mut self,
        layer: Layer,
        // TODO: drop this parameter?
        offset: Option<i32>,
        mut edits: &[tree_sitter::InputEdit],
    ) {
        if edits.is_empty() && offset.unwrap_or(0) == 0 {
            return;
        }
        let layer_data = self.layer_mut(layer);
        let first_relevant_injection = layer_data
            .injections
            .partition_point(|injection| injection.range.end < edits[0].start_byte);
        if first_relevant_injection == layer_data.injections.len() {
            return;
        }
        let mut offset = if let Some(offset) = offset {
            let first_relevant_edit = edits.partition_point(|edit| {
                (edit.old_end_byte as i32) < (layer_data.ranges[0].end_byte as i32 - offset)
            });
            edits = &edits[first_relevant_edit..];
            offset
        } else {
            0
        };
        // injections and edits are non-overlapping and sorted so we can
        // apply edits in O(M+N) instead of O(NM)
        let mut edits = edits.iter().peekable();
        let mut injections = take(&mut layer_data.injections);
        for injection in &mut injections[first_relevant_injection..] {
            let injection_range = &mut injection.range;
            let matched_node_range = &mut injection.matched_node_range;
            let flags = &mut self.layer_mut(injection.layer).flags;

            debug_assert!(matched_node_range.start <= injection_range.start);
            debug_assert!(matched_node_range.end >= injection_range.end);

            while let Some(edit) =
                edits.next_if(|edit| edit.old_end_byte < matched_node_range.start)
            {
                offset += edit.offset();
            }
            let mut mapped_node_range_start = (matched_node_range.start as i32 + offset) as u32;
            if let Some(edit) = edits
                .peek()
                .filter(|edit| edit.start_byte <= matched_node_range.start)
            {
                mapped_node_range_start = (edit.new_end_byte as i32 + offset) as u32;
            }
            while let Some(edit) = edits.next_if(|edit| edit.old_end_byte < injection_range.start) {
                offset += edit.offset();
            }
            flags.moved = offset != 0;
            let mut mapped_start = (injection_range.start as i32 + offset) as u32;
            if let Some(edit) = edits.next_if(|edit| edit.old_end_byte <= injection_range.end) {
                if edit.start_byte < injection_range.start {
                    flags.moved = true;
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                } else {
                    flags.modified = true;
                    flags.edited = true;
                }
                offset += edit.offset();
                while let Some(edit) =
                    edits.next_if(|edit| edit.old_end_byte <= injection_range.end)
                {
                    offset += edit.offset();
                }
            }
            let mut mapped_end = (injection_range.end as i32 + offset) as u32;
            if let Some(edit) = edits
                .peek()
                .filter(|edit| edit.start_byte <= injection_range.end)
            {
                flags.modified = true;
                flags.edited = true;

                if edit.start_byte < injection_range.start {
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                    mapped_end = mapped_start;
                }
            }
            let mut mapped_node_range_end = (matched_node_range.end as i32 + offset) as u32;
            if let Some(edit) = edits
                .peek()
                .filter(|edit| edit.start_byte <= matched_node_range.end)
            {
                if edit.start_byte < matched_node_range.start {
                    mapped_node_range_start = (edit.new_end_byte as i32 + offset) as u32;
                    mapped_node_range_end = mapped_node_range_start;
                }
            }
            *injection_range = mapped_start..mapped_end;
            *matched_node_range = mapped_node_range_start..mapped_node_range_end;
        }
        self.layer_mut(layer).injections = injections;
    }

    fn init_injection(
        &mut self,
        parent: Layer,
        language: Language,
        reuse: Option<Injection>,
    ) -> Layer {
        match reuse {
            Some(old_injection) => {
                let layer_data = self.layer_mut(old_injection.layer);
                debug_assert_eq!(layer_data.parent, Some(parent));
                layer_data.flags.reused = true;
                layer_data.ranges.clear();
                old_injection.layer
            }
            None => {
                let layer = self.layers.insert(LayerData {
                    language,
                    parse_tree: None,
                    ranges: Vec::new(),
                    injections: Vec::new(),
                    flags: LayerUpdateFlags::default(),
                    parent: Some(parent),
                    locals: Locals::default(),
                    injection_matches: Vec::new(),
                    next_match_id: 0,
                    changed_ranges: None,
                    shell: None,
                    queried: false,
                    local_captures: Vec::new(),
                    locals_queried: false,
                });
                Layer(layer as u32)
            }
        }
    }

    // TODO: only reuse if same pattern is matched
    fn reuse_injection(
        &mut self,
        language: Language,
        new_range: Range,
        injections: &mut Peekable<impl Iterator<Item = Injection>>,
    ) -> Option<Injection> {
        while let Some(skipped) =
            injections.next_if(|injection| injection.range.end <= new_range.start)
        {
            // If the layer had an injection and now does not have the injection, consider the
            // skipped layer to be modified so that its tree is re-parsed. It must be re-parsed
            // since the skipped layer now has a different set of ranges than it used to. Note
            // that the layer isn't marked as `touched` so it could be discarded if the layer
            // is not ever visited.
            self.layer_mut(skipped.layer).flags.modified = true;
        }
        injections
            .next_if(|injection| {
                injection.range.start < new_range.end
                    && self.layer(injection.layer).language == language
                    && !self.layer(injection.layer).flags.reused
            })
            .clone()
    }
}

fn intersect_ranges<'tree, 'p>(
    include_children: IncludedChildren,
    node: Node<'tree>,
    parent_ranges: &'p [tree_sitter::Range],
) -> impl Iterator<Item = Range> + use<'tree, 'p> {
    let range = node.byte_range();
    let i = parent_ranges.partition_point(|parent_range| parent_range.end_byte <= range.start);
    let parent_ranges = parent_ranges[i..]
        .iter()
        .map(|range| range.start_byte..range.end_byte);
    let excluded_ranges = node
        .children()
        .filter(move |node| match include_children {
            IncludedChildren::None => true,
            IncludedChildren::All => false,
            IncludedChildren::Unnamed => node.is_named(),
        })
        .map(|n| n.byte_range());

    intersect_ranges_impl(range, excluded_ranges, parent_ranges)
}

/// Creates an iterator over the subtraction of one set of ranges from another.
///
/// Given two iterators that yield a sorted series of ranges, this function
/// returns a new iterator that yields a series of ranges, equivalent to the
/// ranges from the first iterator, but excluding any ranges returned by the
/// second.
///
/// To reiterate, both iterators are assumed to return the ranges in sorted order.
fn exclude_ranges(
    mut ranges: impl Iterator<Item = Range>,
    excluded_ranges: impl Iterator<Item = Range>,
) -> impl Iterator<Item = Range> {
    let mut excluded_ranges = excluded_ranges.filter(|range| !range.is_empty());
    let mut next_range = ranges.next();
    let mut current_excluded_range = excluded_ranges.next();
    from_fn(move || {
        loop {
            let range = next_range.take()?;

            // Consume any ranges that precede the rest of the input ranges
            while let Some(excluded_range) = &current_excluded_range {
                if excluded_range.end <= range.start {
                    current_excluded_range = excluded_ranges.next()
                } else {
                    break;
                }
            }
            // Handle the next exclusion, if applicable
            if let Some(excluded_range) = &current_excluded_range {
                if ranges_intersect(&range, excluded_range) {
                    let preceding_range = range.start..excluded_range.start;
                    let remaining_range = excluded_range.end..range.end;

                    // Handle the rest of the range on the next iteration, if
                    // applicable, or take the next one.
                    if !remaining_range.is_empty() {
                        next_range = Some(remaining_range);
                        // This would also be handled by the loop above
                        current_excluded_range = excluded_ranges.next()
                    } else {
                        next_range = ranges.next()
                    }

                    // Return any part of the range that preceded the exclusion
                    if !preceding_range.is_empty() {
                        return Some(preceding_range);
                    } else {
                        continue;
                    }
                }
            }
            // Return any ranges that precede the next excluded range
            next_range = ranges.next();
            return Some(range);
        }
    })
}

/// Return an iterator over the ranges resulting from intersection calculation.
///
/// Given a range, a set of ranges to exclude, and a set of parent ranges,
/// this function calculates the intersection of range with the parent ranges,
/// minus the excluded ranges.
fn intersect_ranges_impl(
    mut range: Range,
    excluded_ranges: impl Iterator<Item = Range>,
    parent_ranges: impl Iterator<Item = Range>,
) -> impl Iterator<Item = Range> {
    // Skip ranges that end before the start of range
    let parent_ranges = parent_ranges.skip_while(move |r| r.end <= range.start);
    // filter by the excluded ranges
    let mut parent_ranges = exclude_ranges(parent_ranges, excluded_ranges).peekable();
    let mut next_parent_range = parent_ranges.next();
    // Pre-merge any adjacent parent ranges before the first iteration so that the very first
    // intersection is computed against the full merged span rather than just the first piece.
    while let Some(ref mut current) = next_parent_range {
        match parent_ranges.peek() {
            Some(next) if next.start == current.end => {
                current.end = next.end;
                parent_ranges.next();
            }
            _ => break,
        }
    }
    from_fn(move || {
        loop {
            // If either range is exhausted, so is this iterator
            let Some(parent_range) = &next_parent_range else {
                return None;
            };
            if range.is_empty() {
                return None;
            }

            // Discard non-intersecting part of range
            if parent_range.start > range.start {
                range.start = parent_range.start;
            }

            // Consume some or all of the range
            let intersection = range.start..std::cmp::min(range.end, parent_range.end);
            range.start = intersection.end;

            // Discard any parents that fully precede the new range, merging any adjacent ones
            // into the replacement so the next intersection is computed against the full span.
            while let Some(parent_range) = &next_parent_range {
                if parent_range.end <= range.start {
                    let mut next = parent_ranges.next();
                    while let Some(ref mut current) = next {
                        match parent_ranges.peek() {
                            Some(n) if n.start == current.end => {
                                current.end = n.end;
                                parent_ranges.next();
                            }
                            _ => break,
                        }
                    }
                    next_parent_range = next;
                } else {
                    break;
                }
            }

            // Return the part of the range that was consumed
            if !intersection.is_empty() {
                return Some(intersection);
            }
        }
    })
}

fn ranges_intersect(a: &Range, b: &Range) -> bool {
    // Adapted from <https://github.com/helix-editor/helix/blob/8df58b2e1779dcf0046fb51ae1893c1eebf01e7c/helix-core/src/selection.rs#L156-L163>
    a.start == b.start || (a.end > b.start && b.end > a.start)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn exclude_ranges_no_exclusions() {
        let result: Vec<Range> =
            exclude_ranges([0..10, 20..30].into_iter(), [].into_iter()).collect();
        assert_eq!(result, vec![0..10, 20..30]);
    }

    #[test]
    fn exclude_ranges_excludes_prefix() {
        let result: Vec<Range> = exclude_ranges([0..10].into_iter(), [0..5].into_iter()).collect();
        assert_eq!(result, vec![5..10]);
    }

    #[test]
    fn exclude_ranges_excludes_suffix() {
        let result: Vec<Range> = exclude_ranges([0..10].into_iter(), [5..10].into_iter()).collect();
        assert_eq!(result, vec![0..5]);
    }

    #[test]
    fn exclude_ranges_excludes_middle() {
        let result: Vec<Range> = exclude_ranges([0..10].into_iter(), [3..7].into_iter()).collect();
        assert_eq!(result, vec![0..3, 7..10]);
    }

    #[test]
    fn exclude_ranges_full_exclusion() {
        let result: Vec<Range> = exclude_ranges([0..10].into_iter(), [0..10].into_iter()).collect();
        assert_eq!(result, vec![]);
    }

    #[test]
    fn exclude_ranges_multiple_exclusions() {
        let result: Vec<Range> =
            exclude_ranges([0..100].into_iter(), [10..20, 40..50, 70..80].into_iter()).collect();
        assert_eq!(result, vec![0..10, 20..40, 50..70, 80..100]);
    }

    #[test]
    fn exclude_ranges_spans_multiple_input_ranges() {
        let result: Vec<Range> =
            exclude_ranges([0..20, 30..50].into_iter(), [15..35].into_iter()).collect();
        assert_eq!(result, vec![0..15, 35..50]);
    }

    #[test]
    fn intersect_ranges_impl_basic() {
        let result: Vec<Range> =
            intersect_ranges_impl(10..50, [].into_iter(), [0..100].into_iter()).collect();
        assert_eq!(result, vec![10..50]);
    }

    #[test]
    fn intersect_ranges_impl_with_excluded_child() {
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [30..70].into_iter(), [0..100].into_iter()).collect();
        assert_eq!(result, vec![0..30, 70..100]);
    }

    #[test]
    fn intersect_ranges_impl_non_adjacent_parent_ranges() {
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [].into_iter(), [10..40, 60..90].into_iter()).collect();
        assert_eq!(result, vec![10..40, 60..90]);
    }

    #[test]
    fn intersect_ranges_impl_adjacent_parent_ranges() {
        // Two adjacent parent ranges must be merged into one to avoid emitting adjacent
        // injection ranges. The first range's adjacency to the second must be detected before
        // computing the first intersection, not after yielding it.
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [].into_iter(), [10..50, 50..80].into_iter()).collect();
        assert_eq!(result, vec![10..80]);
    }

    #[test]
    fn intersect_ranges_impl_three_adjacent_parent_ranges() {
        // All three adjacent ranges must be collapsed into one.
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [].into_iter(), [10..30, 30..50, 50..80].into_iter())
                .collect();
        assert_eq!(result, vec![10..80]);
    }

    #[test]
    fn intersect_ranges_impl_adjacent_then_gap() {
        // First two adjacent, third separated: merge only the adjacent pair.
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [].into_iter(), [10..30, 30..50, 60..80].into_iter())
                .collect();
        assert_eq!(result, vec![10..50, 60..80]);
    }

    #[test]
    fn intersect_ranges_impl_gap_then_adjacent() {
        // Gap then adjacent: the post-discard merge in the existing code already handles this,
        // but it should also be correct after the pre-merge fix.
        let result: Vec<Range> =
            intersect_ranges_impl(0..100, [].into_iter(), [10..30, 40..60, 60..80].into_iter())
                .collect();
        assert_eq!(result, vec![10..30, 40..80]);
    }
}
