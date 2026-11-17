use helix_core::Selection;
use helix_loader::{
    clipboard_file, command_histfile, file_histfile,
    persistence::{
        push_history, read_history, read_state_file, trim_history, write_history, write_state_file,
    },
    search_histfile,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::view::ViewPosition;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileHistoryEntry {
    pub path: PathBuf,
    pub view_position: ViewPosition,
    pub selection: Selection,
}

impl FileHistoryEntry {
    pub fn new(path: PathBuf, view_position: ViewPosition, selection: Selection) -> Self {
        Self {
            path,
            view_position,
            selection,
        }
    }
}

pub fn push_file_history(entry: &FileHistoryEntry) {
    push_history(file_histfile(), entry)
}

pub fn read_file_history() -> Vec<FileHistoryEntry> {
    read_history(&file_histfile())
}

pub fn trim_file_history(limit: usize) {
    trim_history::<FileHistoryEntry>(file_histfile(), limit)
}

pub fn push_reg_history(register: char, line: &String) {
    let filepath = match register {
        ':' => command_histfile(),
        '/' => search_histfile(),
        _ => return,
    };

    push_history(filepath, line)
}

fn read_reg_history(filepath: PathBuf) -> Vec<String> {
    read_history(&filepath)
}

pub fn read_command_history() -> Vec<String> {
    let mut hist = read_reg_history(command_histfile());
    hist.reverse();
    hist
}

pub fn trim_command_history(limit: usize) {
    trim_history::<String>(command_histfile(), limit)
}

pub fn read_search_history() -> Vec<String> {
    let mut hist = read_reg_history(search_histfile());
    hist.reverse();
    hist
}

pub fn trim_search_history(limit: usize) {
    trim_history::<String>(search_histfile(), limit)
}

pub fn write_clipboard_file(values: &[String]) {
    write_history(clipboard_file(), values)
}

pub fn read_clipboard_file() -> Vec<String> {
    read_history(&clipboard_file())
}

/// What is written next to a file's undo history: what the file held when it was saved, so
/// the history is only used for that text.
#[derive(Debug, Serialize, Deserialize)]
struct UndoFile {
    version: u32,
    /// The saved file's length in chars and a hash of its text.
    len_chars: usize,
    hash: u64,
    history: helix_core::history::SavedHistory,
}

const UNDO_FILE_VERSION: u32 = 1;

/// FNV-1a: a hash that stays the same across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: impl IntoIterator<Item = u8>, mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// The hash of a text, as the undo files keep it.
pub fn text_hash(text: &helix_core::Rope) -> u64 {
    text.chunks()
        .fold(FNV_OFFSET, |hash, chunk| fnv1a(chunk.bytes(), hash))
}

/// The folder undo histories are kept in: `dir` when set (`undo-dir`), else `undo` in the
/// state folder.
pub fn undo_dir(dir: Option<&std::path::Path>) -> PathBuf {
    dir.map(helix_stdx::path::expand_tilde)
        .map(|dir| dir.into_owned())
        .unwrap_or_else(|| helix_loader::state_dir().join("undo"))
}

/// The undo file of `path` in `dir`: named after a hash of the path, so files of the same
/// name in different folders have their own.
pub fn undo_file(dir: &std::path::Path, path: &std::path::Path) -> PathBuf {
    let hash = fnv1a(
        path.as_os_str().as_encoded_bytes().iter().copied(),
        FNV_OFFSET,
    );
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.join(format!("{name}.{hash:016x}.undo"))
}

/// Writes the undo history of `path`, saved with the text `text`.
pub fn write_undo(
    dir: &std::path::Path,
    path: &std::path::Path,
    text: &helix_core::Rope,
    history: helix_core::history::SavedHistory,
) -> std::io::Result<()> {
    let file = UndoFile {
        version: UNDO_FILE_VERSION,
        len_chars: text.len_chars(),
        hash: text_hash(text),
        history,
    };
    write_state_file(&undo_file(dir, path), &file)
}

/// The undo history of `path`, when it was saved with the text it holds now (`text`).
pub fn read_undo(
    dir: &std::path::Path,
    path: &std::path::Path,
    text: &helix_core::Rope,
) -> Option<helix_core::history::SavedHistory> {
    let file: UndoFile = read_state_file(&undo_file(dir, path))?;
    (file.version == UNDO_FILE_VERSION
        && file.len_chars == text.len_chars()
        && file.hash == text_hash(text))
    .then_some(file.history)
}

#[cfg(test)]
mod undo_test {
    use super::*;
    use helix_core::{history::State, Rope, Selection, Transaction};

    #[test]
    fn an_undo_file_is_read_back_only_for_the_same_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project").join("main.rs");
        let mut history = helix_core::history::History::default();
        let mut doc = Rope::from("fn main() {}\n");
        let state = State {
            doc: doc.clone(),
            selection: Selection::point(0),
        };
        let transaction =
            Transaction::change(&doc, std::iter::once((0, 0, Some("// hi\n".into()))));
        history.commit_revision(&transaction, &state);
        assert!(transaction.apply(&mut doc));

        write_undo(dir.path(), &path, &doc, history.to_saved()).unwrap();
        let read = read_undo(dir.path(), &path, &doc);
        assert_eq!(read.map(|saved| saved.len()), Some(2_usize));

        // the file was changed by something else: the history doesn't fit
        let other = Rope::from("fn main() { }\n");
        assert_eq!(read_undo(dir.path(), &path, &other), None);
        // another file of the same name has its own
        assert_eq!(
            read_undo(dir.path(), &dir.path().join("main.rs"), &doc),
            None
        );
        assert_ne!(
            undo_file(dir.path(), &path),
            undo_file(dir.path(), &dir.path().join("main.rs"))
        );
    }

    #[test]
    fn the_text_hash_does_not_depend_on_chunks() {
        let small = Rope::from("abc");
        let mut built = Rope::from("a");
        built.insert(1, "bc");
        assert_eq!(text_hash(&small), text_hash(&built));
        assert_ne!(text_hash(&small), text_hash(&Rope::from("abd")));
        let hash: u64 = text_hash(&Rope::from(""));
        assert_eq!(hash, FNV_OFFSET);
    }
}
