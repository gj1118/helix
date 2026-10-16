use bincode::{deserialize_from, serialize_into};
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
    let file = match File::open(filepath) {
        Ok(file) => file,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                warn!(
                    "Failed to read persistent state file {}: {err}",
                    filepath.display()
                );
            }
            return Vec::new();
        }
    };
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    loop {
        match reader.fill_buf() {
            Ok([]) => break,
            Ok(_) => match deserialize_from(&mut reader) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    warn!(
                        "Ignoring corrupt trailing data in persistent state file {}: {err}",
                        filepath.display()
                    );
                    break;
                }
            },
            Err(err) => {
                warn!(
                    "Failed to read persistent state file {}: {err}",
                    filepath.display()
                );
                break;
            }
        }
    }
    entries
}

pub fn trim_history<T: Clone + Serialize + for<'a> Deserialize<'a>>(
    filepath: PathBuf,
    limit: usize,
) {
    let history: Vec<T> = read_history(&filepath);
    if history.len() > limit {
        let trim_start = history.len() - limit;
        let trimmed_history = history[trim_start..].to_vec();
        write_history(filepath, &trimmed_history);
    }
}
