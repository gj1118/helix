//! File tree panel ("explorer"):
//! lazy-loaded directory tree model, configuration, and runtime state.
//!
//! Disk reads happen when a directory is first expanded (`Tree::expand_at`)
//! and in `Tree::refresh`; rendering only touches already materialized
//! state, so opening the explorer is O(1) in project size and every frame
//! renders straight out of cached rows.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Where the explorer panel lives. Pinned panels `left`/`right` reserve a
/// column from the editor viewports; `floating` draws a window over the
/// editor instead.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExplorerSide {
    #[default]
    Left,
    Right,
    Floating,
}

/// Configuration of the explorer panel (`[editor.explorer]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExplorerConfig {
    /// Pinned `left`/`right` or `floating`. Defaults to `left`.
    pub side: ExplorerSide,
    /// Width of the pinned panel in columns. Defaults to `30`.
    pub width: u16,
    /// Width of the floating window in columns. Defaults to `40`.
    pub floating_width: u16,
    /// Height of the floating window in lines. Defaults to `20`.
    pub floating_height: u16,
    /// Draw file type icons (honoring the global `[editor.icons]` settings).
    /// Defaults to `true`.
    pub icons: bool,
    /// Select the current buffer's file whenever the panel is opened.
    /// Defaults to `false`.
    pub auto_reveal: bool,
    /// Show dotfiles. Defaults to `false`.
    pub show_hidden: bool,
    /// Sort directories before files. Defaults to `true`.
    pub directories_first: bool,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            side: ExplorerSide::Left,
            width: 30,
            floating_width: 40,
            floating_height: 20,
            icons: true,
            auto_reveal: false,
            show_hidden: false,
            directories_first: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// An entry in the explorer clipboard. `cut` items are moved on paste,
/// otherwise copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerClipboard {
    pub path: PathBuf,
    pub cut: bool,
}

/// Runtime explorer state owned by [`crate::Editor`]. The `Option` fields are
/// runtime overrides; they default to the config values and persist until
/// helix exits.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExplorerState {
    /// Whether the panel is currently visible.
    pub visible: bool,
    /// Whether the panel has keyboard focus (tree navigation and file
    /// mutations only respond to keys while this is set).
    pub focused: bool,
    /// Runtime side override (`:explorer-left` / `:explorer-right`). Only ever
    /// holds a pinned side; the floating window is tracked by `floating`.
    pub side: Option<ExplorerSide>,
    /// Whether the panel is currently shown as a floating window. This is a
    /// transient presentation of the one tree, owned by the floating toggle
    /// (`:explorer-float-toggle`); closing the window clears it, so the pinned
    /// panel's toggle never reopens a float.
    pub floating: bool,
    /// Runtime width override (`:explorer-width`).
    pub width: Option<u16>,
    /// Runtime show-hidden override (`h`/'H' inside the panel).
    pub show_hidden: Option<bool>,
}

impl ExplorerState {
    /// Effective side of the panel.
    pub fn side(&self, config: &ExplorerConfig) -> ExplorerSide {
        if self.floating {
            ExplorerSide::Floating
        } else {
            self.side.unwrap_or(config.side)
        }
    }

    /// Hides the panel (pinned or floating) and drops its keyboard focus.
    pub fn hide(&mut self) {
        self.visible = false;
        self.focused = false;
        self.floating = false;
    }

    /// Effective width of the pinned panel.
    pub fn width(&self, config: &ExplorerConfig) -> u16 {
        self.width.unwrap_or(config.width)
    }

    /// Effective width of the floating window.
    pub fn floating_width(&self, config: &ExplorerConfig) -> u16 {
        config.floating_width
    }

    /// Effective height of the floating window.
    pub fn floating_height(&self, config: &ExplorerConfig) -> u16 {
        config.floating_height
    }

    /// Whether hidden (dotfile) entries are shown.
    pub fn show_hidden(&self, config: &ExplorerConfig) -> bool {
        self.show_hidden.unwrap_or(config.show_hidden)
    }
}

/// Parameters resolved from config/state that affect tree reads and
/// rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeParams {
    pub show_hidden: bool,
    pub directories_first: bool,
}

impl TreeParams {
    pub fn new(config: &ExplorerConfig, state: &ExplorerState) -> Self {
        Self {
            show_hidden: state.show_hidden(config),
            directories_first: config.directories_first,
        }
    }
}

// ---------------------------------------------------------------------------
// Tree model
// ---------------------------------------------------------------------------

/// One visible row of the tree: a flattened, depth-annotated walk over every
/// expanded directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// Display name (file name component). The root row uses its tail.
    pub name: String,
    /// Nesting depth: root is 0, immediate children are 1, ...
    pub depth: u16,
    pub is_dir: bool,
    /// Directories only: whether currently expanded.
    pub expanded: bool,
    /// Positionally the last child of its parent (drives `└─` vs `├─`).
    pub is_last: bool,
}

#[derive(Debug)]
struct Child {
    path: PathBuf,
    name: String,
    /// Lowercased name, cached so comparisons never allocate.
    sort_key: String,
    is_dir: bool,
}

#[derive(Debug)]
struct Dir {
    expanded: bool,
    children: Vec<Child>,
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Upper bound on the entries a filter scan collects. Past this the scan stops
/// and the result is flagged as truncated (see [`Tree::filter_truncated`]).
pub const FILTER_SCAN_LIMIT: usize = 100_000;

/// Case-insensitive glob match: `*` matches any run of characters, `?` one
/// character, and `[abc]` / `[a-z]` / `[!abc]` a character class.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let txt: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    glob_match_chars(&pat, &txt)
}

/// Matches one `[...]` class at the start of `pat` (which begins with `[`)
/// against `c`. Returns `(matched, pattern length consumed)`, or `None` when the
/// class is not closed (the `[` is then an ordinary character).
fn match_class(pat: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(pat.get(i), Some('!') | Some('^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pat.len() {
        // A `]` right after the opening (or negation) is a literal member.
        if pat[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
            if pat[i] <= c && c <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

fn glob_match_chars(pat: &[char], txt: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    // Position to resume from after the most recent `*`: (pattern index after
    // the star, text index the star currently swallows up to).
    let mut star: Option<(usize, usize)> = None;

    while t < txt.len() {
        let step = match pat.get(p) {
            Some('*') => {
                star = Some((p + 1, t));
                p += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => match match_class(&pat[p..], txt[t]) {
                Some((true, len)) => Some(len),
                Some((false, _)) => None,
                // Unclosed class: `[` is a literal.
                None => (txt[t] == '[').then_some(1),
            },
            Some(&c) => (c == txt[t]).then_some(1),
            None => None,
        };
        match step {
            Some(len) => {
                p += len;
                t += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    // Let the star swallow one more character and retry.
                    star = Some((sp, st + 1));
                    p = sp;
                    t = st + 1;
                }
                None => return false,
            },
        }
    }
    while pat.get(p) == Some(&'*') {
        p += 1;
    }
    p == pat.len()
}

/// Expands `{a,b}` alternations (not nested) into plain patterns, so
/// `*.{rs,toml}` becomes `*.rs` and `*.toml`.
fn expand_braces(pattern: &str) -> Vec<String> {
    if let Some(open) = pattern.find('{') {
        if let Some(len) = pattern[open..].find('}') {
            let close = open + len;
            let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
            return pattern[open + 1..close]
                .split(',')
                .flat_map(|alt| expand_braces(&format!("{head}{alt}{tail}")))
                .collect();
        }
    }
    vec![pattern.to_string()]
}

/// A parsed filter query for the explorer's input box.
///
/// Matching is case-insensitive. A query with no wildcard characters
/// (`* ? [ {`) matches as a substring, so typing `main` finds `main.rs`.
/// Wildcard queries must match the whole name, e.g. `*.rs` or `test_*`. A
/// pattern containing `/` is matched against the path relative to the tree
/// root, any other pattern against the entry's own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    patterns: Vec<String>,
}

impl Filter {
    /// Parses `raw`; blank input means "no filter".
    pub fn new(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let patterns = if raw.contains(['*', '?', '[', '{']) {
            expand_braces(raw)
        } else {
            vec![format!("*{raw}*")]
        };
        Some(Self { patterns })
    }

    /// Whether an entry called `name`, at `relative` (a `/`-separated path
    /// below the tree root), passes the filter.
    pub fn matches(&self, name: &str, relative: &str) -> bool {
        self.patterns.iter().any(|pattern| {
            if pattern.contains('/') {
                glob_match(pattern, relative)
            } else {
                glob_match(pattern, name)
            }
        })
    }
}

/// One entry of the recursive listing a filter searches.
#[derive(Debug)]
struct ScanEntry {
    path: PathBuf,
    name: String,
    /// `/`-separated path below the tree root.
    relative: String,
    is_dir: bool,
}

/// Lazily loaded directory tree.
#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    dirs: HashMap<PathBuf, Dir>,
    visible: Vec<Entry>,
    /// Active filter. While set, `visible` is built from `filter_dirs` (only
    /// matching entries and the directories leading to them) instead of `dirs`.
    filter: Option<Filter>,
    filter_dirs: HashMap<PathBuf, Dir>,
    /// Recursive listing of the whole tree, taken once when a filter starts
    /// and reused while the query is edited (cleared on filter reset/refresh).
    scan: Option<Vec<ScanEntry>>,
    scan_truncated: bool,
}

impl Tree {
    /// Creates a tree rooted at `root`, loading the root's children.
    pub fn new(root: PathBuf, params: TreeParams) -> io::Result<Self> {
        let mut tree = Self {
            root,
            dirs: HashMap::new(),
            visible: Vec::new(),
            filter: None,
            filter_dirs: HashMap::new(),
            scan: None,
            scan_truncated: false,
        };
        let root = tree.root.clone();
        tree.expand(&root, params)?;
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn visible(&self) -> &[Entry] {
        &self.visible
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// Whether `path` is the tree root.
    pub fn is_root(&self, path: &Path) -> bool {
        path == self.root
    }

    /// The directory map the visible rows are built from: the filtered view
    /// while a filter is active, otherwise the full lazily loaded tree.
    fn active_dirs(&self) -> &HashMap<PathBuf, Dir> {
        if self.filter.is_some() {
            &self.filter_dirs
        } else {
            &self.dirs
        }
    }

    fn active_dirs_mut(&mut self) -> &mut HashMap<PathBuf, Dir> {
        if self.filter.is_some() {
            &mut self.filter_dirs
        } else {
            &mut self.dirs
        }
    }

    /// Whether the directory at `path` is expanded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.active_dirs().get(path).is_some_and(|dir| dir.expanded)
    }

    /// Whether `path` is the trailing entry among its parent directory's
    /// cached children. Drives the `│` vs blank guide lines in the renderer.
    pub fn is_last_child(&self, path: &Path) -> bool {
        path.parent()
            .and_then(|parent| self.active_dirs().get(parent))
            .is_some_and(|dir| dir.children.last().is_some_and(|child| child.path == path))
    }

    /// The active filter, if any.
    pub fn filter(&self) -> Option<&Filter> {
        self.filter.as_ref()
    }

    /// Whether the last filter scan hit [`FILTER_SCAN_LIMIT`] and so may be
    /// missing matches.
    pub fn filter_truncated(&self) -> bool {
        self.filter.is_some() && self.scan_truncated
    }

    fn load_children(&self, dir: &Path, params: TreeParams) -> io::Result<Vec<Child>> {
        let mut children = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                // Individual unreadable entries are skipped, not fatal.
                Err(_) => continue,
            };
            let path = entry.path();
            let name = match path.file_name() {
                Some(name) => name.to_string_lossy().into_owned(),
                None => continue,
            };
            if !params.show_hidden && name.starts_with('.') {
                continue;
            }
            // Symlinks are classified by their target: a link pointing at a
            // directory is expanded like a directory, not rendered as a file.
            // `entry.file_type()` reports the link kind, so follow the target
            // only when we must (this runs once per loaded directory).
            let is_dir = match entry.file_type() {
                Ok(ft) if ft.is_dir() => true,
                Ok(ft) if ft.is_symlink() => {
                    fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false)
                }
                _ => false,
            };
            let sort_key = name.to_lowercase();
            children.push(Child {
                path,
                name,
                sort_key,
                is_dir,
            });
        }

        if params.directories_first {
            children.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.sort_key.cmp(&b.sort_key))
            });
        } else {
            children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        }
        Ok(children)
    }

    fn expand(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if self.is_expanded(path) {
            return Ok(());
        }
        if self.filter.is_some() {
            // Inside a filtered view a directory with matches below it just
            // re-opens; a directory that matched by name alone has nothing
            // cached, so its children are listed unfiltered to browse into.
            if let Some(dir) = self.filter_dirs.get_mut(path) {
                dir.expanded = true;
            } else {
                let children = self.load_children(path, params)?;
                self.filter_dirs.insert(
                    path.to_path_buf(),
                    Dir {
                        expanded: true,
                        children,
                    },
                );
            }
            self.rebuild_visible();
            return Ok(());
        }
        let children = self.load_children(path, params)?;
        self.dirs.insert(
            path.to_path_buf(),
            Dir {
                expanded: true,
                children,
            },
        );
        self.rebuild_visible();
        Ok(())
    }

    /// Expands the directory at `path` (no-op when already expanded).
    pub fn expand_at(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if !self.is_root(path) {
            // Only directories are expandable; a missing path errors out so
            // the caller can surface it.
            let meta = fs::metadata(path)?;
            if !meta.is_dir() {
                return Ok(());
            }
        }
        self.expand(path, params)
    }

    /// Collapses the directory at `path` (no-op for files).
    pub fn collapse(&mut self, path: &Path) {
        if let Some(dir) = self.active_dirs_mut().get_mut(path) {
            dir.expanded = false;
        }
        self.rebuild_visible();
    }

    /// Toggles the expansion state of the directory at `path`.
    pub fn toggle(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if self.is_expanded(path) {
            self.collapse(path);
            Ok(())
        } else {
            self.expand_at(path, params)
        }
    }

    /// Reloads every expanded directory from disk: pruning vanished entries
    /// and adding new ones. Collapsed directories are not touched. Directories
    /// that vanished are collapsed; only a vanished root counts as an error
    /// (returned as the error count).
    pub fn refresh(&mut self, params: TreeParams) -> usize {
        let mut errors = 0;
        let paths: Vec<PathBuf> = self
            .dirs
            .iter()
            .filter(|(_, dir)| dir.expanded)
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            match self.load_children(&path, params) {
                Ok(children) => {
                    if let Some(dir) = self.dirs.get_mut(&path) {
                        dir.children = children;
                    }
                }
                Err(_) => {
                    if path == self.root {
                        errors += 1;
                    } else if let Some(dir) = self.dirs.get_mut(&path) {
                        dir.expanded = false;
                    }
                }
            }
        }
        if self.filter.is_some() {
            // The listing the filter searches is stale now; search anew.
            self.scan = None;
            self.apply_filter(params);
        }
        self.rebuild_visible();
        errors
    }

    /// Sets the filter from the input box text (blank clears it). Matching
    /// entries stay visible together with the directories leading to them, so
    /// the result is a pruned tree of the whole workspace, not just of the
    /// directories that happen to be expanded. Clearing the filter restores
    /// the regular tree, with its expansion state, exactly as it was.
    pub fn set_filter(&mut self, raw: &str, params: TreeParams) {
        let filter = Filter::new(raw);
        if filter == self.filter {
            return;
        }
        match filter {
            None => {
                self.filter = None;
                self.filter_dirs.clear();
                self.scan = None;
                self.scan_truncated = false;
            }
            Some(filter) => {
                self.filter = Some(filter);
                self.apply_filter(params);
            }
        }
        self.rebuild_visible();
    }

    /// Lists the whole tree below the root (skipping `.git` and, unless
    /// `show_hidden`, dotfiles). Symlinked directories are listed but not
    /// descended into, which keeps the walk finite.
    fn scan_all(&self, params: TreeParams) -> (Vec<ScanEntry>, bool) {
        let mut entries = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(read_dir) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                if entries.len() >= FILTER_SCAN_LIMIT {
                    return (entries, true);
                }
                let path = entry.path();
                let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                if (!params.show_hidden && name.starts_with('.')) || name == ".git" {
                    continue;
                }
                let file_type = entry.file_type().ok();
                let real_dir = file_type.is_some_and(|ft| ft.is_dir());
                let is_dir = real_dir
                    || file_type.is_some_and(|ft| ft.is_symlink())
                        && fs::metadata(&path).is_ok_and(|m| m.is_dir());
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if real_dir {
                    stack.push(path.clone());
                }
                entries.push(ScanEntry {
                    path,
                    name,
                    relative,
                    is_dir,
                });
            }
        }
        (entries, false)
    }

    /// Rebuilds `filter_dirs` for the active filter: every matching entry plus
    /// all of its ancestors, grouped under their parents and sorted like a
    /// regular listing. Directories collapsed in the previous filtered view
    /// stay collapsed.
    fn apply_filter(&mut self, params: TreeParams) {
        let Some(filter) = self.filter.clone() else {
            return;
        };
        if self.scan.is_none() {
            let (entries, truncated) = self.scan_all(params);
            self.scan = Some(entries);
            self.scan_truncated = truncated;
        }
        let scan = self.scan.as_ref().expect("scan was just filled");

        let index: HashMap<&Path, usize> = scan
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.path.as_path(), i))
            .collect();
        let mut shown: HashSet<usize> = HashSet::new();
        for (i, entry) in scan.iter().enumerate() {
            if !filter.matches(&entry.name, &entry.relative) {
                continue;
            }
            // Mark the entry and every ancestor; stop at the first one that is
            // already marked since its own ancestors are then marked too.
            let mut cur = Some(i);
            while let Some(c) = cur {
                if !shown.insert(c) {
                    break;
                }
                cur = scan[c]
                    .path
                    .parent()
                    .and_then(|parent| index.get(parent).copied());
            }
        }

        let mut grouped: HashMap<PathBuf, Vec<Child>> = HashMap::new();
        for &i in &shown {
            let entry = &scan[i];
            if let Some(parent) = entry.path.parent() {
                grouped
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(Child {
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        sort_key: entry.name.to_lowercase(),
                        is_dir: entry.is_dir,
                    });
            }
        }
        // The root row exists even when nothing matched.
        grouped.entry(self.root.clone()).or_default();

        let previous = std::mem::take(&mut self.filter_dirs);
        for (path, mut children) in grouped {
            if params.directories_first {
                children.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.sort_key.cmp(&b.sort_key))
                });
            } else {
                children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
            }
            let expanded = previous.get(&path).is_none_or(|dir| dir.expanded);
            self.filter_dirs.insert(path, Dir { expanded, children });
        }
    }

    /// Expands every ancestor of `path` so it becomes visible. Returns `true`
    /// if `path` shows up in the tree afterwards.
    ///
    /// Paths are compared lexically, not canonically: the tree stores entry
    /// paths in the same spelling as the root it was opened with (a root
    /// reached through a symlink must not collide with the canonical
    /// spelling of the same directory).
    pub fn reveal(&mut self, path: &Path, params: TreeParams) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }

        // A filtered view may not contain the target; reveal works on the
        // whole tree.
        if self.filter.is_some() {
            self.set_filter("", params);
        }

        // The root anchors every other expansion; make sure it is expanded.
        if !self.is_expanded(&self.root) {
            let root = self.root.clone();
            if self.expand(&root, params).is_err() {
                return false;
            }
        }

        // Collect the ancestor chain nearest-to-target-first, then expand
        // top-down. The walk always shortens the path, so it terminates on
        // its own.
        let mut ancestors: Vec<PathBuf> = Vec::with_capacity(16);
        let mut cur: PathBuf = path.to_path_buf();
        while cur != self.root {
            let parent = match cur.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return false,
            };
            cur = parent.clone();
            if cur != self.root {
                ancestors.push(parent);
            }
        }
        ancestors.reverse();

        for ancestor in ancestors.iter() {
            if self.is_expanded(ancestor) {
                continue;
            }
            if self.expand(ancestor, params).is_err() {
                self.rebuild_visible();
                return false;
            }
        }
        self.rebuild_visible();
        self.visible.iter().any(|entry| entry.path == path)
    }

    /// Rebuilds `visible` from the expanded directories.
    fn rebuild_visible(&mut self) {
        let dirs = if self.filter.is_some() {
            &self.filter_dirs
        } else {
            &self.dirs
        };
        self.visible = build_visible(&self.root, dirs);
    }
}

/// Builds the flattened visible rows: root row first, then a depth-first
/// pre-order walk over expanded directories. Pure over `dirs`, so borrows
/// end when it returns.
fn build_visible(root: &Path, dirs: &HashMap<PathBuf, Dir>) -> Vec<Entry> {
    let mut visible = Vec::new();
    let Some(root_dir) = dirs.get(root) else {
        return visible;
    };

    visible.push(Entry {
        path: root.to_path_buf(),
        name: root_display_name(root),
        depth: 0,
        is_dir: true,
        expanded: root_dir.expanded,
        is_last: true,
    });
    if !root_dir.expanded {
        return visible;
    }

    // Depth-first pre-order with a stack of child iterators, held as
    // (next_index, slice, depth) frames so children can be pushed without
    // holding a mutable borrow across the iteration. Deep trees cannot blow
    // the stack.
    let mut stack: Vec<(usize, &[Child], u16)> = vec![(0, root_dir.children.as_slice(), 1u16)];
    while let Some((idx, children, depth)) = stack.last().map(|f| (f.0, f.1, f.2)) {
        match children.get(idx) {
            Some(child) => {
                stack.last_mut().expect("frame just peeked").0 += 1;
                visible.push(Entry {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    is_dir: child.is_dir,
                    expanded: dirs.get(&child.path).is_some_and(|d| d.expanded),
                    is_last: false,
                });
                if child.is_dir {
                    if let Some(dir) = dirs.get(&child.path) {
                        if dir.expanded && !dir.children.is_empty() {
                            stack.push((0, dir.children.as_slice(), depth + 1));
                        }
                    }
                }
            }
            None => {
                stack.pop();
            }
        }
    }

    // Patch `is_last`: a row is last when it is the final entry of its
    // parent directory's cached child list.
    for entry in visible.iter_mut().skip(1) {
        if let Some(parent) = entry.path.parent() {
            if let Some(dir) = dirs.get(parent) {
                entry.is_last = dir
                    .children
                    .last()
                    .is_some_and(|child| child.path == entry.path);
            }
        }
    }
    visible
}

fn root_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Recursively copies `from` (file or directory) onto `to` (entries are
/// replaced as they are copied). Used for paste.
pub fn copy_recursively(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let dest = to.join(entry.file_name());
            copy_recursively(&entry.path(), &dest)?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn params(show_hidden: bool, directories_first: bool) -> TreeParams {
        TreeParams {
            show_hidden,
            directories_first,
        }
    }

    fn names(tree: &Tree) -> Vec<String> {
        tree.visible().iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn visible_order_and_lazy_expansion() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("b_dir")).unwrap();
        std::fs::create_dir(root.join("a_dir")).unwrap();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::write(root.join("m.txt"), "m").unwrap();
        std::fs::create_dir(root.join("a_dir").join("nested")).unwrap();
        std::fs::write(root.join("a_dir").join("inner.txt"), "i").unwrap();
        std::fs::create_dir(root.join(".hidden_dir")).unwrap();
        std::fs::write(root.join(".dotfile"), "d").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        // Root row, then directories first (sorted), then files (sorted).
        // Hidden entries are excluded.
        assert_eq!(
            names(&tree),
            vec![
                root.file_name().unwrap().to_string_lossy().into_owned(),
                "a_dir".into(),
                "b_dir".into(),
                "m.txt".into(),
                "z.txt".into(),
            ]
        );
        assert!(!tree.visible()[3].is_last); // m.txt
        assert!(tree.visible()[4].is_last); // z.txt is the last row

        // expand: adds "nested" (dir first) then "inner.txt" at depth 2
        tree.expand_at(&root.join("a_dir"), p).unwrap();
        let vis = tree.visible();
        assert_eq!(vis.len(), 7);
        assert_eq!(vis[2].name, "nested");
        assert_eq!(vis[2].depth, 2);
        assert!(!vis[2].is_last);
        assert_eq!(vis[3].name, "inner.txt");
        assert_eq!(vis[3].depth, 2);
        assert!(vis[3].is_last);

        // collapse removes the subtree from view but keeps data.
        tree.collapse(&root.join("a_dir"));
        assert_eq!(tree.visible().len(), 5);

        toggling_hidden_works(&mut tree, root);
    }

    fn toggling_hidden_works(tree: &mut Tree, root: &Path) {
        tree.toggle(root, params(true, true)).unwrap(); // collapse root
        tree.toggle(root, params(true, true)).unwrap(); // re-expand, now showing hidden
        assert!(names(tree).iter().any(|n| n == ".dotfile"));
        assert!(names(tree).iter().any(|n| n == ".hidden_dir"));
        // With mt_sort... directories still first
        let first_dirs: Vec<bool> = tree
            .visible()
            .iter()
            .skip(1)
            .map_while(|e| Some(e.is_dir))
            .take(3)
            .collect();
        assert_eq!(first_dirs, vec![true, true, true]);
    }

    #[test]
    fn reveal_expands_ancestors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        let file = root.join("sub/deep/file.txt");
        std::fs::write(&file, "f").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        assert!(tree.reveal(&file, p));
        assert!(tree.visible().iter().any(|e| e.path == file));
        assert!(tree.is_expanded(&root.join("sub")));
        assert!(tree.is_expanded(&root.join("sub/deep")));

        // Outside the root cannot be revealed.
        assert!(!tree.reveal(tmp.path().parent().unwrap(), p));

        // The root itself reveals fine.
        assert!(tree.reveal(root, p));
        assert!(tree.is_expanded(root));

        // Missing files reveal nothing (target must exist).
        assert!(!tree.reveal(&root.join("nope.txt"), p));
    }

    #[test]
    fn refresh_prunes_and_adds() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::create_dir(root.join("gone")).unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();
        assert!(names(&tree).iter().any(|n| n == "gone"));

        std::fs::write(root.join("new.txt"), "n").unwrap();
        std::fs::remove_file(root.join("z.txt")).unwrap();
        std::fs::remove_dir(root.join("gone")).unwrap();

        assert_eq!(tree.refresh(p), 0);
        let n = names(&tree);
        assert!(n.iter().any(|n| n == "new.txt"));
        assert!(!n.iter().any(|n| n == "z.txt"));
        assert!(!n.iter().any(|n| n == "gone"));
    }

    #[test]
    fn missing_root_errors() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("no_such_dir");
        assert!(Tree::new(missing, params(false, true)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reveal_works_through_symlinked_root() {
        // The root is reached through a symlink (as happens when TMPDIR or
        // /var on macOS resolves through one). Reveal must operate in the
        // root's own spelling, not the canonical one.
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        std::fs::create_dir_all(link.join("sub/deep")).unwrap();
        let file = link.join("sub/deep/file.txt");
        std::fs::write(&file, "f").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(link.clone(), p).unwrap();
        assert!(tree.reveal(&file, p));
        assert!(tree.visible().iter().any(|e| e.path == file));
        assert!(tree.is_expanded(&link.join("sub")));
        assert!(tree.is_expanded(&link.join("sub/deep")));

        // The root itself still reveals.
        assert!(tree.reveal(&link, p));
    }

    #[test]
    fn recursive_copy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/data")).unwrap();
        std::fs::write(root.join("src/data/a.txt"), "a").unwrap();
        std::fs::write(root.join("src/top.txt"), "t").unwrap();

        copy_recursively(&root.join("src"), &root.join("dst")).unwrap();
        assert!(root.join("dst/data/a.txt").is_file());
        assert!(root.join("dst/top.txt").is_file());

        // Files copy through too.
        copy_recursively(&root.join("src/top.txt"), &root.join("top2.txt")).unwrap();
        assert!(root.join("top2.txt").is_file());
    }

    // Symlink classification uses `fs::metadata`, which is not available on
    // all targets' std APIs in the same shape; keep the test unix-only.
    #[cfg(unix)]
    #[test]
    fn symlinked_dirs_render_as_directories() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("real_dir")).unwrap();
        std::fs::write(root.join("real_dir/file.txt"), "f").unwrap();
        std::fs::write(root.join("loose.txt"), "l").unwrap();
        symlink(root.join("real_dir"), root.join("linked_dir")).unwrap();
        // A link whose target does not exist is kind of a file row: it can
        // not be expanded like a directory.
        symlink(root.join("missing_target"), root.join("dangling")).unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        let linked = tree
            .visible()
            .iter()
            .find(|e| e.name == "linked_dir")
            .expect("linked_dir row exists")
            .clone();
        assert!(linked.is_dir, "symlink to a directory is a dir row");
        let dangling = tree
            .visible()
            .iter()
            .find(|e| e.name == "dangling")
            .expect("dangling row exists")
            .clone();
        assert!(!dangling.is_dir, "dangling link is a file-like row");

        // Expanding the linked directory lists the target's children.
        tree.expand_at(&root.join("linked_dir"), p).unwrap();
        assert!(tree
            .visible()
            .iter()
            .any(|e| e.name == "file.txt" && e.depth == 2));
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.RS", "main.rs"), "case-insensitive");
        assert!(!glob_match("*.rs", "main.rs.bak"));
        assert!(glob_match("test_*", "test_foo"));
        assert!(!glob_match("test_*", "a_test_foo"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        // Character classes.
        assert!(glob_match("[abc].txt", "b.txt"));
        assert!(!glob_match("[abc].txt", "d.txt"));
        assert!(glob_match("file[0-9]", "file7"));
        assert!(!glob_match("file[0-9]", "filex"));
        assert!(glob_match("[!a]x", "bx"));
        assert!(!glob_match("[!a]x", "ax"));
        // An unclosed class is literal.
        assert!(glob_match("a[b", "a[b"));
    }

    #[test]
    fn filter_patterns() {
        assert!(Filter::new("").is_none());
        assert!(Filter::new("   ").is_none());

        // No wildcards: substring.
        let f = Filter::new("main").unwrap();
        assert!(f.matches("main.rs", "src/main.rs"));
        assert!(f.matches("domain.txt", "domain.txt"));
        assert!(!f.matches("lib.rs", "src/lib.rs"));

        // Wildcards: whole-name match, with brace alternatives.
        let f = Filter::new("*.{rs,toml}").unwrap();
        assert!(f.matches("a.rs", "a.rs"));
        assert!(f.matches("Cargo.toml", "Cargo.toml"));
        assert!(!f.matches("a.md", "a.md"));

        // A slash matches against the path below the root.
        let f = Filter::new("src/*.rs").unwrap();
        assert!(f.matches("main.rs", "src/main.rs"));
        assert!(!f.matches("main.rs", "tests/main.rs"));
    }

    fn filter_fixture() -> (TempDir, Tree, TreeParams) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/deep/inner.rs"), "").unwrap();
        std::fs::write(root.join("src/notes.md"), "").unwrap();
        std::fs::write(root.join("docs/guide.md"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join(".hidden.rs"), "").unwrap();
        let p = params(false, true);
        let tree = Tree::new(root.to_path_buf(), p).unwrap();
        (tmp, tree, p)
    }

    #[test]
    fn filter_shows_matches_anywhere_with_their_ancestors() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        // Nothing is expanded yet, yet the filter reaches into directories.
        tree.set_filter("*.rs", p);
        let shown: Vec<String> = names(&tree).into_iter().skip(1).collect();
        assert_eq!(shown, vec!["src", "deep", "inner.rs", "main.rs"]);

        let inner = tree
            .visible()
            .iter()
            .find(|e| e.name == "inner.rs")
            .unwrap();
        assert_eq!(inner.depth, 3);
        assert!(inner.is_last, "last among the matches of deep/");
        let main = tree.visible().iter().find(|e| e.name == "main.rs").unwrap();
        assert!(
            main.is_last,
            "notes.md is filtered out, so main.rs is last in src/"
        );
        assert!(tree.is_last_child(&root.join("src/main.rs")));

        // Hidden files stay hidden, like in the regular tree.
        assert!(!names(&tree).iter().any(|n| n == ".hidden.rs"));
    }

    #[test]
    fn filter_with_no_matches_leaves_the_root_row() {
        let (tmp, mut tree, p) = filter_fixture();
        tree.set_filter("*.nothing", p);
        assert_eq!(tree.visible().len(), 1);
        assert_eq!(tree.visible()[0].path, tmp.path());
    }

    #[test]
    fn clearing_the_filter_restores_the_full_tree() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.expand_at(&root.join("docs"), p).unwrap();
        let before = names(&tree);

        tree.set_filter("inner", p);
        assert_ne!(names(&tree), before);
        tree.set_filter("", p);
        assert!(tree.filter().is_none());
        // Same rows, same expansion state as before filtering.
        assert_eq!(names(&tree), before);
        assert!(tree.is_expanded(&root.join("docs")));
        assert!(!tree.is_expanded(&root.join("src")));
    }

    #[test]
    fn filtered_directories_collapse_and_expand() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        assert!(tree.is_expanded(&root.join("src")));

        tree.collapse(&root.join("src"));
        assert_eq!(names(&tree).len(), 2); // root + collapsed src
        tree.toggle(&root.join("src"), p).unwrap();
        assert_eq!(names(&tree).len(), 5);

        // A directory that matched by name only can still be browsed: its
        // children are listed unfiltered.
        tree.set_filter("docs", p);
        assert_eq!(names(&tree).len(), 2);
        tree.toggle(&root.join("docs"), p).unwrap();
        assert!(names(&tree).iter().any(|n| n == "guide.md"));
    }

    #[test]
    fn refresh_keeps_the_filter_and_sees_new_files() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        std::fs::write(root.join("src/extra.rs"), "").unwrap();
        std::fs::remove_file(root.join("src/main.rs")).unwrap();
        assert_eq!(tree.refresh(p), 0);
        let shown = names(&tree);
        assert!(shown.iter().any(|n| n == "extra.rs"));
        assert!(!shown.iter().any(|n| n == "main.rs"));
        assert!(tree.filter().is_some());
    }

    #[test]
    fn reveal_drops_a_filter_that_hides_the_target() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        assert!(tree.reveal(&root.join("docs/guide.md"), p));
        assert!(tree.filter().is_none());
        assert!(tree.visible().iter().any(|e| e.name == "guide.md"));
    }

    #[test]
    fn hidden_files_can_be_filtered_when_shown() {
        let (tmp, mut tree, _) = filter_fixture();
        let p = params(true, true);
        tree.refresh(p);
        tree.set_filter("*.rs", p);
        assert!(names(&tree).iter().any(|n| n == ".hidden.rs"));
        drop(tmp);
    }

    #[test]
    fn floating_is_independent_of_the_pinned_side() {
        let config = ExplorerConfig::default();
        let mut state = ExplorerState {
            side: Some(ExplorerSide::Right),
            ..ExplorerState::default()
        };
        assert_eq!(state.side(&config), ExplorerSide::Right);

        state.visible = true;
        state.focused = true;
        state.floating = true;
        assert_eq!(state.side(&config), ExplorerSide::Floating);

        // Closing the float leaves the pinned side as it was.
        state.hide();
        assert!(!state.visible && !state.focused && !state.floating);
        assert_eq!(state.side(&config), ExplorerSide::Right);
    }
}
