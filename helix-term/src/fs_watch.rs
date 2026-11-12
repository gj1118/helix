//! A small recursive file system watcher used to keep pickers up to date.
//!
//! A [`FsWatch`] counts the changes it sees in a directory tree. Pickers compare
//! the counter between redraws to find out whether their results are stale. The
//! watcher stops when the value is dropped.
//!
//! The watches are set up on a background thread, so opening a picker never waits
//! for them. How depends on the platform (see [`Strategy`]):
//!
//! - Where the OS watches a tree natively (FSEvents on macOS, ReadDirectoryChangesW
//!   on Windows), the root is watched recursively in one call, which doesn't walk
//!   the tree. Changes in noise directories are filtered out as they arrive.
//! - Elsewhere (inotify on Linux, kqueue on the BSDs) a recursive watch walks the
//!   whole tree and adds a watch per directory, ignored ones included. So the tree
//!   is walked like the file picker does: ignored directories (`.gitignore`,
//!   `.ignore`), `.git` (except its top level) and noise such as `target` are
//!   skipped, and at most [`MAX_WATCHED_DIRS`] directories are watched.

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
};

use notify::{
    event::ModifyKind, Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

/// The most directories one [`FsWatch`] watches with [`Strategy::PerDirectory`].
const MAX_WATCHED_DIRS: usize = 4096;

/// How a tree is watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    /// One recursive watch on the root. Right where the OS watches trees natively:
    /// there, every non-recursive `watch` call is expensive (on macOS notify
    /// restarts the FSEvents stream and loses the changes made meanwhile; on Windows
    /// it starts a thread per directory).
    Recursive,
    /// A non-recursive watch per directory that isn't ignored. Right where watching
    /// recursively means a watch per directory anyway (inotify, kqueue).
    PerDirectory,
}

impl Strategy {
    fn for_platform() -> Self {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            Strategy::Recursive
        } else {
            Strategy::PerDirectory
        }
    }
}

pub struct FsWatch {
    generation: Arc<AtomicU64>,
    /// Shared with the background thread, which only holds a weak reference: dropping
    /// this stops the watcher and ends the thread.
    _watcher: Arc<Mutex<RecommendedWatcher>>,
    /// The first events received, relevant or not, and the errors: what a test that saw no
    /// change reports.
    #[cfg(test)]
    received: Arc<Mutex<Vec<String>>>,
}

/// The watched tree: what noise is judged against.
#[derive(Debug, Clone)]
struct Tree {
    root: PathBuf,
    /// The real path of `root`, when it differs: FSEvents reports real paths.
    real_root: Option<PathBuf>,
    /// The repository's git directory when it is outside the tree (a linked worktree, a
    /// submodule): a `.git` file points at it.
    external_git_dir: Option<PathBuf>,
}

impl Tree {
    fn new(root: &Path) -> Self {
        let real_root = root.canonicalize().ok().filter(|real| real != root);
        let external_git_dir = git_dir(root).filter(|dir| {
            !dir.starts_with(root) && real_root.as_ref().is_none_or(|real| !dir.starts_with(real))
        });
        Self {
            root: root.to_path_buf(),
            real_root,
            external_git_dir,
        }
    }

    /// `path` below the root. Directories above the root don't count: a project inside a
    /// folder called `target` is not noise.
    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.root)
            .ok()
            .or_else(|| {
                let real = self.real_root.as_ref()?;
                path.strip_prefix(real).ok()
            })
            .unwrap_or(path)
    }

    fn is_noise(&self, path: &Path) -> bool {
        if let Some(git_dir) = &self.external_git_dir {
            if path.starts_with(git_dir) {
                return !matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("index" | "HEAD")
                );
            }
        }
        is_noise(self.relative(path))
    }

    fn is_relevant(&self, event: &Event) -> bool {
        // Events were dropped (FSEvents under load sends a rescan instead of them): anything
        // under its path may have changed.
        if event.need_rescan() {
            return event.paths.is_empty() || event.paths.iter().any(|path| !self.is_noise(path));
        }
        matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) | EventKind::Any
        ) && event.paths.iter().any(|path| !self.is_noise(path))
    }
}

/// The git directory of the repository whose work tree is `root`: `root/.git`, or where the
/// `.git` file of a linked worktree or a submodule points.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let metadata = std::fs::metadata(&dot_git).ok()?;
    if metadata.is_dir() {
        return Some(dot_git);
    }
    // A `.git` file is a single short line; don't read anything big.
    if metadata.len() > 4096 {
        return None;
    }
    let content = std::fs::read_to_string(&dot_git).ok()?;
    let dir = content.lines().next()?.strip_prefix("gitdir:")?.trim();
    // Usually relative (`../.git/worktrees/name`): resolved, as the paths of its events are.
    let dir = root.join(dir);
    Some(
        dir.canonicalize()
            .unwrap_or_else(|_| helix_stdx::path::normalize(&dir)),
    )
}

/// Directories whose contents change constantly and never show up in pickers. `path` is
/// relative to the watched root (see [`Tree::relative`]).
fn is_noise(path: &Path) -> bool {
    let mut components = path.components().rev();
    // Inside `.git` only `index` and `HEAD` matter (staging, commits, checkouts).
    let mut in_git = false;
    let mut last = true;
    for component in &mut components {
        if let Component::Normal(name) = component {
            if name == "target" || name == "node_modules" {
                return true;
            }
            if name == ".git" {
                in_git = true;
                break;
            }
            if last && (name.to_string_lossy().ends_with(".lock")) {
                return true;
            }
        }
        last = false;
    }
    if !in_git {
        return false;
    }
    !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("index" | "HEAD")
    )
}

impl FsWatch {
    /// Starts watching `root` recursively. Returns `None` when the platform
    /// watcher cannot be created (for example when inotify limits are reached),
    /// in which case callers simply do not refresh on file system changes.
    ///
    /// The directories are added from a background thread, so changes made right
    /// after this returns may go unnoticed.
    pub fn new(root: &Path) -> Option<Self> {
        Self::with_strategy(root, Strategy::for_platform())
    }

    fn with_strategy(root: &Path, strategy: Strategy) -> Option<Self> {
        let generation = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let received = Arc::new(Mutex::new(Vec::new()));
        #[cfg(test)]
        let events_received = received.clone();
        let counter = generation.clone();
        let (new_dirs, new_dirs_rx) = mpsc::channel::<PathBuf>();
        let tree = Arc::new(Tree::new(root));
        let events_tree = tree.clone();
        let watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                #[cfg(test)]
                {
                    let mut received = events_received.lock().unwrap_or_else(|e| e.into_inner());
                    if received.len() < 20 {
                        received.push(format!("{event:?}"));
                    }
                }
                let Ok(event) = event else { return };
                let tree = &events_tree;
                if !tree.is_relevant(&event) {
                    return;
                }
                counter.fetch_add(1, Ordering::Relaxed);
                helix_event::request_redraw();
                // New directories are watched too, also ones moved into the tree
                // (a rename, not a create). The watcher can't be called from its own
                // event handler, so the background thread adds them; it skips paths
                // that are not directories (e.g. the old side of a rename).
                if strategy == Strategy::PerDirectory
                    && matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
                    )
                {
                    for path in event.paths {
                        if !tree.is_noise(&path) {
                            let _ = new_dirs.send(path);
                        }
                    }
                }
            },
            Config::default(),
        )
        .map_err(|err| log::debug!("file watcher unavailable: {err}"))
        .ok()?;
        let watcher = Arc::new(Mutex::new(watcher));

        let weak = Arc::downgrade(&watcher);
        std::thread::Builder::new()
            .name("fs-watch".into())
            .spawn(move || {
                let root = &tree.root;
                if strategy == Strategy::Recursive {
                    // One call; the OS covers new and moved directories itself.
                    if let Some(watcher) = weak.upgrade() {
                        let mut watcher = watcher.lock().unwrap_or_else(|e| e.into_inner());
                        if let Err(err) = watcher.watch(root, RecursiveMode::Recursive) {
                            log::debug!("could not watch {}: {err}", root.display());
                        }
                        // The `index` and `HEAD` of a worktree or a submodule live outside it.
                        if let Some(git_dir) = &tree.external_git_dir {
                            if let Err(err) = watcher.watch(git_dir, RecursiveMode::NonRecursive) {
                                log::debug!("could not watch {}: {err}", git_dir.display());
                            }
                        }
                    }
                    return;
                }
                let mut watched = 0;
                // The top level of the git directory holds `index` and `HEAD`.
                if let Some(git_dir) = git_dir(root) {
                    if git_dir.is_dir() && !watch_dir(&weak, &git_dir, &mut watched) {
                        return;
                    }
                }
                if !watch_tree(&weak, &tree, root, &mut watched) {
                    return;
                }
                // Runs until the watcher, and with it the sender, is dropped.
                while let Ok(path) = new_dirs_rx.recv() {
                    if path.is_dir()
                        && !is_ignored(&path)
                        && !watch_tree(&weak, &tree, &path, &mut watched)
                    {
                        return;
                    }
                }
            })
            .map_err(|err| log::debug!("could not start the file watcher thread: {err}"))
            .ok()?;

        Some(Self {
            generation,
            _watcher: watcher,
            #[cfg(test)]
            received,
        })
    }

    /// Like [`FsWatch::new`], but refuses home directories and the file system root,
    /// where watching recursively is too expensive to be worth it.
    pub fn workspace(root: &Path) -> Option<Self> {
        let absolute = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if absolute.parent().is_none()
            || helix_stdx::path::home_dir().is_ok_and(|home| home == absolute)
        {
            return None;
        }
        Self::new(root)
    }

    /// Number of relevant changes seen so far.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Watches `dir` (not recursively). Returns `false` when the watcher is gone or
/// [`MAX_WATCHED_DIRS`] is reached, to stop the walk.
fn watch_dir(watcher: &Weak<Mutex<RecommendedWatcher>>, dir: &Path, watched: &mut usize) -> bool {
    if *watched >= MAX_WATCHED_DIRS {
        log::debug!("file watcher: not watching more than {MAX_WATCHED_DIRS} directories");
        return false;
    }
    let Some(watcher) = watcher.upgrade() else {
        return false;
    };
    let result = watcher
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .watch(dir, RecursiveMode::NonRecursive);
    match result {
        Ok(()) => *watched += 1,
        Err(err) => log::debug!("could not watch {}: {err}", dir.display()),
    }
    true
}

/// Whether the directory `dir`, created after the tree was walked, is one the walk skips
/// (`.gitignore`, `.ignore`): a walk from `dir` itself would not know, it always lists its
/// start. So `dir` is looked for in a listing of its parent.
fn is_ignored(dir: &Path) -> bool {
    let Some(parent) = dir.parent() else {
        return false;
    };
    let wanted = dir.to_path_buf();
    !ignore::WalkBuilder::new(parent)
        .hidden(false)
        .follow_links(false)
        .max_depth(Some(1))
        .filter_entry(move |entry| entry.depth() == 0 || entry.path() == wanted)
        .build()
        .flatten()
        .any(|entry| entry.depth() == 1)
}

/// Watches `dir` and the directories under it that are not ignored. Returns `false`
/// when the walk was stopped, see [`watch_dir`].
fn watch_tree(
    watcher: &Weak<Mutex<RecommendedWatcher>>,
    tree: &Arc<Tree>,
    dir: &Path,
    watched: &mut usize,
) -> bool {
    let tree = tree.clone();
    let walk = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            entry.depth() == 0
                || entry.file_type().is_some_and(|kind| kind.is_dir())
                    && entry.file_name() != ".git"
                    && !tree.is_noise(entry.path())
        })
        .build();
    for entry in walk.flatten() {
        if entry.file_type().is_some_and(|kind| kind.is_dir())
            && !watch_dir(watcher, entry.path(), watched)
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter() {
        assert!(is_noise(Path::new("/repo/target/debug/foo")));
        assert!(is_noise(Path::new("/repo/node_modules/a/b.js")));
        assert!(is_noise(Path::new("/repo/.git/objects/ab/cdef")));
        assert!(is_noise(Path::new("/repo/.git/index.lock")));
        assert!(!is_noise(Path::new("/repo/.git/index")));
        assert!(!is_noise(Path::new("/repo/.git/HEAD")));
        assert!(!is_noise(Path::new("/repo/src/main.rs")));
    }

    /// Only what is below the root decides: a project in a folder called `target` (or in
    /// `node_modules`) is watched like any other.
    #[test]
    fn folders_above_the_root_are_not_noise() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("target/node_modules/project");
        std::fs::create_dir_all(&root).unwrap();
        let tree = Tree::new(&root);
        assert!(!tree.is_noise(&root.join("src/main.rs")));
        assert!(tree.is_noise(&root.join("target/debug/foo")));
        // FSEvents reports real paths
        let real = root.canonicalize().unwrap();
        assert!(!tree.is_noise(&real.join("src/main.rs")));
        assert!(tree.is_noise(&real.join(".git/objects/ab/cdef")));
    }

    /// A linked worktree (or a submodule) has a `.git` file pointing at its git directory,
    /// whose `index` and `HEAD` matter and nothing else does.
    #[test]
    fn the_git_directory_of_a_worktree_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let worktree_git = dir.path().join("main/.git/worktrees/wt");
        std::fs::create_dir_all(&worktree_git).unwrap();
        let root = dir.path().join("wt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();

        assert_eq!(git_dir(&root), Some(worktree_git.canonicalize().unwrap()));
        let tree = Tree::new(&root);
        let external = tree.external_git_dir.clone().expect("an external git dir");
        assert!(!tree.is_noise(&external.join("index")));
        assert!(!tree.is_noise(&external.join("HEAD")));
        assert!(tree.is_noise(&external.join("ORIG_HEAD")));
        assert!(tree.is_noise(&external.join("logs/HEAD.lock")));

        // An ordinary repository's git directory is inside the tree.
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(plain.join(".git")).unwrap();
        assert_eq!(git_dir(&plain), Some(plain.join(".git")));
        assert_eq!(Tree::new(&plain).external_git_dir, None);
        assert_eq!(git_dir(dir.path()), None);
    }

    /// Directories created after the walk are looked up in the ignore files too.
    #[test]
    fn new_ignored_directories_are_recognized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".ignore"), "build/\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::create_dir_all(root.join("src/build")).unwrap();
        std::fs::create_dir_all(root.join("src/lib")).unwrap();
        assert!(is_ignored(&root.join("build")));
        assert!(is_ignored(&root.join("src/build")));
        assert!(!is_ignored(&root.join("src")));
        assert!(!is_ignored(&root.join("src/lib")));
    }

    /// A rescan (FSEvents dropped events) counts as a change, unless all its paths are noise;
    /// other events need a create, remove or modify kind.
    #[test]
    fn rescans_count_as_changes() {
        use notify::event::{Flag, ModifyKind};
        let tree = Tree {
            root: PathBuf::from("/project"),
            real_root: None,
            external_git_dir: None,
        };
        let rescan = |paths: &[&str]| {
            paths.iter().fold(
                Event::new(EventKind::Other).set_flag(Flag::Rescan),
                |event, path| event.add_path(PathBuf::from(path)),
            )
        };
        assert!(tree.is_relevant(&rescan(&["/project"])));
        assert!(tree.is_relevant(&rescan(&["/project/src/deep"])));
        assert!(tree.is_relevant(&rescan(&[])));
        assert!(!tree.is_relevant(&rescan(&["/project/target/debug"])));
        // without the flag, `Other` is not a change
        let other = Event::new(EventKind::Other).add_path(PathBuf::from("/project/a.txt"));
        assert!(!tree.is_relevant(&other));
        let modify = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(PathBuf::from("/project/src/a.txt"));
        assert!(tree.is_relevant(&modify));
    }

    /// The tests that start watchers run one at a time: on macOS each starts an FSEvents
    /// stream, and several at once in one process delay one another's events.
    fn one_watcher_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    #[ignore = "starts an OS file watcher, whose events come late or not at all on a busy machine (FSEvents after a build): run with --ignored"]
    fn detects_changes() {
        let _one = one_watcher_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let Some(watch) = FsWatch::new(dir.path()) else {
            return; // no watcher support in this environment
        };
        // The watches are added in the background: write until a change is seen.
        let seen = wait_for(&watch, |i| {
            std::fs::write(dir.path().join("a.txt"), i.to_string()).unwrap()
        });
        assert!(seen, "{}", no_change(&watch, dir.path()));
    }

    /// How long to keep making a change for the watcher to see it. FSEvents delivers events
    /// late, and much later while the system is busy writing elsewhere (right after a build,
    /// as `cargo test` runs): give it longer.
    const WAIT_ROUNDS: usize = if cfg!(target_os = "macos") { 600 } else { 200 };

    /// The failure message of a test whose change the watcher of `root` didn't see: what it
    /// received instead.
    fn no_change(watch: &FsWatch, root: &Path) -> String {
        let received = watch.received.lock().unwrap_or_else(|e| e.into_inner());
        format!(
            "no change detected in {} s under {} (real path {:?}); events received: {:#?}",
            WAIT_ROUNDS / 20,
            root.display(),
            root.canonicalize().ok(),
            *received
        )
    }

    /// Repeats `change` until the watcher sees it, for up to [`WAIT_ROUNDS`] / 20 s (FSEvents
    /// delivers events with some delay, and the watches are set up in the background).
    fn wait_for(watch: &FsWatch, mut change: impl FnMut(usize)) -> bool {
        let before = watch.generation();
        for i in 0..WAIT_ROUNDS {
            change(i);
            std::thread::sleep(std::time::Duration::from_millis(50));
            if watch.generation() != before {
                return true;
            }
        }
        false
    }

    /// Nested and newly created directories are watched with either strategy; with
    /// [`Strategy::PerDirectory`] ignored directories are not.
    fn watches_subdirectories(strategy: Strategy) {
        let _one = one_watcher_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("ignored/deep")).unwrap();
        std::fs::write(root.join(".ignore"), "ignored/\n").unwrap();
        let Some(watch) = FsWatch::with_strategy(root, strategy) else {
            return; // no watcher support in this environment
        };
        let seen = wait_for(&watch, |i| {
            std::fs::write(root.join("src/deep/a.txt"), i.to_string()).unwrap()
        });
        assert!(seen, "{}", no_change(&watch, root));
        if strategy == Strategy::PerDirectory {
            // inotify reports at once, so a second without an event is enough here
            let before = watch.generation();
            for i in 0..20 {
                std::fs::write(root.join("ignored/deep/a.txt"), i.to_string()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert_eq!(
                watch.generation(),
                before,
                "a change in an ignored directory"
            );
        }
        // Directories created later are watched too.
        std::fs::create_dir(root.join("src/new")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let seen = wait_for(&watch, |i| {
            std::fs::write(root.join("src/new/a.txt"), i.to_string()).unwrap()
        });
        assert!(seen, "{}", no_change(&watch, root));
    }

    #[test]
    fn platforms_with_native_tree_watching_watch_recursively() {
        let native = cfg!(any(target_os = "macos", target_os = "windows"));
        let want = if native {
            Strategy::Recursive
        } else {
            Strategy::PerDirectory
        };
        assert_eq!(Strategy::for_platform(), want);
    }

    #[test]
    #[ignore = "starts an OS file watcher, whose events come late or not at all on a busy machine (FSEvents after a build): run with --ignored"]
    fn watches_subdirectories_recursively() {
        watches_subdirectories(Strategy::Recursive);
    }

    // kqueue's per-directory watching is not exercised on macOS: it isn't used there.
    #[test]
    #[ignore = "starts an OS file watcher, whose events come late or not at all on a busy machine (FSEvents after a build): run with --ignored"]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn watches_subdirectories_one_by_one() {
        watches_subdirectories(Strategy::PerDirectory);
    }
}
