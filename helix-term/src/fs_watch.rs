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
}

/// Directories whose contents change constantly and never show up in pickers.
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

fn is_relevant(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
    ) && event.paths.iter().any(|path| !is_noise(path))
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
        let counter = generation.clone();
        let (new_dirs, new_dirs_rx) = mpsc::channel::<PathBuf>();
        let watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                let Ok(event) = event else { return };
                if !is_relevant(&event) {
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
                        if !is_noise(&path) {
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

        let (weak, root) = (Arc::downgrade(&watcher), root.to_path_buf());
        std::thread::Builder::new()
            .name("fs-watch".into())
            .spawn(move || {
                if strategy == Strategy::Recursive {
                    // One call; the OS covers new and moved directories itself.
                    if let Some(watcher) = weak.upgrade() {
                        let result = watcher
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .watch(&root, RecursiveMode::Recursive);
                        if let Err(err) = result {
                            log::debug!("could not watch {}: {err}", root.display());
                        }
                    }
                    return;
                }
                let mut watched = 0;
                // The top level of `.git` holds `index` and `HEAD`.
                let git = root.join(".git");
                if git.is_dir() && !watch_dir(&weak, &git, &mut watched) {
                    return;
                }
                if !watch_tree(&weak, &root, &mut watched) {
                    return;
                }
                // Runs until the watcher, and with it the sender, is dropped.
                while let Ok(path) = new_dirs_rx.recv() {
                    if path.is_dir() && !watch_tree(&weak, &path, &mut watched) {
                        return;
                    }
                }
            })
            .map_err(|err| log::debug!("could not start the file watcher thread: {err}"))
            .ok()?;

        Some(Self {
            generation,
            _watcher: watcher,
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

/// Watches `root` and the directories under it that are not ignored. Returns `false`
/// when the walk was stopped, see [`watch_dir`].
fn watch_tree(watcher: &Weak<Mutex<RecommendedWatcher>>, root: &Path, watched: &mut usize) -> bool {
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry.file_type().is_some_and(|kind| kind.is_dir())
                    && entry.file_name() != ".git"
                    && !is_noise(entry.path())
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

    /// The tests that start watchers run one at a time: on macOS each starts an FSEvents
    /// stream, and several at once in one process delay one another's events.
    fn one_watcher_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn detects_changes() {
        let _one = one_watcher_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let Some(watch) = FsWatch::new(dir.path()) else {
            return; // no watcher support in this environment
        };
        let before = watch.generation();
        // The watches are added in the background: write until a change is seen.
        for i in 0..200 {
            std::fs::write(dir.path().join("a.txt"), i.to_string()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            if watch.generation() != before {
                return;
            }
        }
        panic!("no change detected");
    }

    /// Repeats `change` until the watcher sees it, for up to 10 s (FSEvents delivers
    /// events with some delay, and the watches are set up in the background).
    fn wait_for(watch: &FsWatch, mut change: impl FnMut(usize)) -> bool {
        let before = watch.generation();
        for i in 0..200 {
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
        assert!(wait_for(&watch, |i| {
            std::fs::write(root.join("src/deep/a.txt"), i.to_string()).unwrap()
        }));
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
        assert!(wait_for(&watch, |i| {
            std::fs::write(root.join("src/new/a.txt"), i.to_string()).unwrap()
        }));
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
    fn watches_subdirectories_recursively() {
        watches_subdirectories(Strategy::Recursive);
    }

    // kqueue's per-directory watching is not exercised on macOS: it isn't used there.
    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn watches_subdirectories_one_by_one() {
        watches_subdirectories(Strategy::PerDirectory);
    }
}
