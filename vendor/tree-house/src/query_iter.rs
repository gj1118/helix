use core::slice;
use std::iter::Peekable;
use std::mem::replace;
use std::ops::RangeBounds;

use hashbrown::{HashMap, HashSet};
use ropey::RopeSlice;

use crate::{
    locals::{Scope, ScopeCursor},
    Injection, Language, Layer, Range, Syntax, TREE_SITTER_MATCH_LIMIT,
};
use tree_sitter::{
    Capture, InactiveQueryCursor, Node, Pattern, Query, QueryCursor, QueryMatch, RopeInput,
};

/// A single capture produced by the capture-based iterator.
#[derive(Debug, Clone)]
pub struct MatchedNode<'tree> {
    pub match_id: u32,
    pub pattern: Pattern,
    pub node: Node<'tree>,
    pub capture: Capture,
    pub scope: Scope,
}

/// A complete match produced by the match-based iterator.
/// Contains all captures for one tree-sitter pattern match.
#[derive(Debug, Clone)]
pub struct CapturedMatch<'tree> {
    pub pattern: Pattern,
    pub nodes: Box<[tree_sitter::MatchedNode<'tree>]>,
}

impl<'tree> CapturedMatch<'tree> {
    pub fn nodes_for_capture(&self, capture: Capture) -> impl Iterator<Item = &Node<'tree>> {
        self.nodes
            .iter()
            .filter(move |n| n.capture == capture)
            .map(|n| &n.node)
    }
}

/// Abstracts over capture-based vs match-based cursor advancement.
pub(crate) trait IterStrategy<'a, 'tree>: Sized {
    type Peeked: 'tree;

    /// Advance the cursor to produce the next peeked item.
    /// Returns `None` and leaves `cursor` as `None` when exhausted.
    fn next_item<Loader: QueryLoader<'a>>(
        cursor: &mut Option<QueryCursor<'a, 'tree, RopeInput<'a>>>,
        source: RopeSlice<'_>,
        scope_cursor: &mut ScopeCursor<'tree>,
        language: Language,
        loader: &Loader,
    ) -> Option<Self::Peeked>;

    fn start_byte(peeked: &Self::Peeked) -> u32;
    fn end_byte(peeked: &Self::Peeked) -> u32;
    fn is_empty_range(peeked: &Self::Peeked) -> bool {
        Self::start_byte(peeked) == Self::end_byte(peeked)
    }
}

pub(crate) struct CaptureStrategy;

impl<'a, 'tree> IterStrategy<'a, 'tree> for CaptureStrategy {
    type Peeked = MatchedNode<'tree>;

    fn next_item<Loader: QueryLoader<'a>>(
        cursor: &mut Option<QueryCursor<'a, 'tree, RopeInput<'a>>>,
        source: RopeSlice<'_>,
        scope_cursor: &mut ScopeCursor<'tree>,
        language: Language,
        loader: &Loader,
    ) -> Option<Self::Peeked> {
        loop {
            let mut cur = cursor.take()?;
            let (query_match, node_idx) = cur.next_matched_node()?;
            let node = query_match.matched_node(node_idx);
            let match_id = query_match.id();
            let pattern = query_match.pattern();
            let range = node.node.byte_range();
            let scope = scope_cursor.advance(range.start);

            if !loader.are_predicates_satisfied(language, &query_match, source, scope_cursor) {
                query_match.remove();
                *cursor = Some(cur);
                continue;
            }

            let result = MatchedNode {
                match_id,
                pattern,
                node: node.node.clone(),
                capture: node.capture,
                scope,
            };
            *cursor = Some(cur);
            return Some(result);
        }
    }

    fn start_byte(peeked: &Self::Peeked) -> u32 {
        peeked.node.start_byte()
    }

    fn end_byte(peeked: &Self::Peeked) -> u32 {
        peeked.node.end_byte()
    }

    fn is_empty_range(peeked: &Self::Peeked) -> bool {
        peeked.node.byte_range().is_empty()
    }
}

pub(crate) struct MatchStrategy;

impl<'a, 'tree> IterStrategy<'a, 'tree> for MatchStrategy {
    type Peeked = CapturedMatch<'tree>;

    fn next_item<Loader: QueryLoader<'a>>(
        cursor: &mut Option<QueryCursor<'a, 'tree, RopeInput<'a>>>,
        source: RopeSlice<'_>,
        scope_cursor: &mut ScopeCursor<'tree>,
        language: Language,
        loader: &Loader,
    ) -> Option<Self::Peeked> {
        loop {
            let mut cur = cursor.take()?;
            let query_match = cur.next_match()?;

            let start = query_match
                .matched_nodes()
                .map(|n| n.node.start_byte())
                .min()
                .unwrap_or(0);
            scope_cursor.advance(start);

            if !loader.are_predicates_satisfied(language, &query_match, source, scope_cursor) {
                query_match.remove();
                *cursor = Some(cur);
                continue;
            }

            let pattern = query_match.pattern();
            let nodes: Box<[_]> = query_match.matched_nodes().cloned().collect();
            *cursor = Some(cur);
            return Some(CapturedMatch { pattern, nodes });
        }
    }

    fn start_byte(peeked: &Self::Peeked) -> u32 {
        peeked
            .nodes
            .iter()
            .map(|n| n.node.start_byte())
            .min()
            .unwrap_or(0)
    }

    fn end_byte(peeked: &Self::Peeked) -> u32 {
        peeked
            .nodes
            .iter()
            .map(|n| n.node.end_byte())
            .max()
            .unwrap_or(0)
    }
}

struct LayerIter<'a, 'tree, S: IterStrategy<'a, 'tree>> {
    cursor: Option<QueryCursor<'a, 'tree, RopeInput<'a>>>,
    peeked: Option<S::Peeked>,
    language: Language,
    scope_cursor: ScopeCursor<'tree>,
}

impl<'a, 'tree, S: IterStrategy<'a, 'tree>> LayerIter<'a, 'tree, S> {
    fn peek<Loader: QueryLoader<'a>>(
        &mut self,
        source: RopeSlice<'_>,
        loader: &Loader,
    ) -> Option<&S::Peeked> {
        if self.peeked.is_none() {
            self.peeked = S::next_item(
                &mut self.cursor,
                source,
                &mut self.scope_cursor,
                self.language,
                loader,
            );
        }
        self.peeked.as_ref()
    }

    fn consume(&mut self) -> S::Peeked {
        self.peeked.take().unwrap()
    }

    fn has_peeked(&self) -> bool {
        self.peeked.is_some()
    }
}

struct ActiveLayer<'a, 'tree, S: IterStrategy<'a, 'tree>, LayerState> {
    state: LayerState,
    layer_iter: LayerIter<'a, 'tree, S>,
    injections: Peekable<slice::Iter<'a, Injection>>,
}

struct QueryIterLayerManager<'a, 'tree, Loader, S: IterStrategy<'a, 'tree>, LayerState> {
    range: Range,
    loader: Loader,
    src: RopeSlice<'a>,
    syntax: &'tree Syntax,
    active_layers: HashMap<Layer, Box<ActiveLayer<'a, 'tree, S, LayerState>>>,
    active_injections: Vec<Injection>,
    /// Layers which are known to have no more captures.
    finished_layers: HashSet<Layer>,
}

impl<'a, 'tree: 'a, Loader, S, LayerState> QueryIterLayerManager<'a, 'tree, Loader, S, LayerState>
where
    Loader: QueryLoader<'a>,
    S: IterStrategy<'a, 'tree>,
    LayerState: Default,
{
    fn init_layer(&mut self, injection: Injection) -> Box<ActiveLayer<'a, 'tree, S, LayerState>> {
        self.active_layers
            .remove(&injection.layer)
            .unwrap_or_else(|| {
                let layer = self.syntax.layer(injection.layer);
                let start_point = injection.range.start.max(self.range.start);
                let injection_start = layer
                    .injections
                    .partition_point(|child| child.range.end < start_point);
                let cursor = if self.finished_layers.contains(&injection.layer) {
                    None
                } else {
                    self.loader
                        .get_query(layer.language)
                        .and_then(|query| Some((query, layer.tree()?.root_node())))
                        .map(|(query, node)| {
                            InactiveQueryCursor::new(self.range.clone(), TREE_SITTER_MATCH_LIMIT)
                                .execute_query(query, &node, RopeInput::new(self.src))
                        })
                };
                Box::new(ActiveLayer {
                    state: LayerState::default(),
                    layer_iter: LayerIter {
                        language: layer.language,
                        cursor,
                        peeked: None,
                        scope_cursor: layer.locals.scope_cursor(self.range.start),
                    },
                    injections: layer.injections[injection_start..].iter().peekable(),
                })
            })
    }
}

pub(crate) struct BaseIter<
    'a,
    'tree,
    Loader: QueryLoader<'a>,
    S: IterStrategy<'a, 'tree>,
    LayerState = (),
> {
    layer_manager: Box<QueryIterLayerManager<'a, 'tree, Loader, S, LayerState>>,
    current_layer: Box<ActiveLayer<'a, 'tree, S, LayerState>>,
    current_injection: Injection,
}

impl<'a, 'tree: 'a, Loader, S, LayerState> BaseIter<'a, 'tree, Loader, S, LayerState>
where
    Loader: QueryLoader<'a>,
    S: IterStrategy<'a, 'tree>,
    LayerState: Default,
{
    pub fn new(
        syntax: &'tree Syntax,
        src: RopeSlice<'a>,
        loader: Loader,
        range: impl RangeBounds<u32>,
    ) -> Self {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&i) => i,
            std::ops::Bound::Excluded(&i) => i + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&i) => i + 1,
            std::ops::Bound::Excluded(&i) => i,
            std::ops::Bound::Unbounded => src.len_bytes() as u32,
        };
        let range = start..end;
        let node = syntax.tree().root_node();
        let injection = Injection {
            range: node.byte_range(),
            layer: syntax.root,
            matched_node_range: node.byte_range(),
        };
        let mut layer_manager = Box::new(QueryIterLayerManager {
            range,
            loader,
            src,
            syntax,
            active_layers: HashMap::with_capacity(8),
            active_injections: Vec::with_capacity(8),
            finished_layers: HashSet::with_capacity(8),
        });
        Self {
            current_layer: layer_manager.init_layer(injection.clone()),
            current_injection: injection,
            layer_manager,
        }
    }

    #[inline]
    pub fn source(&self) -> RopeSlice<'a> {
        self.layer_manager.src
    }

    #[inline]
    pub fn syntax(&self) -> &'tree Syntax {
        self.layer_manager.syntax
    }

    #[inline]
    pub fn loader(&mut self) -> &mut Loader {
        &mut self.layer_manager.loader
    }

    #[inline]
    pub fn current_layer(&self) -> Layer {
        self.current_injection.layer
    }

    #[inline]
    pub fn current_injection(&mut self) -> (Injection, &mut LayerState) {
        (
            self.current_injection.clone(),
            &mut self.current_layer.state,
        )
    }

    #[inline]
    pub fn current_language(&self) -> Language {
        self.layer_manager
            .syntax
            .layer(self.current_injection.layer)
            .language
    }

    pub fn layer_state(&mut self, layer: Layer) -> &mut LayerState {
        if layer == self.current_injection.layer {
            &mut self.current_layer.state
        } else {
            &mut self
                .layer_manager
                .active_layers
                .get_mut(&layer)
                .unwrap()
                .state
        }
    }

    fn enter_injection(&mut self, injection: Injection) {
        let active_layer = self.layer_manager.init_layer(injection.clone());
        let old_injection = replace(&mut self.current_injection, injection);
        let old_layer = replace(&mut self.current_layer, active_layer);
        self.layer_manager
            .active_layers
            .insert(old_injection.layer, old_layer);
        self.layer_manager.active_injections.push(old_injection);
    }

    fn exit_injection(&mut self) -> Option<(Injection, Option<LayerState>)> {
        let injection = replace(
            &mut self.current_injection,
            self.layer_manager.active_injections.pop()?,
        );
        let mut layer = replace(
            &mut self.current_layer,
            self.layer_manager
                .active_layers
                .remove(&self.current_injection.layer)?,
        );
        let layer_unfinished = layer.layer_iter.has_peeked() || layer.injections.peek().is_some();
        if layer_unfinished {
            self.layer_manager
                .active_layers
                .insert(injection.layer, layer);
            Some((injection, None))
        } else {
            self.layer_manager.finished_layers.insert(injection.layer);
            Some((injection, Some(layer.state)))
        }
    }
}

impl<'a, 'tree: 'a, Loader, S, LayerState> Iterator for BaseIter<'a, 'tree, Loader, S, LayerState>
where
    Loader: QueryLoader<'a>,
    S: IterStrategy<'a, 'tree>,
    LayerState: Default,
{
    type Item = BaseIterEvent<S::Peeked, LayerState>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next_injection = self
                .current_layer
                .injections
                .peek()
                .filter(|inj| inj.range.start <= self.current_injection.range.end);
            let next_item = self
                .current_layer
                .layer_iter
                .peek(self.layer_manager.src, &self.layer_manager.loader)
                .filter(|item| S::start_byte(item) <= self.current_injection.range.end);

            match (next_item, next_injection) {
                (None, None) => {
                    return self.exit_injection().map(|(injection, state)| {
                        BaseIterEvent::ExitInjection { injection, state }
                    });
                }
                (Some(item), _) if S::is_empty_range(item) => {
                    self.current_layer.layer_iter.consume();
                    continue;
                }
                (Some(_), None) => {
                    let item = self.current_layer.layer_iter.consume();
                    return Some(BaseIterEvent::Match(item));
                }
                (Some(item), Some(injection)) if S::start_byte(item) < injection.range.end => {
                    let item = self.current_layer.layer_iter.consume();
                    if S::start_byte(&item) <= injection.range.start
                        || injection.range.end < S::end_byte(&item)
                    {
                        return Some(BaseIterEvent::Match(item));
                    }
                }
                (Some(_), Some(_)) | (None, Some(_)) => {
                    let injection = self.current_layer.injections.next().unwrap();
                    self.enter_injection(injection.clone());
                    return Some(BaseIterEvent::EnterInjection(injection.clone()));
                }
            }
        }
    }
}

pub(crate) enum BaseIterEvent<Item, State = ()> {
    EnterInjection(Injection),
    Match(Item),
    ExitInjection {
        injection: Injection,
        state: Option<State>,
    },
}

pub struct QueryIter<'a, 'tree, Loader: QueryLoader<'a>, LayerState = ()>(
    BaseIter<'a, 'tree, Loader, CaptureStrategy, LayerState>,
);

impl<'a, 'tree: 'a, Loader, LayerState> QueryIter<'a, 'tree, Loader, LayerState>
where
    Loader: QueryLoader<'a>,
    LayerState: Default,
{
    pub fn new(
        syntax: &'tree Syntax,
        src: RopeSlice<'a>,
        loader: Loader,
        range: impl RangeBounds<u32>,
    ) -> Self {
        Self(BaseIter::new(syntax, src, loader, range))
    }

    #[inline]
    pub fn source(&self) -> RopeSlice<'a> {
        self.0.source()
    }

    #[inline]
    pub fn syntax(&self) -> &'tree Syntax {
        self.0.syntax()
    }

    #[inline]
    pub fn loader(&mut self) -> &mut Loader {
        self.0.loader()
    }

    #[inline]
    pub fn current_layer(&self) -> Layer {
        self.0.current_layer()
    }

    #[inline]
    pub fn current_injection(&mut self) -> (Injection, &mut LayerState) {
        self.0.current_injection()
    }

    #[inline]
    pub fn current_language(&self) -> Language {
        self.0.current_language()
    }

    pub fn layer_state(&mut self, layer: Layer) -> &mut LayerState {
        self.0.layer_state(layer)
    }
}

impl<'a, 'tree: 'a, Loader, LayerState> Iterator for QueryIter<'a, 'tree, Loader, LayerState>
where
    Loader: QueryLoader<'a>,
    LayerState: Default,
{
    type Item = QueryIterEvent<'tree, LayerState>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|event| match event {
            BaseIterEvent::EnterInjection(i) => QueryIterEvent::EnterInjection(i),
            BaseIterEvent::Match(m) => QueryIterEvent::Match(m),
            BaseIterEvent::ExitInjection { injection, state } => {
                QueryIterEvent::ExitInjection { injection, state }
            }
        })
    }
}

#[derive(Debug)]
pub enum QueryIterEvent<'tree, State = ()> {
    EnterInjection(Injection),
    Match(MatchedNode<'tree>),
    ExitInjection {
        injection: Injection,
        state: Option<State>,
    },
}

impl<S> QueryIterEvent<'_, S> {
    pub fn start_byte(&self) -> u32 {
        match self {
            QueryIterEvent::EnterInjection(injection) => injection.range.start,
            QueryIterEvent::Match(mat) => mat.node.start_byte(),
            QueryIterEvent::ExitInjection { injection, .. } => injection.range.end,
        }
    }
}

pub struct QueryMatchIter<'a, 'tree, Loader: QueryLoader<'a>, LayerState = ()>(
    BaseIter<'a, 'tree, Loader, MatchStrategy, LayerState>,
);

impl<'a, 'tree: 'a, Loader, LayerState> QueryMatchIter<'a, 'tree, Loader, LayerState>
where
    Loader: QueryLoader<'a>,
    LayerState: Default,
{
    pub fn new(
        syntax: &'tree Syntax,
        src: RopeSlice<'a>,
        loader: Loader,
        range: impl RangeBounds<u32>,
    ) -> Self {
        Self(BaseIter::new(syntax, src, loader, range))
    }

    #[inline]
    pub fn current_language(&self) -> Language {
        self.0.current_language()
    }

    #[inline]
    pub fn current_layer(&self) -> Layer {
        self.0.current_layer()
    }
}

impl<'a, 'tree: 'a, Loader, LayerState> Iterator for QueryMatchIter<'a, 'tree, Loader, LayerState>
where
    Loader: QueryLoader<'a>,
    LayerState: Default,
{
    type Item = QueryMatchIterEvent<'tree, LayerState>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|event| match event {
            BaseIterEvent::EnterInjection(i) => QueryMatchIterEvent::EnterInjection(i),
            BaseIterEvent::Match(m) => QueryMatchIterEvent::Match(m),
            BaseIterEvent::ExitInjection { injection, state } => {
                QueryMatchIterEvent::ExitInjection { injection, state }
            }
        })
    }
}

#[derive(Debug)]
pub enum QueryMatchIterEvent<'tree, State = ()> {
    EnterInjection(Injection),
    Match(CapturedMatch<'tree>),
    ExitInjection {
        injection: Injection,
        state: Option<State>,
    },
}

pub trait QueryLoader<'a> {
    fn get_query(&mut self, lang: Language) -> Option<&'a Query>;

    fn are_predicates_satisfied(
        &self,
        _lang: Language,
        _match: &QueryMatch<'_, '_>,
        _source: RopeSlice<'_>,
        _locals_cursor: &ScopeCursor<'_>,
    ) -> bool {
        true
    }
}

impl<'a, F> QueryLoader<'a> for F
where
    F: FnMut(Language) -> Option<&'a Query>,
{
    fn get_query(&mut self, lang: Language) -> Option<&'a Query> {
        (self)(lang)
    }
}
