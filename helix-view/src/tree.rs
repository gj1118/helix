use crate::{graphics::Rect, View, ViewId};
use slotmap::SlotMap;

// the dimensions are recomputed on window resize/tree change.
//
#[derive(Debug)]
pub struct Tree {
    /// The root container of the tab shown.
    root: ViewId,
    // (container, index inside the container)
    pub focus: ViewId,
    /// The tab pages: each a root container (its own split layout) and the view focused in
    /// it. The entry of the tab shown is out of date: `root` and `focus` are its own.
    tabs: Vec<(ViewId, ViewId)>,
    /// The index of the tab shown in `tabs`.
    active: usize,
    // fullscreen: bool,
    area: Rect,

    nodes: SlotMap<ViewId, Node>,

    // used for traversals
    stack: Vec<(ViewId, Rect)>,
}

#[derive(Debug)]
pub struct Node {
    parent: ViewId,
    content: Content,
}

#[derive(Debug)]
pub enum Content {
    View(Box<View>),
    Container(Box<Container>),
}

pub enum Resize {
    Shrink,
    Grow,
}

pub enum Dimension {
    Width,
    Height,
}

impl Node {
    pub fn container(layout: Layout) -> Self {
        Self {
            parent: ViewId::default(),
            content: Content::Container(Box::new(Container::new(layout))),
        }
    }

    pub fn view(view: View) -> Self {
        Self {
            parent: ViewId::default(),
            content: Content::View(Box::new(view)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Horizontal,
    Vertical,
    // could explore stacked/tabbed
}

/// Narrowest a split may be dragged down to, in columns.
const MIN_SPLIT_WIDTH: u16 = 4;
/// Shortest a split may be dragged down to, in rows (content and statusline).
const MIN_SPLIT_HEIGHT: u16 = 3;

/// The boundary between two neighbouring splits, as grabbed with the mouse.
/// Obtained from [`Tree::divider_at`] and moved with [`Tree::drag_divider`].
/// The edge of the screen a docked window is put on ([`Tree::dock`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockSide {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitDivider {
    container: ViewId,
    /// The divider sits between children `index` and `index + 1`.
    index: usize,
    /// Whether it is a vertical line between side by side splits (dragged
    /// horizontally) rather than a horizontal boundary between stacked ones.
    pub vertical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug)]
pub struct Container {
    layout: Layout,
    children: Vec<ViewId>,
    area: Rect,
    node_bounds: Vec<ContainerBounds>,
}

/// Relative size of a split within its container.
///
/// All weights share one unit: a freshly created split weighs
/// [`DEFAULT_WEIGHT`], a split in focus mode [`EXPANDED_WEIGHT`], and `Alt-w`
/// resizing adds or removes one unit at a time (bounded by the
/// `max-panel-*` settings). Dragging a divider rewrites the weights in the
/// same unit (keeping their average at [`DEFAULT_WEIGHT`]) rather than in
/// cells, so focus mode and keyboard resizing behave the same afterwards.
type Weight = f64;
const DEFAULT_WEIGHT: Weight = 10.0;
const EXPANDED_WEIGHT: Weight = 40.0;
/// Smallest weight keyboard resizing shrinks a split to.
const MIN_WEIGHT: Weight = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct ContainerBounds {
    width: Weight,
    height: Weight,
    expand: bool,
}

/// Cells for a child of `weight` when each weight unit is `slot` cells wide.
/// The small bias absorbs float error so exact sizes don't floor one short.
fn weighted_cells(slot: f64, weight: Weight) -> u16 {
    (slot * weight + 1e-6).floor() as u16
}

impl Container {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            area: Rect::default(),
            node_bounds: Vec::new(),
        }
    }

    fn get_child_by_view_id(&mut self, node: ViewId) -> Option<&mut ContainerBounds> {
        self.children
            .iter()
            .position(|child| child == &node)
            .and_then(|index| self.node_bounds.get_mut(index))
    }

    fn push_child(&mut self, node: ViewId) -> &mut Self {
        self.children.push(node);
        self.add_child_bounds();
        self
    }

    fn insert_child(&mut self, index: usize, node: ViewId) -> &mut Self {
        self.children.insert(index, node);
        self.insert_child_bounds(index);
        self
    }

    fn add_child_bounds(&mut self) -> &mut Self {
        self.node_bounds.push(ContainerBounds {
            width: DEFAULT_WEIGHT,
            height: DEFAULT_WEIGHT,
            expand: false,
        });
        self
    }

    fn insert_child_bounds(&mut self, index: usize) -> &mut Self {
        self.node_bounds.insert(
            index,
            ContainerBounds {
                width: DEFAULT_WEIGHT,
                height: DEFAULT_WEIGHT,
                expand: false,
            },
        );
        self
    }

    fn layout(&self) -> Layout {
        self.layout
    }

    fn children_count(&self) -> usize {
        self.children.len()
    }

    fn calculate_slots_width(&self) -> Weight {
        self.node_bounds
            .iter()
            .map(|bounds| match bounds.expand {
                true => EXPANDED_WEIGHT,
                false => bounds.width,
            })
            .sum()
    }

    fn calculate_slots_height(&self) -> Weight {
        self.node_bounds
            .iter()
            .map(|bounds| match bounds.expand {
                true => EXPANDED_WEIGHT,
                false => bounds.height,
            })
            .sum()
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new(Layout::Vertical)
    }
}

impl Tree {
    pub fn new(area: Rect) -> Self {
        let root = Node::container(Layout::Vertical);

        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(root);

        // root is it's own parent
        nodes[root].parent = root;

        Self {
            root,
            focus: root,
            tabs: vec![(root, root)],
            active: 0,
            // fullscreen: false,
            area,
            nodes,
            stack: Vec::new(),
        }
    }

    pub fn insert(&mut self, view: View) -> ViewId {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;
        let mut node = Node::view(view);
        node.parent = parent;
        let node = self.nodes.insert(node);
        self.get_mut(node).id = node;

        let container = match &mut self.nodes[parent] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        };

        // insert node after the current item if there is children already
        let pos = if container.children.is_empty() {
            0
        } else {
            let pos = container
                .children
                .iter()
                .position(|&child| child == focus)
                .unwrap();
            pos + 1
        };

        container.insert_child(pos, node);
        // focus the new node
        self.focus = node;

        // recalculate all the sizes
        self.recalculate();

        node
    }

    pub fn split(&mut self, view: View, layout: Layout) -> ViewId {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;

        let node = Node::view(view);
        let node = self.nodes.insert(node);
        self.get_mut(node).id = node;

        let container = match &mut self.nodes[parent] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        };
        if container.layout == layout {
            // insert node after the current item if there is children already
            let pos = if container.children.is_empty() {
                0
            } else {
                let pos = container
                    .children
                    .iter()
                    .position(|&child| child == focus)
                    .unwrap();
                pos + 1
            };
            container.insert_child(pos, node);
            self.nodes[node].parent = parent;
        } else {
            let mut split = Node::container(layout);
            split.parent = parent;
            let split = self.nodes.insert(split);

            let container = match &mut self.nodes[split] {
                Node {
                    content: Content::Container(container),
                    ..
                } => container,
                _ => unreachable!(),
            };
            container.push_child(focus);
            container.push_child(node);
            self.nodes[focus].parent = split;
            self.nodes[node].parent = split;

            let container = match &mut self.nodes[parent] {
                Node {
                    content: Content::Container(container),
                    ..
                } => container,
                _ => unreachable!(),
            };

            let pos = container
                .children
                .iter()
                .position(|&child| child == focus)
                .unwrap();

            // replace focus on parent with split
            container.children[pos] = split;
        }

        // focus the new node
        self.focus = node;

        // recalculate all the sizes
        self.recalculate();

        node
    }

    /// Get a mutable reference to a [Container] by index.
    /// # Panics
    /// Panics if `index` is not in self.nodes, or if the node's content is not a [Content::Container].
    fn container_mut(&mut self, index: ViewId) -> &mut Container {
        match &mut self.nodes[index] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        }
    }

    fn remove_or_replace(&mut self, child: ViewId, replacement: Option<ViewId>) {
        let parent = self.nodes[child].parent;

        self.nodes.remove(child);

        let container = self.container_mut(parent);
        let pos = container
            .children
            .iter()
            .position(|&item| item == child)
            .unwrap();

        if let Some(new) = replacement {
            container.children[pos] = new;
            self.nodes[new].parent = parent;
        } else {
            container.children.remove(pos);
            container.node_bounds.remove(pos);
        }
    }

    /// If the container at `container_id` has the same layout as its grandparent,
    /// dissolve the container by promoting its children into the grandparent.
    ///
    /// This eliminates unnecessary nesting that can arise from alternating split
    /// directions, ensuring correct proportional size distribution after removal.
    ///
    /// Does nothing if:
    /// - `container_id` is the root
    /// - The container has no children
    /// - The container's layout differs from its grandparent's layout
    fn flatten_container_if_same_layout(&mut self, container_id: ViewId) {
        if container_id == self.root {
            return;
        }

        let grandparent = self.nodes[container_id].parent;

        let container_layout = self.container(container_id).layout();
        let container_children_count = self.container(container_id).children_count();

        if container_children_count == 0 {
            return;
        }

        let grandparent_layout = self.container(grandparent).layout();

        if container_layout != grandparent_layout {
            return;
        }

        // Find the position of this container in grandparent's children.
        let pos = {
            let grandparent_container = self.container_mut(grandparent);
            grandparent_container
                .children
                .iter()
                .position(|&child| child == container_id)
                .unwrap()
        };

        // Move children and bounds out of the container being dissolved.
        let (promoted_children, promoted_bounds) = {
            let container = self.container_mut(container_id);
            let children: Vec<ViewId> = container.children.drain(..).collect();
            let bounds: Vec<ContainerBounds> = container.node_bounds.drain(..).collect();
            (children, bounds)
        };

        // Update parent pointers for all promoted children.
        for &child in &promoted_children {
            self.nodes[child].parent = grandparent;
        }

        // Splice promoted children into grandparent at the dissolved container's position.
        let grandparent_container = self.container_mut(grandparent);
        grandparent_container
            .children
            .splice(pos..pos + 1, promoted_children);
        grandparent_container
            .node_bounds
            .splice(pos..pos + 1, promoted_bounds);

        // Remove the now-empty container from the slotmap.
        self.nodes.remove(container_id);
    }

    /// Get an immutable reference to a [Container] by index.
    /// # Panics
    /// Panics if `index` is not in self.nodes, or if the node's content is not a [Content::Container].
    fn container(&self, index: ViewId) -> &Container {
        match &self.nodes[index] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        }
    }

    /// The root container of the tab `id` is in.
    fn root_of(&self, mut id: ViewId) -> ViewId {
        loop {
            let parent = self.nodes[id].parent;
            if parent == id {
                return id;
            }
            id = parent;
        }
    }

    /// How many tab pages there are (at least one).
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The index of the tab shown.
    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// The index of the tab `id` (a view of any tab) is in.
    pub fn tab_of(&self, id: ViewId) -> Option<usize> {
        if !self.nodes.contains_key(id) {
            return None;
        }
        let root = self.root_of(id);
        if root == self.root {
            return Some(self.active);
        }
        self.tabs.iter().position(|(tab_root, _)| *tab_root == root)
    }

    /// The root container of tab `index`: it identifies the tab while tabs are opened,
    /// closed and moved around it.
    pub fn tab_root(&self, index: usize) -> Option<ViewId> {
        if index == self.active {
            return Some(self.root);
        }
        self.tabs.get(index).map(|(root, _)| *root)
    }

    /// The view focused in tab `index` (the root container when the tab has none).
    pub fn tab_focus(&self, index: usize) -> Option<ViewId> {
        if index == self.active {
            return Some(self.focus);
        }
        self.tabs.get(index).map(|(_, focus)| *focus)
    }

    /// Opens a new, empty tab page after the one shown, and shows it: insert a view next.
    /// Puts `view` along an edge of the tab shown, `size` cells wide (left, right) or high
    /// (top, bottom), at most half the tab's width (height): the whole height (or width) of the
    /// screen, beside all the other windows. The focus does not move (but to the first window
    /// of an empty tab). The size is kept in proportion when the screen is resized, like
    /// the other splits'.
    pub fn dock(&mut self, view: View, side: DockSide, size: u16) -> ViewId {
        let layout = match side {
            DockSide::Left | DockSide::Right => Layout::Vertical,
            DockSide::Top | DockSide::Bottom => Layout::Horizontal,
        };
        let root = self.root;
        let node = self.nodes.insert(Node::view(view));
        self.get_mut(node).id = node;

        let Content::Container(container) = &mut self.nodes[root].content else {
            unreachable!("a root is a container")
        };
        if container.layout != layout {
            if container.children.is_empty() {
                container.layout = layout;
            } else {
                // The windows there go into a container of their own, beside the new one.
                let children = std::mem::take(&mut container.children);
                let bounds = std::mem::take(&mut container.node_bounds);
                let old_layout = container.layout;
                container.layout = layout;
                let mut inner = Node::container(old_layout);
                inner.parent = root;
                let inner = self.nodes.insert(inner);
                for &child in &children {
                    self.nodes[child].parent = inner;
                }
                if let Content::Container(inner_container) = &mut self.nodes[inner].content {
                    inner_container.children = children;
                    inner_container.node_bounds = bounds;
                }
                if let Content::Container(container) = &mut self.nodes[root].content {
                    container.push_child(inner);
                }
            }
        }
        let Content::Container(container) = &mut self.nodes[root].content else {
            unreachable!("a root is a container")
        };
        let index = match side {
            DockSide::Left | DockSide::Top => 0,
            DockSide::Right | DockSide::Bottom => container.children.len(),
        };
        container.insert_child(index, node);
        self.nodes[node].parent = root;
        if self.focus == root {
            self.focus = node;
        }
        self.recalculate();
        self.set_child_size(root, index, size);
        node
    }

    /// Makes child `index` of `container` `size` cells along its layout (clamped so the others
    /// keep their least size), the others sharing what is left in proportion to their sizes.
    fn set_child_size(&mut self, container: ViewId, index: usize, size: u16) {
        let Content::Container(node) = &self.nodes[container].content else {
            return;
        };
        let layout = node.layout;
        let sizes: Vec<u16> = node
            .children
            .iter()
            .map(|&child| {
                let area = self.node_area(child);
                match layout {
                    Layout::Vertical => area.width,
                    Layout::Horizontal => area.height,
                }
            })
            .collect();
        if sizes.len() < 2 || index >= sizes.len() {
            return;
        }
        let min = match layout {
            Layout::Vertical => MIN_SPLIT_WIDTH,
            Layout::Horizontal => MIN_SPLIT_HEIGHT,
        };
        let total: u32 = sizes.iter().map(|&s| s.max(1) as u32).sum();
        // the least the others take: each window in them keeps its least size
        let reserved: u32 = node
            .children
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, &child)| {
                let leaves = self.leaves_along(child, layout);
                // (and the gap column between side by side windows)
                let gaps = if layout == Layout::Vertical {
                    leaves - 1
                } else {
                    0
                };
                leaves * min as u32 + gaps
            })
            .sum();
        // at most half: the windows beside it keep their proportions among themselves
        let most = total.saturating_sub(reserved).min(total / 2);
        let size = (size as u32).clamp(min as u32, most.max(min as u32));
        let rest_before: u32 = sizes
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, &s)| s.max(1) as u32)
            .sum();
        let rest = total.saturating_sub(size);
        let unit = sizes.len() as f64 * DEFAULT_WEIGHT / total.max(1) as f64;
        // whole cells (what rounding leaves goes to the last of the others): a fraction
        // would be rounded into the first window's size
        let mut cells: Vec<u32> = sizes
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                if i == index {
                    size
                } else {
                    s.max(1) as u32 * rest / rest_before.max(1)
                }
            })
            .collect();
        let given: u32 = cells.iter().sum();
        if let Some(last) = (0..cells.len()).rev().find(|&i| i != index) {
            cells[last] += total.saturating_sub(given);
        }
        let Content::Container(node) = &mut self.nodes[container].content else {
            return;
        };
        for (i, bounds) in node.node_bounds.iter_mut().enumerate() {
            let weight = (cells[i].max(1) as f64) * unit;
            match layout {
                Layout::Vertical => bounds.width = weight,
                Layout::Horizontal => bounds.height = weight,
            }
            bounds.expand = false;
        }
        self.recalculate();
    }

    /// How many windows of `node` sit side by side along `layout` at most.
    fn leaves_along(&self, node: ViewId, layout: Layout) -> u32 {
        match &self.nodes[node].content {
            Content::View(_) => 1,
            Content::Container(container) => {
                let counts = container
                    .children
                    .iter()
                    .map(|&child| self.leaves_along(child, layout));
                if container.layout == layout {
                    counts.sum()
                } else {
                    counts.max().unwrap_or(1)
                }
            }
        }
    }

    pub fn new_tab(&mut self) {
        let mut root = Node::container(Layout::Vertical);
        root.parent = ViewId::default();
        let root = self.nodes.insert(root);
        // a root is its own parent
        self.nodes[root].parent = root;
        self.tabs[self.active] = (self.root, self.focus);
        self.active += 1;
        self.tabs.insert(self.active, (root, root));
        self.root = root;
        self.focus = root;
        self.recalculate();
    }

    /// Shows tab `index`. Nothing for the tab shown or one that doesn't exist.
    pub fn switch_tab(&mut self, index: usize) {
        if index == self.active || index >= self.tabs.len() {
            return;
        }
        self.tabs[self.active] = (self.root, self.focus);
        self.active = index;
        (self.root, self.focus) = self.tabs[index];
        self.recalculate();
    }

    /// Moves the tab shown to `index` among the tabs (clamped).
    pub fn move_tab(&mut self, index: usize) {
        let index = index.min(self.tabs.len() - 1);
        let tab = self.tabs.remove(self.active);
        self.tabs.insert(index, tab);
        self.active = index;
    }

    /// The views of every tab, the ones shown or not.
    pub fn all_views(&self) -> impl Iterator<Item = (&View, bool)> {
        let focus = self.focus;
        self.nodes.iter().filter_map(move |(key, node)| match node {
            Node {
                content: Content::View(view),
                ..
            } => Some((view.as_ref(), focus == key)),
            _ => None,
        })
    }

    /// The views of every tab, the ones shown or not.
    pub fn all_views_mut(&mut self) -> impl Iterator<Item = (&mut View, bool)> {
        let focus = self.focus;
        self.nodes
            .iter_mut()
            .filter_map(move |(key, node)| match node {
                Node {
                    content: Content::View(view),
                    ..
                } => Some((view.as_mut(), focus == key)),
                _ => None,
            })
    }

    /// The views of tab `index`, in order.
    pub fn tab_views(&self, index: usize) -> Vec<ViewId> {
        let Some(root) = (if index == self.active {
            Some(self.root)
        } else {
            self.tabs.get(index).map(|(root, _)| *root)
        }) else {
            return Vec::new();
        };
        let mut views = Vec::new();
        let mut stack = vec![root];
        while let Some(key) = stack.pop() {
            match &self.nodes[key].content {
                Content::View(_) => views.push(key),
                Content::Container(container) => stack.extend(container.children.iter().rev()),
            }
        }
        views
    }

    /// Removes view `index`, of any tab. A tab whose last view goes is closed (unless it is
    /// the only one): the tab after it is shown in its place (the one before, for the last).
    pub fn remove(&mut self, index: ViewId) {
        if !self.nodes.contains_key(index) {
            return;
        }
        let root = self.root_of(index);
        if root == self.root {
            self.remove_in_active(index);
            return;
        }
        // A view of a tab not shown: removed in it, then back to the tab shown.
        let shown = self.root;
        let Some(tab) = self.tabs.iter().position(|(tab_root, _)| *tab_root == root) else {
            return;
        };
        self.switch_tab(tab);
        self.remove_in_active(index);
        if let Some(back) = self
            .tabs
            .iter()
            .position(|(tab_root, _)| *tab_root == shown)
        {
            self.switch_tab(back);
        }
    }

    fn remove_in_active(&mut self, index: ViewId) {
        self.remove_view(index);
        if self.is_empty() && self.tabs.len() > 1 {
            // the tab's last view went: the tab goes
            self.nodes.remove(self.root);
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len() - 1);
            (self.root, self.focus) = self.tabs[self.active];
            self.recalculate();
        }
    }

    fn remove_view(&mut self, index: ViewId) {
        if self.focus == index {
            // focus on something else
            self.focus = self.prev();
        }

        let parent = self.nodes[index].parent;
        let parent_is_root = parent == self.root;

        self.remove_or_replace(index, None);

        let mut flatten_target = parent;
        let parent_container = self.container_mut(parent);
        if parent_container.children.len() == 1 && !parent_is_root {
            // Lets merge the only child back to its grandparent so that Views
            // are equally spaced.
            let sibling = parent_container.children.pop().unwrap();
            self.remove_or_replace(parent, Some(sibling));
            // The promoted sibling may now have the same layout as its new
            // parent (the grandparent). If so, it should be flattened.
            if matches!(self.nodes[sibling].content, Content::Container(_)) {
                flatten_target = sibling;
            } else {
                // View sibling promoted; nothing to flatten.
                self.recalculate();
                return;
            }
        }

        // Flatten any container whose layout matches its grandparent's layout.
        // This eliminates unnecessary nesting from alternating split directions,
        // ensuring correct proportional size distribution.
        self.flatten_container_if_same_layout(flatten_target);

        self.recalculate()
    }

    /// The views of the tab shown (see [`Self::all_views`] for every tab's).
    pub fn views(&self) -> impl Iterator<Item = (&View, bool)> {
        let focus = self.focus;
        let single_tab = self.tabs.len() == 1;
        self.nodes
            .iter()
            .filter(move |(key, _)| single_tab || self.root_of(*key) == self.root)
            .filter_map(move |(key, node)| match node {
                Node {
                    content: Content::View(view),
                    ..
                } => Some((view.as_ref(), focus == key)),
                _ => None,
            })
    }

    /// The views of the tab shown (see [`Self::all_views_mut`] for every tab's).
    pub fn views_mut(&mut self) -> impl Iterator<Item = (&mut View, bool)> {
        let focus = self.focus;
        let shown: Option<std::collections::HashSet<ViewId>> =
            (self.tabs.len() > 1).then(|| self.tab_views(self.active).into_iter().collect());
        self.nodes
            .iter_mut()
            .filter(move |(key, _)| shown.as_ref().is_none_or(|shown| shown.contains(key)))
            .filter_map(move |(key, node)| match node {
                Node {
                    content: Content::View(view),
                    ..
                } => Some((view.as_mut(), focus == key)),
                _ => None,
            })
    }

    /// Get reference to a [View] by index.
    /// # Panics
    ///
    /// Panics if `index` is not in self.nodes, or if the node's content is not [Content::View]. This can be checked with [Self::contains].
    pub fn get(&self, index: ViewId) -> &View {
        self.try_get(index).unwrap()
    }

    /// Try to get reference to a [View] by index. Returns `None` if node content is not a [`Content::View`].
    ///
    /// Does not panic if the view does not exists anymore.
    pub fn try_get(&self, index: ViewId) -> Option<&View> {
        match self.nodes.get(index) {
            Some(Node {
                content: Content::View(view),
                ..
            }) => Some(view),
            _ => None,
        }
    }

    /// Get a mutable reference to a [View] by index.
    /// # Panics
    ///
    /// Panics if `index` is not in self.nodes, or if the node's content is not [Content::View]. This can be checked with [Self::contains].
    pub fn get_mut(&mut self, index: ViewId) -> &mut View {
        match &mut self.nodes[index] {
            Node {
                content: Content::View(view),
                ..
            } => view,
            _ => unreachable!(),
        }
    }

    /// Check if tree contains a [Node] with a given index.
    pub fn contains(&self, index: ViewId) -> bool {
        self.nodes.contains_key(index)
    }

    pub fn is_empty(&self) -> bool {
        match &self.nodes[self.root] {
            Node {
                content: Content::Container(container),
                ..
            } => container.children.is_empty(),
            _ => unreachable!(),
        }
    }

    pub fn resize(&mut self, area: Rect) -> bool {
        if self.area != area {
            self.area = area;
            self.recalculate();
            return true;
        }
        false
    }

    pub fn recalculate(&mut self) {
        if self.is_empty() {
            // There are no more views, so the tree should focus itself again.
            self.focus = self.root;

            return;
        }

        self.stack.push((self.root, self.area));

        // take the area
        // fetch the node
        // a) node is view, give it whole area
        // b) node is container, calculate areas for each child and push them on the stack

        while let Some((key, area)) = self.stack.pop() {
            let node = &mut self.nodes[key];

            match &mut node.content {
                Content::View(view) => {
                    // debug!!("setting view area {:?}", area);
                    view.area = area;
                } // TODO: call f()
                Content::Container(container) => {
                    // debug!!("setting container area {:?}", area);
                    container.area = area;

                    match container.layout {
                        Layout::Horizontal => {
                            let len = container.children.len();
                            let slots = container.calculate_slots_height();
                            let slot_height = area.height as f64 / slots;
                            let mut child_y = area.y;

                            for (i, child) in container.children.iter().enumerate() {
                                let bounds = container.node_bounds[i];
                                let height = match bounds.expand {
                                    true => weighted_cells(slot_height, EXPANDED_WEIGHT),
                                    false => weighted_cells(slot_height, bounds.height),
                                };

                                let mut area = Rect::new(
                                    container.area.x,
                                    child_y,
                                    container.area.width,
                                    height,
                                );
                                child_y += height;

                                // last child takes the remaining height because we can get uneven
                                // space from rounding
                                if i == len - 1 {
                                    area.height = container.area.y + container.area.height - area.y;
                                }

                                self.stack.push((*child, area));
                            }
                        }
                        Layout::Vertical => {
                            let len = container.children.len();
                            let len_u16 = len as u16;

                            let inner_gap = 1u16;
                            // There are (len - 1) inner gaps
                            let total_gap = inner_gap * len_u16.saturating_sub(1);

                            let used_area = area.width.saturating_sub(total_gap);

                            let slots = container.calculate_slots_width();
                            let slot_width = used_area as f64 / slots;

                            // Pre-compute child widths using floor, then distribute remaining pixels fairly
                            let mut base_widths: Vec<u16> = Vec::with_capacity(len);
                            let mut sum_widths: u16 = 0;
                            for i in 0..len {
                                let bounds = container.node_bounds[i];
                                let w = match bounds.expand {
                                    true => weighted_cells(slot_width, EXPANDED_WEIGHT),
                                    false => weighted_cells(slot_width, bounds.width),
                                };
                                base_widths.push(w);
                                sum_widths = sum_widths.saturating_add(w);
                            }

                            let mut remaining: u16 = used_area.saturating_sub(sum_widths);
                            let mut widths = base_widths;
                            // Distribute the remainder from left to right to avoid biasing the last pane
                            let mut idx = 0usize;
                            while remaining > 0 && len > 0 {
                                widths[idx] = widths[idx].saturating_add(1);
                                remaining -= 1;
                                idx += 1;
                                if idx >= len {
                                    idx = 0;
                                }
                            }

                            let mut child_x = area.x;
                            let right = container.area.right();
                            for (i, child) in container.children.iter().enumerate() {
                                // Narrower than its gaps, the container can't fit every
                                // child: the ones past its right edge get no columns rather
                                // than an area outside of it.
                                child_x = child_x.min(right);
                                let width = widths[i].min(right - child_x);
                                let mut area = Rect::new(
                                    child_x,
                                    container.area.y,
                                    width,
                                    container.area.height,
                                );

                                // Advance x; add inner gap only between children
                                if i < len - 1 {
                                    child_x = child_x.saturating_add(width + inner_gap);
                                } else {
                                    child_x = child_x.saturating_add(width);
                                }

                                // Ensure last child takes any 1px drift due to rounding
                                if i == len - 1 {
                                    area.width = right - area.x;
                                }

                                self.stack.push((*child, area));
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn traverse(&self) -> Traverse<'_> {
        Traverse::new(self)
    }

    // Finds the split in the given direction if it exists
    pub fn find_split_in_direction(&self, id: ViewId, direction: Direction) -> Option<ViewId> {
        let parent = self.nodes[id].parent;
        // Base case, we found the root of the tree
        if parent == id {
            return None;
        }
        // Parent must always be a container
        let parent_container = match &self.nodes[parent].content {
            Content::Container(container) => container,
            Content::View(_) => unreachable!(),
        };

        match (direction, parent_container.layout) {
            (Direction::Up, Layout::Vertical)
            | (Direction::Left, Layout::Horizontal)
            | (Direction::Right, Layout::Horizontal)
            | (Direction::Down, Layout::Vertical) => {
                // The desired direction of movement is not possible within
                // the parent container so the search must continue closer to
                // the root of the split tree.
                self.find_split_in_direction(parent, direction)
            }
            (Direction::Up, Layout::Horizontal)
            | (Direction::Down, Layout::Horizontal)
            | (Direction::Left, Layout::Vertical)
            | (Direction::Right, Layout::Vertical) => {
                // It's possible to move in the desired direction within
                // the parent container so an attempt is made to find the
                // correct child.
                match self.find_child(id, &parent_container.children, direction) {
                    // Child is found, search is ended
                    Some(id) => Some(id),
                    // A child is not found. This could be because of either two scenarios
                    // 1. Its not possible to move in the desired direction, and search should end
                    // 2. A layout like the following with focus at X and desired direction Right
                    // | _ | x |   |
                    // | _ _ _ |   |
                    // | _ _ _ |   |
                    // The container containing X ends at X so no rightward movement is possible
                    // however there still exists another view/container to the right that hasn't
                    // been explored. Thus another search is done here in the parent container
                    // before concluding it's not possible to move in the desired direction.
                    None => self.find_split_in_direction(parent, direction),
                }
            }
        }
    }

    fn find_child(&self, id: ViewId, children: &[ViewId], direction: Direction) -> Option<ViewId> {
        let mut child_id = match direction {
            // index wise in the child list the Up and Left represents a -1
            // thus reversed iterator.
            Direction::Up | Direction::Left => children
                .iter()
                .rev()
                .skip_while(|i| **i != id)
                .copied()
                .nth(1)?,
            // Down and Right => +1 index wise in the child list
            Direction::Down | Direction::Right => {
                children.iter().skip_while(|i| **i != id).copied().nth(1)?
            }
        };
        let (current_x, current_y) = match &self.nodes[self.focus].content {
            Content::View(current_view) => (current_view.area.left(), current_view.area.top()),
            Content::Container(_) => unreachable!(),
        };

        // If the child is a container the search finds the closest container child
        // visually based on screen location.
        while let Content::Container(container) = &self.nodes[child_id].content {
            match (direction, container.layout) {
                (_, Layout::Vertical) => {
                    // find closest split based on x because y is irrelevant
                    // in a vertical container (and already correct based on previous search)
                    child_id = *container.children.iter().min_by_key(|id| {
                        let x = match &self.nodes[**id].content {
                            Content::View(view) => view.area.left(),
                            Content::Container(container) => container.area.left(),
                        };
                        (current_x as i16 - x as i16).abs()
                    })?;
                }
                (_, Layout::Horizontal) => {
                    // find closest split based on y because x is irrelevant
                    // in a horizontal container (and already correct based on previous search)
                    child_id = *container.children.iter().min_by_key(|id| {
                        let y = match &self.nodes[**id].content {
                            Content::View(view) => view.area.top(),
                            Content::Container(container) => container.area.top(),
                        };
                        (current_y as i16 - y as i16).abs()
                    })?;
                }
            }
        }
        Some(child_id)
    }

    pub fn prev(&self) -> ViewId {
        // This function is very dumb, but that's because we don't store any parent links.
        // (we'd be able to go parent.prev_sibling() recursively until we find something)
        // For now that's okay though, since it's unlikely you'll be able to open a large enough
        // number of splits to notice.

        let mut views = self
            .traverse()
            .rev()
            .skip_while(|&(id, _view)| id != self.focus)
            .skip(1); // Skip focused value
        if let Some((id, _)) = views.next() {
            id
        } else {
            // extremely crude, take the last item
            let (key, _) = self.traverse().next_back().unwrap();
            key
        }
    }

    pub fn next(&self) -> ViewId {
        // This function is very dumb, but that's because we don't store any parent links.
        // (we'd be able to go parent.next_sibling() recursively until we find something)
        // For now that's okay though, since it's unlikely you'll be able to open a large enough
        // number of splits to notice.

        let mut views = self
            .traverse()
            .skip_while(|&(id, _view)| id != self.focus)
            .skip(1); // Skip focused value
        if let Some((id, _)) = views.next() {
            id
        } else {
            // extremely crude, take the first item again
            let (key, _) = self.traverse().next().unwrap();
            key
        }
    }

    pub fn transpose(&mut self) {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;
        if let Content::Container(container) = &mut self.nodes[parent].content {
            container.layout = match container.layout {
                Layout::Vertical => Layout::Horizontal,
                Layout::Horizontal => Layout::Vertical,
            };
            self.recalculate();
        }
    }

    fn get_active_node_bounds_mut(
        &mut self,
        expect_layout: Layout,
    ) -> Option<&mut ContainerBounds> {
        let mut focus = self.focus;
        let mut parent = self.nodes[focus].parent;

        // Parent expected to be container
        if let Some(focused_layout) = match &self.nodes[parent].content {
            Content::View(_) => unreachable!(),
            Content::Container(node) => Some(node.layout),
        } {
            // if we want to make a width change and we have a `Horizontal` layout focused,
            // alter the parent `Vertical` layout instead and vice versa
            if focused_layout != expect_layout {
                focus = parent;
                parent = self.nodes[parent].parent;
            }

            if let Content::Container(node) = &mut self.nodes[parent].content {
                return node.as_mut().get_child_by_view_id(focus);
            };
        }
        None
    }

    pub fn resize_buffer(
        &mut self,
        resize_type: Resize,
        dimension: Dimension,
        config: &crate::editor::Config,
    ) {
        match dimension {
            Dimension::Width => {
                let terminal_width = self.area.width;
                if let Some(bounds) = self.get_active_node_bounds_mut(Layout::Vertical) {
                    match resize_type {
                        Resize::Shrink => {
                            if bounds.width > MIN_WEIGHT {
                                bounds.width = (bounds.width - 1.0).max(MIN_WEIGHT);
                            }
                        }
                        Resize::Grow => {
                            let max_width = if config.max_panel_width == 0 {
                                // Dynamic limit based on configurable percentage of terminal width
                                (terminal_width as f32 * config.max_panel_width_percent) as usize
                            } else {
                                config.max_panel_width
                            } as Weight;

                            if bounds.width < max_width {
                                bounds.width = (bounds.width + 1.0).min(max_width);
                            }
                        }
                    };
                    self.recalculate();
                }
            }
            Dimension::Height => {
                let terminal_height = self.area.height;
                if let Some(bounds) = self.get_active_node_bounds_mut(Layout::Horizontal) {
                    match resize_type {
                        Resize::Shrink => {
                            if bounds.height > MIN_WEIGHT {
                                bounds.height = (bounds.height - 1.0).max(MIN_WEIGHT);
                            }
                        }
                        Resize::Grow => {
                            let max_height = if config.max_panel_height == 0 {
                                // Dynamic limit based on configurable percentage of terminal height
                                (terminal_height as f32 * config.max_panel_height_percent) as usize
                            } else {
                                config.max_panel_height
                            } as Weight;

                            if bounds.height < max_height {
                                bounds.height = (bounds.height + 1.0).min(max_height);
                            }
                        }
                    };
                    self.recalculate();
                }
            }
        }
    }

    fn node_area(&self, id: ViewId) -> Rect {
        match &self.nodes[id].content {
            Content::View(view) => view.area,
            Content::Container(container) => container.area,
        }
    }

    /// The divider under the screen position, if there is one: the one-column
    /// gap between two side by side splits, or the statusline row of a split
    /// that has another one stacked below it.
    pub fn divider_at(&self, row: u16, column: u16) -> Option<SplitDivider> {
        for (id, node) in self.nodes.iter() {
            let Content::Container(container) = &node.content else {
                continue;
            };
            let area = container.area;
            if column < area.x || column >= area.right() || row < area.y || row >= area.bottom() {
                continue;
            }
            for (index, pair) in container.children.windows(2).enumerate() {
                let first = self.node_area(pair[0]);
                let hit = match container.layout {
                    Layout::Vertical => column == first.right(),
                    Layout::Horizontal => row == first.bottom().saturating_sub(1),
                };
                if hit {
                    return Some(SplitDivider {
                        container: id,
                        index,
                        vertical: container.layout == Layout::Vertical,
                    });
                }
            }
        }
        None
    }

    /// Moves `divider` so that it follows the pointer at (`row`, `column`),
    /// growing one neighbouring split and shrinking the other by the same
    /// amount. Sizes are clamped so neither neighbour vanishes. Returns whether
    /// the layout changed.
    ///
    /// The container's weights are rewritten in proportion to the splits'
    /// current sizes (which keeps all the others exactly where they are)
    /// before the two neighbours are adjusted. The weights stay in the usual
    /// unit (averaging [`DEFAULT_WEIGHT`]), so focus mode and keyboard
    /// resizing keep working afterwards. Any "focus mode" expansion in the
    /// container ends, as the weights now describe the real sizes.
    pub fn drag_divider(&mut self, divider: SplitDivider, row: u16, column: u16) -> bool {
        let SplitDivider {
            container, index, ..
        } = divider;
        let Some(Content::Container(node)) = self.nodes.get(container).map(|n| &n.content) else {
            return false;
        };
        let layout = node.layout;
        let (Some(&first), Some(_)) = (node.children.get(index), node.children.get(index + 1))
        else {
            return false;
        };
        let sizes: Vec<u16> = node
            .children
            .iter()
            .map(|&child| {
                let area = self.node_area(child);
                match layout {
                    Layout::Vertical => area.width,
                    Layout::Horizontal => area.height,
                }
            })
            .collect();

        let first_area = self.node_area(first);
        let (wanted, min) = match layout {
            // The gap column sits right after the first split.
            Layout::Vertical => (column.saturating_sub(first_area.x), MIN_SPLIT_WIDTH),
            // The statusline row is the first split's last row.
            Layout::Horizontal => ((row + 1).saturating_sub(first_area.y), MIN_SPLIT_HEIGHT),
        };
        let total = sizes[index] + sizes[index + 1];
        if total < min * 2 {
            return false;
        }
        let new_first = wanted.clamp(min, total - min);
        if new_first == sizes[index] {
            return false;
        }
        let new_second = total - new_first;

        let Some(Content::Container(node)) = self.nodes.get_mut(container).map(|n| &mut n.content)
        else {
            return false;
        };
        // Cells per weight unit, chosen so the average weight is DEFAULT_WEIGHT.
        let total_cells: u32 = sizes.iter().map(|&s| s.max(1) as u32).sum();
        let unit = sizes.len() as f64 * DEFAULT_WEIGHT / total_cells.max(1) as f64;
        for (i, bounds) in node.node_bounds.iter_mut().enumerate() {
            let size = if i == index {
                new_first
            } else if i == index + 1 {
                new_second
            } else {
                sizes.get(i).copied().unwrap_or(1)
            };
            let weight = size.max(1) as f64 * unit;
            match layout {
                Layout::Vertical => bounds.width = weight,
                Layout::Horizontal => bounds.height = weight,
            }
            bounds.expand = false;
        }
        self.recalculate();
        true
    }

    pub fn toggle_focus_window(&mut self) {
        if let Some(bounds) = self.get_active_node_bounds_mut(Layout::Horizontal) {
            bounds.expand = !bounds.expand;
        }

        if let Some(bounds) = self.get_active_node_bounds_mut(Layout::Vertical) {
            bounds.expand = !bounds.expand;
        }

        self.recalculate();
    }

    pub fn swap_split_in_direction(&mut self, direction: Direction) -> Option<()> {
        let focus = self.focus;
        let target = self.find_split_in_direction(focus, direction)?;
        let focus_parent = self.nodes[focus].parent;
        let target_parent = self.nodes[target].parent;

        if focus_parent == target_parent {
            let parent = focus_parent;
            let [parent, focus, target] = self.nodes.get_disjoint_mut([parent, focus, target])?;
            match (&mut parent.content, &mut focus.content, &mut target.content) {
                (
                    Content::Container(parent),
                    Content::View(focus_view),
                    Content::View(target_view),
                ) => {
                    let focus_pos = parent.children.iter().position(|id| focus_view.id == *id)?;
                    let target_pos = parent
                        .children
                        .iter()
                        .position(|id| target_view.id == *id)?;
                    // swap node positions so that traversal order is kept
                    parent.children[focus_pos] = target_view.id;
                    parent.children[target_pos] = focus_view.id;
                    // swap area so that views rendered at the correct location
                    std::mem::swap(&mut focus_view.area, &mut target_view.area);

                    Some(())
                }
                _ => unreachable!(),
            }
        } else {
            let [focus_parent, target_parent, focus, target] =
                self.nodes
                    .get_disjoint_mut([focus_parent, target_parent, focus, target])?;
            match (
                &mut focus_parent.content,
                &mut target_parent.content,
                &mut focus.content,
                &mut target.content,
            ) {
                (
                    Content::Container(focus_parent),
                    Content::Container(target_parent),
                    Content::View(focus_view),
                    Content::View(target_view),
                ) => {
                    let focus_pos = focus_parent
                        .children
                        .iter()
                        .position(|id| focus_view.id == *id)?;
                    let target_pos = target_parent
                        .children
                        .iter()
                        .position(|id| target_view.id == *id)?;
                    // re-parent target and focus nodes
                    std::mem::swap(
                        &mut focus_parent.children[focus_pos],
                        &mut target_parent.children[target_pos],
                    );
                    std::mem::swap(&mut focus.parent, &mut target.parent);
                    // swap area so that views rendered at the correct location
                    std::mem::swap(&mut focus_view.area, &mut target_view.area);

                    Some(())
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn area(&self) -> Rect {
        self.area
    }
}

#[derive(Debug)]
pub struct Traverse<'a> {
    tree: &'a Tree,
    stack: Vec<ViewId>, // TODO: reuse the one we use on update
}

impl<'a> Traverse<'a> {
    fn new(tree: &'a Tree) -> Self {
        Self {
            tree,
            stack: vec![tree.root],
        }
    }
}

impl<'a> Iterator for Traverse<'a> {
    type Item = (ViewId, &'a View);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let key = self.stack.pop()?;

            let node = &self.tree.nodes[key];

            match &node.content {
                Content::View(view) => return Some((key, view)),
                Content::Container(container) => {
                    self.stack.extend(container.children.iter().rev());
                }
            }
        }
    }
}

impl DoubleEndedIterator for Traverse<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            let key = self.stack.pop()?;

            let node = &self.tree.nodes[key];

            match &node.content {
                Content::View(view) => return Some((key, view)),
                Content::Container(container) => {
                    self.stack.extend(container.children.iter());
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::editor::GutterConfig;
    use crate::DocumentId;

    fn new_view() -> View {
        View::new(DocumentId::default(), GutterConfig::default())
    }

    #[test]
    fn tabs_have_their_own_views_layout_and_focus() {
        let mut tree = Tree::new(Rect::new(0, 0, 100, 40));
        let first = tree.insert(new_view());
        let second = tree.split(new_view(), Layout::Vertical);
        assert_eq!((tree.tab_count(), tree.active_tab()), (1_usize, 0_usize));

        tree.new_tab();
        assert!(
            tree.is_empty(),
            "a new tab has no views until one is inserted"
        );
        let third = tree.insert(new_view());
        assert_eq!((tree.tab_count(), tree.active_tab()), (2_usize, 1_usize));
        // only the tab shown: one view, the whole area
        let shown: Vec<ViewId> = tree.views().map(|(view, _)| view.id).collect();
        assert_eq!(shown, vec![third]);
        assert_eq!(tree.get(third).area.width, 100_u16);
        assert_eq!(tree.views_mut().count(), 1_usize);
        assert_eq!(tree.all_views().count(), 3_usize);
        assert_eq!(tree.tab_of(first), Some(0_usize));
        assert_eq!(tree.tab_of(third), Some(1_usize));

        // back: the first tab's focus and split come back
        tree.switch_tab(0);
        assert_eq!(tree.focus, second);
        let mut shown: Vec<ViewId> = tree.views().map(|(view, _)| view.id).collect();
        shown.sort();
        let mut expected = vec![first, second];
        expected.sort();
        assert_eq!(shown, expected);
        assert_eq!(tree.tab_views(0), vec![first, second]);
        assert_eq!(tree.tab_focus(1), Some(third));
        tree.switch_tab(7);
        assert_eq!(tree.active_tab(), 0_usize, "no such tab: nothing changes");
    }

    #[test]
    fn a_tab_whose_last_view_goes_is_closed() {
        let mut tree = Tree::new(Rect::new(0, 0, 100, 40));
        let first = tree.insert(new_view());
        tree.new_tab();
        let second = tree.insert(new_view());
        tree.new_tab();
        let third = tree.insert(new_view());
        assert_eq!(tree.tab_count(), 3_usize);

        // a view of a tab not shown: that tab closes, the one shown stays shown
        tree.remove(second);
        assert_eq!(tree.tab_count(), 2_usize);
        assert_eq!(tree.focus, third);
        assert_eq!(tree.active_tab(), 1_usize);
        assert!(!tree.contains(second));

        // the shown tab's last view: the tab before it is shown
        tree.remove(third);
        assert_eq!(tree.tab_count(), 1_usize);
        assert_eq!(tree.focus, first);
        assert!(!tree.is_empty());

        // the last tab's last view: the tree is empty (Helix quits)
        tree.remove(first);
        assert_eq!(tree.tab_count(), 1_usize);
        assert!(tree.is_empty());
    }

    #[test]
    fn a_tab_can_move() {
        let mut tree = Tree::new(Rect::new(0, 0, 100, 40));
        let first = tree.insert(new_view());
        tree.new_tab();
        let second = tree.insert(new_view());
        tree.move_tab(0);
        assert_eq!(tree.active_tab(), 0_usize);
        assert_eq!(tree.tab_views(0), vec![second]);
        assert_eq!(tree.tab_views(1), vec![first]);
        tree.move_tab(9);
        assert_eq!(tree.active_tab(), 1_usize);
    }

    #[test]
    fn find_split_in_direction() {
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 80,
        });
        let mut view = View::new(DocumentId::default(), GutterConfig::default());
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let l0 = tree.focus;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let r0 = tree.focus;

        tree.focus = l0;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Horizontal);
        let l1 = tree.focus;

        tree.focus = l0;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);

        // Tree in test
        // | L0  | L2 |    |
        // |    L1    | R0 |
        let l2 = tree.focus;
        assert_eq!(Some(l0), tree.find_split_in_direction(l2, Direction::Left));
        assert_eq!(Some(l1), tree.find_split_in_direction(l2, Direction::Down));
        assert_eq!(Some(r0), tree.find_split_in_direction(l2, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(l2, Direction::Up));

        tree.focus = l1;
        assert_eq!(None, tree.find_split_in_direction(l1, Direction::Left));
        assert_eq!(None, tree.find_split_in_direction(l1, Direction::Down));
        assert_eq!(Some(r0), tree.find_split_in_direction(l1, Direction::Right));
        assert_eq!(Some(l0), tree.find_split_in_direction(l1, Direction::Up));

        tree.focus = l0;
        assert_eq!(None, tree.find_split_in_direction(l0, Direction::Left));
        assert_eq!(Some(l1), tree.find_split_in_direction(l0, Direction::Down));
        assert_eq!(Some(l2), tree.find_split_in_direction(l0, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(l0, Direction::Up));

        tree.focus = r0;
        assert_eq!(Some(l2), tree.find_split_in_direction(r0, Direction::Left));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Down));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Up));
    }

    #[test]
    fn swap_split_in_direction() {
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 80,
        });

        let doc_l0 = DocumentId::default();
        let mut view = View::new(doc_l0, GutterConfig::default());
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let l0 = tree.focus;

        let doc_r0 = DocumentId::default();
        let view = View::new(doc_r0, GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let r0 = tree.focus;

        tree.focus = l0;

        let doc_l1 = DocumentId::default();
        let view = View::new(doc_l1, GutterConfig::default());
        tree.split(view, Layout::Horizontal);
        let l1 = tree.focus;

        tree.focus = l0;

        let doc_l2 = DocumentId::default();
        let view = View::new(doc_l2, GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let l2 = tree.focus;

        // Views in test
        // | L0  | L2 |    |
        // |    L1    | R0 |

        // Document IDs in test
        // | l0  | l2 |    |
        // |    l1    | r0 |

        fn doc_id(tree: &Tree, view_id: ViewId) -> Option<DocumentId> {
            if let Content::View(view) = &tree.nodes[view_id].content {
                Some(view.doc)
            } else {
                None
            }
        }

        tree.focus = l0;
        // `*` marks the view in focus from view table (here L0)
        // | l0*  | l2 |    |
        // |    l1     | r0 |
        tree.swap_split_in_direction(Direction::Down);
        // | l1   | l2 |    |
        // |    l0*    | r0 |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_l0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_r0));

        tree.swap_split_in_direction(Direction::Right);

        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        // cannot swap, nothing changes
        tree.swap_split_in_direction(Direction::Up);
        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        // cannot swap, nothing changes
        tree.swap_split_in_direction(Direction::Down);
        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        tree.focus = l2;
        // | l1  | l2* |    |
        // |    r0     | l0 |

        tree.swap_split_in_direction(Direction::Down);
        // | l1  | r0  |    |
        // |    l2*    | l0 |
        assert_eq!(tree.focus, l2);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_l2));
        assert_eq!(doc_id(&tree, l2), Some(doc_r0));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        tree.swap_split_in_direction(Direction::Up);
        // | l2* | r0 |    |
        // |    l1    | l0 |
        assert_eq!(tree.focus, l2);
        assert_eq!(doc_id(&tree, l0), Some(doc_l2));
        assert_eq!(doc_id(&tree, l1), Some(doc_l1));
        assert_eq!(doc_id(&tree, l2), Some(doc_r0));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));
    }

    #[test]
    fn all_vertical_views_have_same_width() {
        let tree_area_width = 180;
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: tree_area_width,
            height: 80,
        });
        let mut view = View::new(DocumentId::default(), GutterConfig::default());
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);

        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Horizontal);

        tree.remove(tree.focus);

        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);

        // Make sure that we only have one level in the tree.
        assert_eq!(3, tree.views().count());
        assert_eq!(
            vec![
                tree_area_width / 3,
                tree_area_width / 3 - 1, // gap here
                tree_area_width / 3 - 1, // gap here
            ],
            tree.views()
                .map(|(view, _)| view.area.width)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn vsplit_gap_rounding() {
        let (tree_area_width, tree_area_height) = (80, 24);
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: tree_area_width,
            height: tree_area_height,
        });
        let mut view = View::new(DocumentId::default(), GutterConfig::default());
        view.area = Rect::new(0, 0, tree_area_width, tree_area_height);
        tree.insert(view);

        for _ in 0..9 {
            let view = View::new(DocumentId::default(), GutterConfig::default());
            tree.split(view, Layout::Vertical);
        }

        assert_eq!(10, tree.views().count());
        assert_eq!(
            std::iter::once(8) // Rounding in `recalculate`.
                .chain(std::iter::repeat_n(7, 9))
                .collect::<Vec<_>>(),
            tree.views()
                .map(|(view, _)| view.area.width)
                .collect::<Vec<_>>()
        );
    }

    /// Helper to create a tree with `count` views in the given layout.
    /// Returns the tree and a Vec of ViewIds in creation order.
    fn make_tree(area: Rect, layout: Layout, count: usize) -> (Tree, Vec<ViewId>) {
        let mut tree = Tree::new(area);
        let mut ids = Vec::new();
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.insert(view);
        ids.push(tree.focus);
        for _ in 1..count {
            let view = View::new(DocumentId::default(), GutterConfig::default());
            tree.split(view, layout);
            ids.push(tree.focus);
        }
        (tree, ids)
    }

    /// Helper to check that all views have approximately the same value in a
    /// given dimension, allowing at most 1px difference from rounding.
    fn assert_approx_equal(widths: &[u16]) {
        let min = *widths.iter().min().unwrap();
        let max = *widths.iter().max().unwrap();
        assert!(
            max - min <= 1,
            "expected widths within 1px of each other, got {:?}",
            widths
        );
    }

    #[test]
    fn remove_middle_of_three_flat_vertical_splits() {
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 3);

        assert_eq!(tree.views().count(), 3);
        tree.focus = ids[1];
        tree.remove(ids[1]);

        assert_eq!(tree.views().count(), 2);
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        // Each view should get roughly half the total width minus gaps.
        // Total gap for 2 vertical children = 1px.
        // Used area = 180 - 1 = 179. Each gets ~89-90.
        assert!(widths[0] + widths[1] + 1 == width);
        // Focus must be on a surviving view.
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_first_of_three_flat_vertical_splits() {
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 3);

        tree.focus = ids[0];
        tree.remove(ids[0]);

        assert_eq!(tree.views().count(), 2);
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        assert!(widths[0] + widths[1] + 1 == width);
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_last_of_three_flat_vertical_splits() {
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 3);

        tree.focus = ids[2];
        tree.remove(ids[2]);

        assert_eq!(tree.views().count(), 2);
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        assert!(widths[0] + widths[1] + 1 == width);
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_middle_of_three_flat_horizontal_splits() {
        let (area, height) = (Rect::new(0, 0, 180, 80), 80);
        let (mut tree, ids) = make_tree(area, Layout::Horizontal, 3);

        assert_eq!(tree.views().count(), 3);
        tree.focus = ids[1];
        tree.remove(ids[1]);

        assert_eq!(tree.views().count(), 2);
        let heights: Vec<u16> = tree.views().map(|(v, _)| v.area.height).collect();
        assert_approx_equal(&heights);
        // Horizontal layout has no gaps. Total height = 80.
        assert!(heights[0] + heights[1] == height);
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_from_nested_same_layout_flattens() {
        // Remove a view that causes merge-up, promoting a container whose
        // layout matches its new parent.  The promoted container should be
        // flattened so its children become direct children of the grandparent.
        //
        // Expected tree after construction:
        //
        // root(Vertical)
        // ├── h_container(Horizontal)
        // │   ├── v_container(Vertical)     ← same layout as root
        // │   │   ├── V0
        // │   │   └── V3
        // │   └── V2
        // └── V1
        //
        // After removing V2:
        //   1. h_container has only v_container left → merge-up
        //   2. v_container promoted to root → root = [v_container, V1]
        //   3. v_container(Vertical) == root(Vertical) → flatten
        //   4. root = [V0, V3, V1]
        //
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let mut tree = Tree::new(area);

        // Insert V0
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.insert(view);
        let v0 = tree.focus;

        // vsplit V1 → root(Vertical) = [V0, V1]
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let v1 = tree.focus;

        // Focus V0, hsplit V2 → root = [h_container(H, [V0, V2]), V1]
        tree.focus = v0;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Horizontal);
        let v2 = tree.focus;

        // Focus V0, vsplit V3 → h_container = [v_container(V, [V0, V3]), V2]
        tree.focus = v0;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let v3 = tree.focus;

        assert_eq!(tree.views().count(), 4);

        // Remove V2.  h_container has only v_container left → merge-up.
        // v_container(Vertical) promoted to root(Vertical) → flatten.
        tree.focus = v2;
        tree.remove(v2);

        assert_eq!(tree.views().count(), 3);

        // After flattening, root(Vertical) = [V0, V3, V1].
        // Total gap for 3 vertical children = 2px.
        // Used area = 178, each gets ~59-60.
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        assert!(
            widths.iter().sum::<u16>() + 2 == width,
            "total widths + gaps should equal area width, got {:?}",
            widths
        );
        // All three surviving views should be present.
        let remaining: Vec<ViewId> = tree.views().map(|(v, _)| v.id).collect();
        assert!(remaining.contains(&v0));
        assert!(remaining.contains(&v3));
        assert!(remaining.contains(&v1));
    }

    #[test]
    fn remove_does_not_flatten_different_layout_containers() {
        // root(Vertical) -> [h_container(Horizontal, [V0, V1, V2]), V3]
        //
        // Construction:
        let (area, _width) = (Rect::new(0, 0, 180, 80), 180);
        let mut tree = Tree::new(area);

        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.insert(view);
        let v0 = tree.focus;

        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let v3 = tree.focus;

        // Focus V0, hsplit V1 → creates h_container(Horizontal, [V0, V1])
        tree.focus = v0;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        let v1 = tree.split(view, Layout::Horizontal);

        // Focus V1, hsplit V2 → added to h_container (same layout)
        tree.focus = v1;
        let view = View::new(DocumentId::default(), GutterConfig::default());
        tree.split(view, Layout::Horizontal);
        let v2 = tree.focus;

        assert_eq!(tree.views().count(), 4);

        // Remove V1. h_container has 2 children left [V0, V2].
        // h_container(Horizontal) != root(Vertical) → should NOT flatten.
        tree.focus = v1;
        tree.remove(v1);

        assert_eq!(tree.views().count(), 3);

        // root has 2 children: h_container and V3.
        // Total gap = 1px, used area = 179. h_container gets ~90, V3 gets ~89.
        let v3_view = tree.get(v3);
        assert!(
            v3_view.area.width == 89 || v3_view.area.width == 90,
            "V3 should get roughly half the width, got {}",
            v3_view.area.width
        );

        // V0 and V2 are stacked vertically inside h_container (no gaps).
        // h_container height = 80, each gets 40.
        let v0_view = tree.get(v0);
        let v2_view = tree.get(v2);
        assert_eq!(v0_view.area.height, 40);
        assert_eq!(v2_view.area.height, 40);
        assert_eq!(v0_view.area.y, 0);
        assert_eq!(v2_view.area.y, 40);
    }

    #[test]
    fn remove_middle_of_four_vertical_splits() {
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 4);

        tree.focus = ids[1];
        tree.remove(ids[1]);

        assert_eq!(tree.views().count(), 3);
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        // Total gap for 3 children = 2px, used area = 178.
        assert!(widths.iter().sum::<u16>() + 2 == width);
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_middle_of_five_vertical_splits() {
        let (area, width) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 5);

        tree.focus = ids[2];
        tree.remove(ids[2]);

        assert_eq!(tree.views().count(), 4);
        let widths: Vec<u16> = tree.views().map(|(v, _)| v.area.width).collect();
        assert_approx_equal(&widths);
        // Total gap for 4 children = 3px, used area = 177.
        assert!(widths.iter().sum::<u16>() + 3 == width);
        assert!(ids.contains(&tree.focus));
    }

    #[test]
    fn remove_focus_moves_to_remaining_view() {
        let (area, _) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 3);

        tree.focus = ids[1];
        tree.remove(ids[1]);

        // Focus must be on a surviving view, not the removed one.
        assert_ne!(tree.focus, ids[1]);
        assert!(tree.contains(tree.focus));
        assert!(tree.try_get(tree.focus).is_some());
    }

    #[test]
    fn remove_preserves_document_association() {
        let (area, _) = (Rect::new(0, 0, 180, 80), 180);
        let mut tree = Tree::new(area);

        let doc_a = DocumentId::default();
        let doc_b = DocumentId::default();
        let doc_c = DocumentId::default();

        let view = View::new(doc_a, GutterConfig::default());
        tree.insert(view);
        let v0 = tree.focus;

        let view = View::new(doc_b, GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let v1 = tree.focus;

        let view = View::new(doc_c, GutterConfig::default());
        tree.split(view, Layout::Vertical);
        let v2 = tree.focus;

        // Remove the middle view.
        tree.focus = v1;
        tree.remove(v1);

        // Remaining views must still reference their original documents.
        assert_eq!(tree.get(v0).doc, doc_a);
        assert_eq!(tree.get(v2).doc, doc_c);
    }

    #[test]
    fn remove_single_view_tree_does_not_panic() {
        let (area, _) = (Rect::new(0, 0, 180, 80), 180);
        let (mut tree, ids) = make_tree(area, Layout::Vertical, 1);

        assert_eq!(tree.views().count(), 1);
        tree.remove(ids[0]);

        // Tree should be empty; focus reset to root.
        assert!(tree.is_empty());
        assert_eq!(tree.focus, tree.root);
    }

    fn new_tree(width: u16, height: u16) -> (Tree, ViewId) {
        let mut tree = Tree::new(Rect::new(0, 0, width, height));
        let first = tree.insert(View::new(DocumentId::default(), GutterConfig::default()));
        (tree, first)
    }

    #[test]
    fn docked_windows_sit_along_an_edge_at_their_size() {
        let (mut tree, main) = new_tree(101, 40);
        let other = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        tree.focus = main;
        // on the left, 30 columns: the others share the rest; the focus stays
        let left = tree.dock(
            View::new(DocumentId::default(), GutterConfig::default()),
            DockSide::Left,
            30,
        );
        assert_eq!(tree.focus, main);
        assert_eq!(tree.get(left).area.x, 0);
        assert_eq!(tree.get(left).area.width, 30);
        assert_eq!(tree.get(left).area.height, 40);
        assert!(tree.get(main).area.x > 30);
        let widths =
            tree.get(left).area.width + tree.get(main).area.width + tree.get(other).area.width;
        assert_eq!(widths, 99, "101 columns, two 1-column gaps");

        // at the bottom: across the whole width, under all of them
        let bottom = tree.dock(
            View::new(DocumentId::default(), GutterConfig::default()),
            DockSide::Bottom,
            8,
        );
        assert_eq!(tree.get(bottom).area.height, 8);
        assert_eq!(tree.get(bottom).area.width, 101);
        assert_eq!(tree.get(bottom).area.y, 32);
        assert_eq!(tree.get(left).area.height, 32);
        assert_eq!(tree.get(left).area.width, 30);
        assert_eq!(tree.views().count(), 4_usize);

        // closing it gives the room back
        tree.remove(bottom);
        assert_eq!(tree.get(left).area.height, 40);

        // too big a size: half the width at most
        let right = tree.dock(
            View::new(DocumentId::default(), GutterConfig::default()),
            DockSide::Right,
            500,
        );
        assert_eq!(tree.get(right).area.right(), 101);
        assert!(
            tree.get(right).area.width <= 51,
            "{}",
            tree.get(right).area.width
        );
        assert!(tree.get(main).area.width >= MIN_SPLIT_WIDTH);
        assert!(tree.get(other).area.width >= MIN_SPLIT_WIDTH);
    }

    #[test]
    fn a_docked_window_in_an_empty_tab_gets_the_focus() {
        let mut tree = Tree::new(Rect::new(0, 0, 80, 20));
        let docked = tree.dock(
            View::new(DocumentId::default(), GutterConfig::default()),
            DockSide::Top,
            5,
        );
        assert_eq!(tree.focus, docked);
        assert_eq!(tree.get(docked).area.height, 20);
    }

    #[test]
    fn dragging_a_vertical_divider_resizes_both_neighbours() {
        let (mut tree, left) = new_tree(101, 40);
        let right = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        // 101 columns minus the 1-column gap, shared out evenly.
        let (lw, rw) = (tree.get(left).area.width, tree.get(right).area.width);
        assert_eq!(lw + rw, 100);

        // The gap column right after the left split is the handle.
        let gap = tree.get(left).area.right();
        let divider = tree.divider_at(10, gap).expect("divider on the gap");
        assert_eq!(tree.divider_at(10, gap + 5), None);

        assert!(tree.drag_divider(divider, 10, 30));
        assert_eq!(tree.get(left).area.width, 30);
        assert_eq!(tree.get(right).area.width, 70);
        // The right split still starts one gap column after the left one.
        assert_eq!(tree.get(right).area.x, 31);

        // The divider follows the pointer back too.
        assert!(tree.drag_divider(divider, 10, 60));
        assert_eq!(tree.get(left).area.width, 60);
        assert_eq!(tree.get(right).area.width, 40);

        // Neither neighbour can be squeezed out.
        assert!(tree.drag_divider(divider, 10, 0));
        assert_eq!(tree.get(left).area.width, MIN_SPLIT_WIDTH);
        assert!(tree.drag_divider(divider, 10, 500));
        assert_eq!(tree.get(right).area.width, MIN_SPLIT_WIDTH);
        assert_eq!(tree.get(left).area.width + tree.get(right).area.width, 100);
    }

    #[test]
    fn dragging_a_divider_leaves_other_splits_alone() {
        let (mut tree, a) = new_tree(122, 40);
        let b = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        tree.focus = b;
        let c = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        let c_width = tree.get(c).area.width;

        let divider = tree
            .divider_at(5, tree.get(a).area.right())
            .expect("divider between a and b");
        let a_width = tree.get(a).area.width;
        assert!(tree.drag_divider(divider, 5, a_width.saturating_sub(10)));
        assert_eq!(tree.get(a).area.width, a_width - 10);
        assert_eq!(tree.get(c).area.width, c_width);
    }

    #[test]
    fn dragging_a_horizontal_divider_resizes_stacked_splits() {
        let (mut tree, top) = new_tree(80, 41);
        let bottom = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Horizontal,
        );
        let total = tree.get(top).area.height + tree.get(bottom).area.height;
        assert_eq!(total, 41);

        // The statusline row of the upper split is the handle.
        let status_row = tree.get(top).area.bottom() - 1;
        let divider = tree
            .divider_at(status_row, 10)
            .expect("divider on statusline");
        assert_eq!(tree.divider_at(status_row - 1, 10), None);

        assert!(tree.drag_divider(divider, 10, 10));
        assert_eq!(tree.get(top).area.height, 11);
        assert_eq!(tree.get(bottom).area.height, total - 11);
        assert_eq!(tree.get(bottom).area.y, 11);

        assert!(tree.drag_divider(divider, 0, 10));
        assert_eq!(tree.get(top).area.height, MIN_SPLIT_HEIGHT);
    }

    #[test]
    fn focus_mode_after_dragging_grows_the_focused_split() {
        let (mut tree, left) = new_tree(101, 40);
        let right = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        let gap = tree.get(left).area.right();
        let divider = tree.divider_at(10, gap).expect("divider on the gap");
        assert!(tree.drag_divider(divider, 10, 60));
        assert_eq!(tree.get(left).area.width, 60);
        assert_eq!(tree.get(right).area.width, 40);

        // Focus mode must enlarge the focused split, never shrink it.
        tree.focus = left;
        tree.toggle_focus_window();
        assert!(tree.get(left).area.width > 60);
        tree.toggle_focus_window();
        assert_eq!(tree.get(left).area.width, 60);

        tree.focus = right;
        tree.toggle_focus_window();
        assert!(tree.get(right).area.width > 40);
        tree.toggle_focus_window();
        assert_eq!(tree.get(right).area.width, 40);
    }

    #[test]
    fn keyboard_resize_still_works_after_dragging() {
        let config = crate::editor::Config::default();
        let (mut tree, left) = new_tree(101, 40);
        let _right = tree.split(
            View::new(DocumentId::default(), GutterConfig::default()),
            Layout::Vertical,
        );
        let gap = tree.get(left).area.right();
        let divider = tree.divider_at(10, gap).expect("divider on the gap");
        assert!(tree.drag_divider(divider, 10, 60));

        tree.focus = left;
        tree.resize_buffer(Resize::Grow, Dimension::Width, &config);
        assert!(tree.get(left).area.width > 60);
        let grown = tree.get(left).area.width;
        tree.resize_buffer(Resize::Shrink, Dimension::Width, &config);
        assert!(tree.get(left).area.width < grown);
    }

    #[test]
    fn a_single_split_has_no_divider() {
        let (tree, view) = new_tree(80, 40);
        let area = tree.get(view).area;
        assert_eq!(tree.divider_at(area.bottom() - 1, area.right()), None);
        assert_eq!(tree.divider_at(area.bottom() - 1, 10), None);
    }

    #[test]
    fn splits_narrower_than_their_gaps_stay_inside_the_tree() {
        for width in [0, 1, 2, 3, 5] {
            let (mut tree, _) = make_tree(Rect::new(0, 0, width, 10), Layout::Vertical, 6);
            let hsplit = tree.split(
                View::new(DocumentId::default(), GutterConfig::default()),
                Layout::Horizontal,
            );
            tree.focus = hsplit;
            tree.split(
                View::new(DocumentId::default(), GutterConfig::default()),
                Layout::Vertical,
            );
            for (view, _) in tree.views() {
                assert!(
                    view.area.right() <= width && view.area.bottom() <= 10,
                    "width {width}: {:?}",
                    view.area
                );
            }
        }
    }
}
