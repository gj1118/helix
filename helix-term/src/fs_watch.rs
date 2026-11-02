//! A small recursive file system watcher used to keep pickers up to date.
//!
//! A [`FsWatch`] counts the changes it sees in a directory tree. Pickers compare
//! the counter between redraws to find out whether their results are stale. The
//! watcher stops when the value is dropped.
//!
//! Directories are watched one by one (not recursively) from a background thread
//! that walks the tree like the file picker does: ignored directories (`.gitignore`,
//! `.ignore`), `.git` (except its top level) and noise such as `target` are skipped,
//! and at most [`MAX_WATCHED_DIRS`] directories are watched. Opening a picker thus
//! never waits for the walk, and huge trees don't use up the inotify watches.

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// The most directories one [`FsWatch`] watches.
const MAX_WATCHED_DIRS: usize = 4096;

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
                // New directories are watched too. The watcher can't be called from
                // its own event handler, so the background thread adds them.
                if matches!(event.kind, EventKind::Create(_)) {
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
        .unwrap()
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

    #[test]
    fn detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        let Some(watch) = FsWatch::new(dir.path()) else {
            return; // no watcher support in this environment
        };
        let before = watch.generation();
        // The watches are added in the background: write until a change is seen.
        for i in 0..100 {
            std::fs::write(dir.path().join("a.txt"), i.to_string()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            if watch.generation() != before {
                return;
            }
        }
        panic!("no change detected");
    }

    fn wait_for(watch: &FsWatch, mut change: impl FnMut(usize)) -> bool {
        let before = watch.generation();
        for i in 0..40 {
            change(i);
            std::thread::sleep(std::time::Duration::from_millis(50));
            if watch.generation() != before {
                return true;
            }
        }
        false
    }

    #[test]
    fn watches_subdirectories_but_not_ignored_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("ignored/deep")).unwrap();
        std::fs::write(root.join(".ignore"), "ignored/\n").unwrap();
        let Some(watch) = FsWatch::new(root) else {
            return; // no watcher support in this environment
        };
        assert!(wait_for(&watch, |i| {
            std::fs::write(root.join("src/deep/a.txt"), i.to_string()).unwrap()
        }));
        assert!(!wait_for(&watch, |i| {
            std::fs::write(root.join("ignored/deep/a.txt"), i.to_string()).unwrap()
        }));
        // Directories created later are watched too.
        std::fs::create_dir(root.join("src/new")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(wait_for(&watch, |i| {
            std::fs::write(root.join("src/new/a.txt"), i.to_string()).unwrap()
        }));
    }
}
