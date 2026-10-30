//! A small recursive file system watcher used to keep pickers up to date.
//!
//! A [`FsWatch`] counts the changes it sees in a directory tree. Pickers compare
//! the counter between redraws to find out whether their results are stale. The
//! watcher stops when the value is dropped.

use std::{
    path::{Component, Path},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct FsWatch {
    generation: Arc<AtomicU64>,
    _watcher: RecommendedWatcher,
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
    pub fn new(root: &Path) -> Option<Self> {
        let generation = Arc::new(AtomicU64::new(0));
        let counter = generation.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                if event.is_ok_and(|event| is_relevant(&event)) {
                    counter.fetch_add(1, Ordering::Relaxed);
                    helix_event::request_redraw();
                }
            },
            Config::default(),
        )
        .map_err(|err| log::debug!("file watcher unavailable: {err}"))
        .ok()?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|err| log::debug!("could not watch {}: {err}", root.display()))
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
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        for _ in 0..50 {
            if watch.generation() != before {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("no change detected");
    }
}
