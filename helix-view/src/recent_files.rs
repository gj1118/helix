//! The recently used files (`[editor.recent-picker]`): the files opened and saved, most recent
//! first, kept in a file of the cache directory shared by all the editors running, for the
//! recent file picker.
//!
//! Nothing here touches the disk on the editor's thread: [`RecentFiles::record`] notes the
//! file in memory and a background task writes the notes (all those taken in the meantime, in
//! one go), merging them with what the file has (other editors write it too) and replacing it
//! atomically. [`RecentFiles::flush`] writes what is left on exit.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// The files to note, and whether a background task is writing them.
#[derive(Debug, Default)]
struct Pending {
    /// Least recent first.
    files: Vec<PathBuf>,
    writing: bool,
}

/// See the module's documentation.
#[derive(Debug, Clone, Default)]
pub struct RecentFiles {
    pending: Arc<Mutex<Pending>>,
    /// Held while the file is read and written, so that one editor's writes never interleave.
    file_lock: Arc<Mutex<()>>,
}

impl RecentFiles {
    /// Notes `path` as the most recent file, written to `recents_path` (keeping the `max`
    /// most recent files) by a background task. Without a Tokio runtime (in unit tests), it
    /// is written right away.
    pub fn record(&self, path: &Path, recents_path: &Path, max: usize) {
        let spawn = {
            let mut pending = self.pending.lock().unwrap();
            pending.files.retain(|file| file != path);
            pending.files.push(path.to_path_buf());
            !std::mem::replace(&mut pending.writing, true)
        };
        if !spawn {
            // the task writing will write this one too
            return;
        }
        let this = self.clone();
        let recents_path = recents_path.to_path_buf();
        let write_pending = move || this.write_pending(&recents_path, max);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(write_pending);
            }
            Err(_) => write_pending(),
        }
    }

    /// Writes the notes until there are none left; ends the writing.
    fn write_pending(&self, recents_path: &Path, max: usize) {
        loop {
            // The notes are taken with the file locked: notes taken later are written later.
            let _lock = self.file_lock.lock().unwrap();
            let files = {
                let mut pending = self.pending.lock().unwrap();
                if pending.files.is_empty() {
                    pending.writing = false;
                    return;
                }
                std::mem::take(&mut pending.files)
            };
            write(&files, recents_path, max);
        }
    }

    /// Writes what is still noted, on this thread (on exit), after what a background task
    /// is writing.
    pub fn flush(&self, recents_path: &Path, max: usize) {
        let _lock = self.file_lock.lock().unwrap();
        let files = std::mem::take(&mut self.pending.lock().unwrap().files);
        if !files.is_empty() {
            write(&files, recents_path, max);
        }
    }

    /// The recent files, most recent first: those noted and not yet written, then those of
    /// `recents_path` (a file that does not exist has none). Reads the disk: call it off the
    /// editor's thread.
    pub fn list(&self, recents_path: &Path, max: usize) -> Vec<PathBuf> {
        let noted = self.pending.lock().unwrap().files.clone();
        let written = read(recents_path).unwrap_or_else(|err| {
            log::warn!(
                "Failed to read the recent files from {}: {err}",
                recents_path.display()
            );
            Vec::new()
        });
        merge(&noted, written, max)
    }
}

/// `files` (least recent first) over what `recents_path` has, the `max` most recent kept,
/// written atomically (a file next to it, renamed over it); a failure is logged. Call it
/// with the file locked.
fn write(files: &[PathBuf], recents_path: &Path, max: usize) {
    let result = (|| -> io::Result<()> {
        let merged = merge(files, read(recents_path)?, max);
        if let Some(dir) = recents_path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut contents = String::new();
        for file in &merged {
            // A path with a line break cannot be a line of the file.
            if let Some(file) = file.to_str().filter(|file| !file.contains(['\n', '\r'])) {
                contents.push_str(file);
                contents.push('\n');
            }
        }
        let temporary = recents_path.with_extension(format!("tmp-{}", std::process::id()));
        // Nothing left behind when it fails (a full disk, say).
        fs::write(&temporary, contents)
            .and_then(|()| fs::rename(&temporary, recents_path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })
    })();
    if let Err(err) = result {
        log::warn!(
            "Failed to write the recent files to {}: {err}",
            recents_path.display()
        );
    }
}

/// The paths of the file `path`, one per line, most recent first; none when it does not exist.
fn read(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// `newer` (least recent first) before `older` (most recent first), each file once, the `max`
/// most recent.
fn merge(newer: &[PathBuf], older: Vec<PathBuf>, max: usize) -> Vec<PathBuf> {
    let mut merged: Vec<PathBuf> = Vec::with_capacity(newer.len() + older.len());
    for file in newer.iter().rev().cloned().chain(older) {
        if !merged.contains(&file) {
            merged.push(file);
        }
        if merged.len() == max {
            break;
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn merging_puts_the_newest_first_once_and_keeps_the_most_recent() {
        let merged: Vec<PathBuf> = merge(&paths(&["/c", "/a"]), paths(&["/b", "/a", "/d"]), 10);
        assert_eq!(merged, paths(&["/a", "/c", "/b", "/d"]));
        let merged = merge(&paths(&["/c", "/a"]), paths(&["/b", "/a", "/d"]), 3);
        assert_eq!(merged, paths(&["/a", "/c", "/b"]));
        assert_eq!(merge(&[], Vec::new(), 5), Vec::<PathBuf>::new());
    }

    #[test]
    fn recording_writes_the_file_most_recent_first() {
        let dir = tempfile::tempdir().unwrap();
        let recents = dir.path().join("sub").join("recents.txt");
        let files = RecentFiles::default();
        // no runtime: written right away
        files.record(Path::new("/one"), &recents, 3);
        files.record(Path::new("/two"), &recents, 3);
        files.record(Path::new("/one"), &recents, 3);
        files.record(Path::new("/three"), &recents, 3);
        files.record(Path::new("/four"), &recents, 3);
        let written: String = fs::read_to_string(&recents).unwrap();
        assert_eq!(written, "/four\n/three\n/one\n");
        assert_eq!(files.list(&recents, 2), paths(&["/four", "/three"]));
        // nothing left behind
        let entries: Vec<_> = fs::read_dir(recents.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn another_editors_files_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let recents = dir.path().join("recents.txt");
        fs::write(&recents, "/theirs\n/shared\n").unwrap();
        let files = RecentFiles::default();
        files.record(Path::new("/shared"), &recents, 10);
        files.record(Path::new("/mine"), &recents, 10);
        let written: String = fs::read_to_string(&recents).unwrap();
        assert_eq!(written, "/mine\n/shared\n/theirs\n");
    }

    #[test]
    fn a_missing_file_has_no_recent_files_and_bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let recents = dir.path().join("recents.txt");
        let files = RecentFiles::default();
        assert_eq!(files.list(&recents, 10), Vec::<PathBuf>::new());
        files.record(Path::new("/a\nb"), &recents, 10);
        files.record(Path::new("/ok"), &recents, 10);
        let written: String = fs::read_to_string(&recents).unwrap();
        assert_eq!(written, "/ok\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_a_runtime_the_file_is_written_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let recents = dir.path().join("recents.txt");
        let files = RecentFiles::default();
        for n in 0..50 {
            files.record(Path::new(&format!("/file{n}")), &recents, 100);
        }
        // whatever is still noted is written on exit (after the task writing, if any)
        files.flush(&recents, 100);
        let written: Vec<String> = fs::read_to_string(&recents)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        assert_eq!(written.len(), 50);
        assert_eq!(written[0], "/file49");
        assert_eq!(written[49], "/file0");
    }
}
