//! File tree panel ("explorer"):
//! lazy-loaded directory tree model, configuration, and runtime state.
//!
//! Disk reads happen when a directory is first expanded (`Tree::expand_at`)
//! and in `Tree::refresh`; rendering only touches already materialized
//! state, so opening the explorer is O(1) in project size and every frame
//! renders straight out of cached rows.

use std::collections::HashMap;
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
    /// Runtime side override (`:explorer-left` / `:explorer-right` /
    /// `:explorer-float`).
    pub side: Option<ExplorerSide>,
    /// Runtime width override (`:explorer-width`).
    pub width: Option<u16>,
    /// Runtime show-hidden override (`h`/'H' inside the panel).
    pub show_hidden: Option<bool>,
}

impl ExplorerState {
    /// Effective side of the panel.
    pub fn side(&self, config: &ExplorerConfig) -> ExplorerSide {
        self.side.unwrap_or(config.side)
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

/// Lazily loaded directory tree.
#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    dirs: HashMap<PathBuf, Dir>,
    visible: Vec<Entry>,
}

impl Tree {
    /// Creates a tree rooted at `root`, loading the root's children.
    pub fn new(root: PathBuf, params: TreeParams) -> io::Result<Self> {
        let mut tree = Self {
            root,
            dirs: HashMap::new(),
            visible: Vec::new(),
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

    /// Whether the directory at `path` is expanded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.dirs.get(path).is_some_and(|dir| dir.expanded)
    }

    /// Whether `path` is the trailing entry among its parent directory's
    /// cached children. Drives the `│` vs blank guide lines in the renderer.
    pub fn is_last_child(&self, path: &Path) -> bool {
        path.parent()
            .and_then(|parent| self.dirs.get(parent))
            .is_some_and(|dir| dir.children.last().is_some_and(|child| child.path == path))
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
        if self.dirs.get(path).is_some_and(|dir| dir.expanded) {
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
        if let Some(dir) = self.dirs.get_mut(path) {
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
        self.rebuild_visible();
        errors
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
        self.visible = build_visible(&self.root, &self.dirs);
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
}
