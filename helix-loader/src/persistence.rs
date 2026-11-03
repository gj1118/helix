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

#[cfg(test)]
mod test {
    use super::*;

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
}
