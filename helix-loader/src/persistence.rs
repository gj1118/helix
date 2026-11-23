use bincode::{serialize_into, Options};
use log::warn;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

fn bincode_io_error(err: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

fn open_state_file(path: &Path, append: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).append(append);
    if !append {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub fn write_history<T: Serialize>(filepath: PathBuf, entries: &[T]) {
    let result = open_state_file(&filepath, false).and_then(|file| {
        let mut writer = BufWriter::new(file);
        for entry in entries {
            serialize_into(&mut writer, entry).map_err(bincode_io_error)?;
        }
        writer.flush()
    });
    if let Err(err) = result {
        warn!(
            "Failed to write persistent state file {}: {err}",
            filepath.display()
        );
    }
}

pub fn push_history<T: Serialize>(filepath: PathBuf, entry: &T) {
    let mut encoded = Vec::new();
    let result = serialize_into(&mut encoded, entry)
        .map_err(bincode_io_error)
        .and_then(|()| {
            open_state_file(&filepath, true).and_then(|mut file| file.write_all(&encoded))
        });
    if let Err(err) = result {
        warn!(
            "Failed to update persistent state file {}: {err}",
            filepath.display()
        );
    }
}

pub fn read_history<T: for<'a> Deserialize<'a>>(filepath: &Path) -> Vec<T> {
    read_history_impl(filepath).0
}

/// Reads the entries of a state file, and whether all of it could be read.
fn read_history_impl<T: for<'a> Deserialize<'a>>(filepath: &Path) -> (Vec<T>, bool) {
    let file = match File::open(filepath) {
        Ok(file) => file,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                warn!(
                    "Failed to read persistent state file {}: {err}",
                    filepath.display()
                );
            }
            return (Vec::new(), true);
        }
    };
    // The format `serialize_into` writes (fixed-size integers, little endian), with
    // a size limit: a corrupt length prefix would otherwise make bincode allocate a
    // buffer of that length before reading it. No entry is larger than the file.
    let limit = file.metadata().map_or(u64::MAX, |metadata| metadata.len());
    let options = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .with_limit(limit);
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut complete = true;
    loop {
        match reader.fill_buf() {
            Ok([]) => break,
            Ok(_) => match options.deserialize_from(&mut reader) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    warn!(
                        "Ignoring corrupt trailing data in persistent state file {}: {err}",
                        filepath.display()
                    );
                    complete = false;
                    break;
                }
            },
            Err(err) => {
                warn!(
                    "Failed to read persistent state file {}: {err}",
                    filepath.display()
                );
                complete = false;
                break;
            }
        }
    }
    (entries, complete)
}

pub fn trim_history<T: Clone + Serialize + for<'a> Deserialize<'a>>(
    filepath: PathBuf,
    limit: usize,
) {
    let (history, complete): (Vec<T>, _) = read_history_impl(&filepath);
    // A file with corrupt data is rewritten too: entries appended after the corrupt
    // part could never be read.
    if history.len() > limit || !complete {
        let trim_start = history.len().saturating_sub(limit);
        write_history(filepath, &history[trim_start..]);
    }
}

/// [`trim_history`] for a history with one entry that counts for each `key` (each file's last
/// place, say): past `limit` entries, the file is rewritten with the last entry of each of the
/// last `limit` keys.
pub fn trim_history_by<T, K>(filepath: PathBuf, limit: usize, key: impl Fn(&T) -> K)
where
    T: Clone + Serialize + for<'a> Deserialize<'a>,
    K: Eq + std::hash::Hash,
{
    let (history, complete): (Vec<T>, _) = read_history_impl(&filepath);
    if history.len() <= limit && complete {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<T> = history
        .into_iter()
        .rev()
        .filter(|entry| seen.insert(key(entry)))
        .take(limit)
        .collect();
    kept.reverse();
    write_history(filepath, &kept);
}

/// Writes `value` alone to `filepath`, creating its folder: to a temporary file next to it
/// first, then moved over it, so a crash halfway leaves the old file (or none), never half a
/// file.
pub fn write_state_file<T: Serialize>(filepath: &Path, value: &T) -> io::Result<()> {
    let dir = filepath
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut encoded = Vec::new();
    serialize_into(&mut encoded, value).map_err(bincode_io_error)?;
    let file_name = filepath
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let mut temporary = file_name.to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = dir.join(temporary);
    let written = open_state_file(&temporary, false).and_then(|mut file| {
        file.write_all(&encoded)?;
        file.flush()
    });
    match written.and_then(|()| std::fs::rename(&temporary, filepath)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&temporary);
            Err(err)
        }
    }
}

/// Reads what [`write_state_file`] wrote; `None` when the file is missing or can't be read
/// (corrupt, another format).
pub fn read_state_file<T: for<'a> Deserialize<'a>>(filepath: &Path) -> Option<T> {
    let (mut entries, complete) = read_history_impl::<T>(filepath);
    (complete && entries.len() == 1).then(|| entries.remove(0))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn a_state_file_round_trips_and_a_bad_one_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("value");
        write_state_file(&path, &(7u32, "seven".to_string())).unwrap();
        let read: Option<(u32, String)> = read_state_file(&path);
        assert_eq!(read, Some((7u32, "seven".to_string())));
        // replaced whole, and no temporary file left behind
        write_state_file(&path, &(8u32, "eight".to_string())).unwrap();
        assert_eq!(
            read_state_file::<(u32, String)>(&path),
            Some((8u32, "eight".to_string()))
        );
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("value")]);

        std::fs::write(&path, b"\xff\xff").unwrap();
        assert_eq!(read_state_file::<(u32, String)>(&path), None);
        assert_eq!(read_state_file::<u32>(&dir.path().join("missing")), None);
    }

    #[test]
    fn history_round_trips_and_survives_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");

        write_history(path.clone(), &["a".to_string(), "bc".to_string()]);
        push_history(path.clone(), &"def".to_string());
        assert_eq!(read_history::<String>(&path), ["a", "bc", "def"]);

        // A length prefix far beyond the file size (as from a corrupt file or another
        // format) must not be allocated; the entries before it are kept.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
        bytes.extend_from_slice(b"xyz");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(read_history::<String>(&path), ["a", "bc", "def"]);

        // trimming drops the corrupt data, so later entries can be read again
        trim_history::<String>(path.clone(), 2);
        push_history(path.clone(), &"g".to_string());
        assert_eq!(read_history::<String>(&path), ["bc", "def", "g"]);

        assert!(read_history::<String>(&dir.path().join("missing")).is_empty());
    }

    /// Past the limit, a history of places keeps the last place of each of the last files;
    /// within it, the file is left as it is (repeated files and all).
    #[test]
    fn a_history_trimmed_by_key_keeps_the_last_entry_of_the_last_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("places");
        let place = |file: &str, line: u32| (file.to_string(), line);
        let places = [
            place("a", 1),
            place("b", 2),
            place("a", 3),
            place("c", 4),
            place("b", 5),
        ];
        write_history(path.clone(), &places);
        let by_file = |entry: &(String, u32)| entry.0.clone();

        trim_history_by(path.clone(), 5, by_file);
        let read: Vec<(String, u32)> = read_history(&path);
        assert_eq!(read, places.to_vec(), "five entries are within the limit");

        trim_history_by(path.clone(), 2, by_file);
        let read: Vec<(String, u32)> = read_history(&path);
        assert_eq!(read, vec![place("c", 4), place("b", 5)]);

        write_history(path.clone(), &places);
        trim_history_by(path.clone(), 4, by_file);
        let read: Vec<(String, u32)> = read_history(&path);
        assert_eq!(
            read,
            vec![place("a", 3), place("c", 4), place("b", 5)],
            "each file once, its last place"
        );
    }
}
