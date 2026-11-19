use std::borrow::Cow;
use std::cmp;
use std::fmt;
use std::mem::replace;
use std::num::NonZeroU32;
use std::ops::RangeBounds;
use std::slice;
use std::sync::Arc;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::locals::ScopeCursor;
use crate::query_iter::{MatchedNode, QueryIter, QueryIterEvent, QueryLoader};
use crate::{Injection, Language, Layer, Syntax};
use arc_swap::ArcSwap;
use hashbrown::{HashMap, HashSet};
use ropey::RopeSlice;
use tree_sitter::{
    query::{self, InvalidPredicateError, Query, UserPredicate},
    Capture, Grammar,
};
use tree_sitter::{Pattern, QueryMatch};

/// Contains the data needed to highlight code written in a particular language.
///
/// This struct is immutable and can be shared between threads.
#[derive(Debug)]
pub struct HighlightQuery {
    pub query: Query,
    highlight_indices: ArcSwap<Vec<Option<Highlight>>>,
    #[allow(dead_code)]
    /// Patterns that do not match when the node is a local.
    non_local_patterns: HashSet<Pattern>,
    local_reference_capture: Option<Capture>,
    /// The first pattern index that belongs to the locals query (i.e. the number of patterns
    /// from the highlights query). Any pattern with index >= this value is from `locals.scm`.
    first_locals_pattern: Pattern,
}

impl HighlightQuery {
    pub(crate) fn new(
        grammar: Grammar,
        highlight_query_text: &str,
        local_query_text: &str,
    ) -> Result<Self, query::ParseError> {
        // Concatenate the highlights and locals queries.
        let mut query_source =
            String::with_capacity(highlight_query_text.len() + local_query_text.len());
        query_source.push_str(highlight_query_text);
        query_source.push_str(local_query_text);

        let mut non_local_patterns = HashSet::new();
        let mut query = Query::new(grammar, &query_source, |pattern, predicate| {
            match predicate {
                // Allow the `(#set! local.scope-inherits <bool>)` property to be parsed.
                // This information is not used by this query though, it's used in the
                // injection query instead.
                UserPredicate::SetProperty {
                    key: "local.scope-inherits",
                    ..
                } => (),
                // TODO: `(#is(-not)? local)` applies to the entire pattern. Ideally you
                // should be able to supply capture(s?) which are each checked.
                UserPredicate::IsPropertySet {
                    negate: true,
                    key: "local",
                    val: None,
                } => {
                    non_local_patterns.insert(pattern);
                }
                _ => return Err(InvalidPredicateError::unknown(predicate)),
            }
            Ok(())
        })?;

        // Patterns with a start byte inside the highlights query source belong to
        // highlights.scm; everything at or after belongs to locals.scm. Must be computed
        // before disabling captures, which can invalidate `start_byte_for_pattern`.
        let first_locals_pattern = query
            .patterns()
            .find(|&p| query.start_byte_for_pattern(p) >= highlight_query_text.len())
            .unwrap_or(Pattern::SENTINEL);

        // The highlight query only cares about local.reference captures. All scope and definition
        // captures can be disabled.
        query.disable_capture("local.scope");
        let local_definition_captures: Vec<_> = query
            .captures()
            .filter(|&(_, name)| name.starts_with("local.definition."))
            .map(|(_, name)| Box::<str>::from(name))
            .collect();
        for name in local_definition_captures {
            query.disable_capture(&name);
        }

        Ok(Self {
            highlight_indices: ArcSwap::from_pointee(vec![None; query.num_captures() as usize]),
            non_local_patterns,
            local_reference_capture: query.get_capture("local.reference"),
            first_locals_pattern,
            query,
        })
    }

    /// Configures the list of recognized highlight names.
    ///
    /// Tree-sitter syntax-highlighting queries specify highlights in the form of dot-separated
    /// highlight names like `punctuation.bracket` and `function.method.builtin`. Consumers of
    /// these queries can choose to recognize highlights with different levels of specificity.
    /// For example, the string `function.builtin` will match against `function.builtin.constructor`
    /// but will not match `function.method.builtin` and `function.method`.
    ///
    /// The closure provided to this function should therefore try to first lookup the full
    /// name. If no highlight was found for that name it should [`rsplit_once('.')`](str::rsplit_once)
    /// and retry until a highlight has been found. If none of the parent scopes are defined
    /// then `Highlight::NONE` should be returned.
    ///
    /// When highlighting, results are returned as `Highlight` values, configured by this function.
    /// The meaning of these indices is up to the user of the implementation. The highlighter
    /// treats the indices as entirely opaque.
    pub(crate) fn configure(&self, f: &mut impl FnMut(&str) -> Option<Highlight>) {
        let highlight_indices = self
            .query
            .captures()
            .map(|(_, capture_name)| f(capture_name))
            .collect();
        self.highlight_indices.store(Arc::new(highlight_indices));
    }
}

/// Indicates which highlight should be applied to a region of source code.
///
/// This type is represented as a non-max u32 - a u32 which cannot be `u32::MAX`. This is checked
/// at runtime with assertions in `Highlight::new`.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Highlight(NonZeroU32);

impl Highlight {
    pub const MAX: u32 = u32::MAX - 1;

    pub const fn new(inner: u32) -> Self {
        assert!(inner != u32::MAX);
        // SAFETY: must be non-zero because `inner` is not `u32::MAX`.
        Self(unsafe { NonZeroU32::new_unchecked(inner ^ u32::MAX) })
    }

    pub const fn get(&self) -> u32 {
        self.0.get() ^ u32::MAX
    }

    pub const fn idx(&self) -> usize {
        self.get() as usize
    }
}

impl fmt::Debug for Highlight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Highlight").field(&self.get()).finish()
    }
}

#[derive(Debug)]
struct HighlightedNode {
    end: u32,
    highlight: Highlight,
}

#[derive(Debug, Default)]
pub struct LayerData {
    parent_highlights: usize,
    dormant_highlights: Vec<HighlightedNode>,
}

pub struct Highlighter<'a, 'tree, Loader: LanguageLoader> {
    query: QueryIter<'a, 'tree, HighlightQueryLoader<&'a Loader>, ()>,
    next_query_event: Option<QueryIterEvent<'tree, ()>>,
    /// The stack of currently active highlights.
    /// The ranges of the highlights stack, so each highlight in the Vec must have a starting
    /// point `>=` the starting point of the next highlight in the Vec and and ending point `<=`
    /// the ending point of the next highlight in the Vec.
    ///
    /// For a visual:
    ///
    /// ```text
    ///     | C |
    ///   |   B   |
    /// |     A    |
    /// ```
    ///
    /// would be `vec![A, B, C]`.
    active_highlights: Vec<HighlightedNode>,
    next_highlight_end: u32,
    next_highlight_start: u32,
    active_config: Option<&'a LanguageConfig>,
    // The current layer and per-layer state could be tracked on the QueryIter itself (see
    // `QueryIter::current_layer` and `QueryIter::layer_state`) however the highlighter peeks the
    // query iter. The query iter is always one event ahead, so it will enter/exit injections
    // before we get a chance to in the highlighter. So instead we track these on the highlighter.
    // Also see `Self::advance_query_iter`.
    current_layer: Layer,
    layer_states: HashMap<Layer, LayerData>,
}

pub struct HighlightList<'a>(slice::Iter<'a, HighlightedNode>);

impl Iterator for HighlightList<'_> {
    type Item = Highlight;

    fn next(&mut self) -> Option<Highlight> {
        self.0.next().map(|node| node.highlight)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for HighlightList<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|node| node.highlight)
    }
}

impl ExactSizeIterator for HighlightList<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightEvent {
    /// Reset the active set of highlights to the given ones.
    Refresh,
    /// Add more highlights which build on the existing highlights.
    Push,
}

impl<'a, 'tree: 'a, Loader: LanguageLoader> Highlighter<'a, 'tree, Loader> {
    pub fn new(
        syntax: &'tree Syntax,
        src: RopeSlice<'a>,
        loader: &'a Loader,
        range: impl RangeBounds<u32>,
    ) -> Self {
        let mut query = QueryIter::new(syntax, src, HighlightQueryLoader(loader), range);
        let active_language = query.current_language();
        let mut res = Highlighter {
            active_config: query.loader().0.get_config(active_language),
            next_query_event: None,
            current_layer: query.current_layer(),
            layer_states: Default::default(),
            active_highlights: Vec::new(),
            next_highlight_end: u32::MAX,
            next_highlight_start: 0,
            query,
        };
        res.advance_query_iter();
        res
    }

    pub fn active_highlights(&self) -> HighlightList<'_> {
        HighlightList(self.active_highlights.iter())
    }

    pub fn next_event_offset(&self) -> u32 {
        self.next_highlight_start.min(self.next_highlight_end)
    }

    pub fn advance(&mut self) -> (HighlightEvent, HighlightList<'_>) {
        let mut refresh = false;

        let pos = self.next_event_offset();
        if self.next_highlight_end == pos {
            self.process_highlight_end(pos);
            refresh = true;
        }

        // Capture the boundary after any ended highlights are removed. Highlights
        // at indices below this are from prior advance() calls and must not be
        // replaced or removed by the current one. When refresh=true we return
        // active_highlights.iter() anyway, so this value only matters for the
        // search window in start_highlight and the Push slice when refresh=false.
        let prev_stack_size = self.active_highlights.len();

        let mut first_highlight = true;
        let mut local_ref_candidate: Option<(u32, Highlight)> = None;
        while self.next_highlight_start == pos {
            let Some(query_event) = self.advance_query_iter() else {
                break;
            };
            match query_event {
                QueryIterEvent::EnterInjection(injection) => self.enter_injection(injection.layer),
                QueryIterEvent::Match(node) => self.start_highlight(
                    node,
                    &mut first_highlight,
                    prev_stack_size,
                    &mut local_ref_candidate,
                ),
                QueryIterEvent::ExitInjection { injection, state } => {
                    // `state` is returned if the layer is finished according to the `QueryIter`.
                    // The highlighter should only consider a layer finished, though, when it also
                    // has no remaining ranges to highlight. If the injection is combined and has
                    // highlight(s) past this injection's range then we should deactivate it
                    // (saving the highlights for the layer's next injection range) rather than
                    // removing it.
                    let layer_is_finished = state.is_some()
                        && self
                            .current_layer_highlights()
                            .iter()
                            .all(|h| h.end <= injection.range.end);
                    if layer_is_finished {
                        self.layer_states.remove(&injection.layer);
                    } else {
                        self.deactivate_layer(injection);
                        refresh = true;
                    }
                    let active_language = self.query.syntax().layer(self.current_layer).language;
                    self.active_config = self.query.loader().0.get_config(active_language);
                }
            }
        }
        // Commit any local reference candidate not cancelled by a discard pattern.
        if let Some((end, highlight)) = local_ref_candidate {
            let node = HighlightedNode { end, highlight };
            let search_start = prev_stack_size.min(self.active_highlights.len());
            let insert_position = self.active_highlights[search_start..]
                .iter()
                .rposition(|h| h.end <= end)
                .map(|rel_idx| rel_idx + search_start);
            match insert_position {
                Some(idx) => match self.active_highlights[idx].end.cmp(&end) {
                    cmp::Ordering::Equal => self.active_highlights[idx] = node,
                    cmp::Ordering::Less => self.active_highlights.insert(idx, node),
                    cmp::Ordering::Greater => unreachable!(),
                },
                None => self.active_highlights.extend(Some(node)),
            }
        }

        self.next_highlight_end = self
            .active_highlights
            .last()
            .map_or(u32::MAX, |node| node.end);

        if refresh {
            (
                HighlightEvent::Refresh,
                HighlightList(self.active_highlights.iter()),
            )
        } else {
            (
                HighlightEvent::Push,
                HighlightList(self.active_highlights[prev_stack_size..].iter()),
            )
        }
    }

    fn advance_query_iter(&mut self) -> Option<QueryIterEvent<'tree, ()>> {
        // Track the current layer **before** calling `QueryIter::next`. The QueryIter moves
        // to the next event with `QueryIter::next` but we're treating that event as peeked - it
        // hasn't occurred yet - so the current layer is the one the query iter was on _before_
        // `QueryIter::next`.
        self.current_layer = self.query.current_layer();
        let event = replace(&mut self.next_query_event, self.query.next());
        self.next_highlight_start = self
            .next_query_event
            .as_ref()
            .map_or(u32::MAX, |event| event.start_byte());
        event
    }

    fn process_highlight_end(&mut self, pos: u32) {
        let i = self
            .active_highlights
            .iter()
            .rposition(|highlight| highlight.end != pos)
            .map_or(0, |i| i + 1);
        self.active_highlights.truncate(i);
    }

    fn current_layer_highlights(&self) -> &[HighlightedNode] {
        let parent_start = self
            .layer_states
            .get(&self.current_layer)
            .map(|layer| layer.parent_highlights)
            .unwrap_or_default()
            .min(self.active_highlights.len());
        &self.active_highlights[parent_start..]
    }

    fn enter_injection(&mut self, layer: Layer) {
        debug_assert_eq!(layer, self.current_layer);
        let active_language = self.query.syntax().layer(layer).language;
        self.active_config = self.query.loader().0.get_config(active_language);

        let state = self.layer_states.entry(layer).or_default();
        state.parent_highlights = self.active_highlights.len();
        self.active_highlights.append(&mut state.dormant_highlights);
    }

    fn deactivate_layer(&mut self, injection: Injection) {
        let LayerData {
            mut parent_highlights,
            ref mut dormant_highlights,
            ..
        } = self.layer_states.get_mut(&injection.layer).unwrap();
        parent_highlights = parent_highlights.min(self.active_highlights.len());
        dormant_highlights.extend(self.active_highlights.drain(parent_highlights..));
        self.process_highlight_end(injection.range.end);
    }

    fn start_highlight(
        &mut self,
        node: MatchedNode,
        first_highlight: &mut bool,
        prev_stack_size: usize,
        local_ref_candidate: &mut Option<(u32, Highlight)>,
    ) {
        let range = node.node.byte_range();
        // `<QueryIter as Iterator>::next` skips matches with empty ranges.
        debug_assert!(
            !range.is_empty(),
            "QueryIter should not emit matches with empty ranges"
        );

        let config = self
            .active_config
            .expect("must have an active config to emit matches");

        let is_local_reference =
            Some(node.capture) == config.highlight_query.local_reference_capture;

        // Captures from the locals.scm tier (pattern >= first_locals_pattern) that are not
        // @local.reference act as discards: they cancel any pending local reference candidate
        // for this node and leave the highlights.scm highlight intact.
        if node.pattern >= config.highlight_query.first_locals_pattern && !is_local_reference {
            if local_ref_candidate.is_some_and(|(end, _)| end == range.end) {
                *local_ref_candidate = None;
            }
            return;
        }

        let highlight = if is_local_reference {
            // If this capture was a `@local.reference` from the locals queries, look up the
            // text of the node in the current locals cursor and use that highlight.
            let text: Cow<str> = self
                .query
                .source()
                .byte_slice(range.start as usize..range.end as usize)
                .into();
            let Some(definition) = self
                .query
                .syntax()
                .layer(self.current_layer)
                .locals
                .lookup_reference(node.scope, &text)
                .filter(|def| range.start >= def.range.end)
            else {
                return;
            };
            let highlight = config
                .injection_query
                .local_definition_captures
                .load()
                .get(&definition.capture)
                .copied();
            // Store as a candidate rather than replacing immediately. A later discard
            // pattern in locals.scm can cancel this before it takes effect.
            if let Some(h) = highlight {
                *local_ref_candidate = Some((range.end, h));
            }
            return;
        } else {
            config.highlight_query.highlight_indices.load()[node.capture.idx()]
        };

        let highlight = highlight.map(|highlight| HighlightedNode {
            end: range.end,
            highlight,
        });

        // If multiple patterns match this exact node, prefer the last one which matched.
        // This matches the precedence of Neovim, Zed, and tree-sitter-cli.
        if !*first_highlight {
            // NOTE: `!*first_highlight` implies that the start positions are the same.
            // Only search within highlights added during this advance() call. Highlights
            // before `prev_stack_size` belong to ancestor nodes from prior advance() calls
            // and must not be replaced or removed by a descendant's captures.
            let search_start = prev_stack_size.min(self.active_highlights.len());
            let insert_position = self.active_highlights[search_start..]
                .iter()
                .rposition(|h| h.end <= range.end)
                .map(|rel_idx| rel_idx + search_start);
            if let Some(idx) = insert_position {
                match self.active_highlights[idx].end.cmp(&range.end) {
                    // If there is a prior highlight for this start..end range, replace it.
                    cmp::Ordering::Equal => {
                        if let Some(highlight) = highlight {
                            self.active_highlights[idx] = highlight;
                        } else {
                            self.active_highlights.remove(idx);
                        }
                    }
                    // Captures are emitted in the order that they are finished. Insert any
                    // highlights which start at the same position into the active highlights so
                    // that the ordering invariant remains satisfied.
                    cmp::Ordering::Less => {
                        if let Some(highlight) = highlight {
                            self.active_highlights.insert(idx, highlight)
                        }
                    }
                    // By definition of our `rposition` predicate:
                    cmp::Ordering::Greater => unreachable!(),
                }
            } else {
                self.active_highlights.extend(highlight);
            }
        } else if let Some(highlight) = highlight {
            self.active_highlights.push(highlight);
            *first_highlight = false;
        }

        // `active_highlights` must be a stack of highlight events the highlights stack on the
        // prior highlights in the Vec. Each highlight's range must be a subset of the highlight's
        // range before it.
        debug_assert!(
            // The assertion is actually true for the entire stack but combined injections
            // throw a wrench in things: the highlight can end after the current injection.
            // The highlight is removed from `active_highlights` as the injection layer ends
            // so the wider assertion would be true in practice. We don't track the injection
            // end right here though so we can't assert on it.
            self.current_layer_highlights().is_sorted_by_key(|h| cmp::Reverse(h.end)),
            "unsorted highlights on layer {:?}: {:?}\nall active highlights must be sorted by `end` descending",
            self.current_layer,
            self.active_highlights,
        );
    }
}

pub(crate) struct HighlightQueryLoader<T>(T);

impl<'a, T: LanguageLoader> QueryLoader<'a> for HighlightQueryLoader<&'a T> {
    fn get_query(&mut self, lang: Language) -> Option<&'a Query> {
        self.0
            .get_config(lang)
            .map(|config| &config.highlight_query.query)
    }

    fn are_predicates_satisfied(
        &self,
        lang: Language,
        mat: &QueryMatch<'_, '_>,
        source: RopeSlice<'_>,
        locals_cursor: &ScopeCursor<'_>,
    ) -> bool {
        let highlight_query = &self
            .0
            .get_config(lang)
            .expect("must have a config to emit matches")
            .highlight_query;

        // Highlight queries should reject the match when a pattern is marked with
        // `(#is-not? local)` and any capture in the pattern matches a definition in scope.
        //
        // TODO: in the future we should propose that `#is-not? local` takes one or more
        // captures as arguments. Ideally we would check that the captured node is also captured
        // by a `local.reference` capture from the locals query but that's really messy to pass
        // around that information. For now we assume that all matches in the pattern are also
        // captured as `local.reference` in the locals, which covers most cases.
        if highlight_query.local_reference_capture.is_some()
            && highlight_query.non_local_patterns.contains(&mat.pattern())
        {
            let has_local_reference = mat.matched_nodes().any(|n| {
                let range = n.node.byte_range();
                let text: Cow<str> = source
                    .byte_slice(range.start as usize..range.end as usize)
                    .into();
                locals_cursor
                    .locals
                    .lookup_reference(locals_cursor.current_scope(), &text)
                    .is_some_and(|def| range.start >= def.range.start)
            });
            if has_local_reference {
                return false;
            }
        }

        true
    }
}
