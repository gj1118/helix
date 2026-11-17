//! The undo tree (`:undo-tree`, `Space u`): every revision of the buffer's history, branches
//! included, in a picker (or the quickfix panel, when it is on). The rows are in the order of
//! the tree, a branch left behind indented under the revision it started from; the preview
//! shows the buffer's text at the revision under the cursor, at what it changed. `Enter`
//! turns the buffer into that revision.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use helix_core::history::RevisionSummary;
use helix_view::{DocumentId, Editor, ViewId};

use crate::ui::{picker::TextPreview, Picker, PickerColumn};

/// The glyph of the revision shown in the buffer, and of the others.
const SHOWN: &str = "●";
const OTHER: &str = "○";

/// A row of the undo tree.
pub struct Revision {
    pub doc_id: DocumentId,
    pub view_id: ViewId,
    pub revision: usize,
    /// The drawing in front of it (`├─`, `│ `) and its glyph.
    tree: String,
    /// `#3 +1 line`, `original`; `saved` after the one last written.
    label: String,
    /// The start of what it inserted (or deleted).
    excerpt: String,
    age: String,
    /// The lines it changed in its text (0-based, inclusive), known once it was previewed.
    lines: OnceLock<Option<(usize, usize)>>,
}

/// The undo tree of the buffer `doc_id` in `view_id`, with the cursor on the revision the
/// buffer shows; `None` when either is gone.
pub fn picker(
    editor: &mut Editor,
    doc_id: DocumentId,
    view_id: ViewId,
) -> Option<Picker<Revision, ()>> {
    if !editor.tree.contains(view_id) {
        return None;
    }
    let view = editor.tree.get_mut(view_id);
    let doc = editor.documents.get_mut(&doc_id)?;
    // the edit just made is a revision of its own
    doc.append_changes_to_history(view);
    let saved = doc.get_last_saved_revision();
    let history = doc.history.take();
    let shown = history.current_revision();
    let count = history.revision_count();
    let now = Instant::now();
    let rows: Vec<Revision> = history
        .tree_rows()
        .into_iter()
        .map(|row| {
            let glyph = if row.revision == shown { SHOWN } else { OTHER };
            let summary = history.summary(row.revision);
            let mut label = if row.revision == 0 {
                "original".to_string()
            } else {
                format!("#{} {}", row.revision, change(&summary))
            };
            if row.revision == saved {
                label.push_str(" saved");
            }
            Revision {
                doc_id,
                view_id,
                revision: row.revision,
                tree: format!("{}{glyph}", row.prefix),
                label,
                excerpt: summary.excerpt,
                age: history
                    .timestamp(row.revision)
                    .map(|time| age(now.saturating_duration_since(time)))
                    .unwrap_or_default(),
                lines: OnceLock::new(),
            }
        })
        .collect();
    doc.history.set(history);
    let cursor = rows
        .iter()
        .position(|row| row.revision == shown)
        .unwrap_or(0) as u32;

    let columns = [
        PickerColumn::new("tree", |row: &Revision, _| row.tree.as_str().into()).without_filtering(),
        PickerColumn::new("revision", |row: &Revision, _| row.label.as_str().into()),
        PickerColumn::new("text", |row: &Revision, _| row.excerpt.as_str().into()),
        PickerColumn::new("age", |row: &Revision, _| row.age.as_str().into()).without_filtering(),
    ];
    let picker = Picker::new(columns, 1, rows, (), |cx, row: &Revision, _action| {
        show(cx.editor, row.doc_id, row.view_id, row.revision);
    })
    .with_cursor(cursor)
    .with_title(format!("Undo tree · {count} revisions"))
    .with_text_preview(
        |_, row: &Revision| {
            Some((
                format!("undo/{:?}/{}", row.doc_id, row.revision),
                row.lines.get().copied().flatten(),
            ))
        },
        |editor, row: &Revision| {
            let (text, lines) = revision_text(editor, row.doc_id, row.revision)?;
            let _ = row.lines.set(lines);
            let doc = editor.document(row.doc_id)?;
            Some(TextPreview {
                text: text.to_string(),
                language: doc.language_name().map(String::from),
            })
        },
    );
    Some(picker)
}

/// The buffer's text at `revision`, and the lines that revision changed in it.
pub fn revision_text(
    editor: &Editor,
    doc_id: DocumentId,
    revision: usize,
) -> Option<(helix_core::Rope, Option<(usize, usize)>)> {
    let doc = editor.document(doc_id)?;
    let history = doc.history.take();
    let mut copy = history.clone();
    doc.history.set(history);
    let mut text = doc.text().clone();
    for transaction in copy.jump_to_revision(revision) {
        transaction.apply(&mut text);
    }
    let lines = copy.changed_range(revision).map(|(from, to)| {
        let slice = text.slice(..);
        let from = from.min(slice.len_chars());
        let last = if to > from { to - 1 } else { from };
        (
            slice.char_to_line(from),
            slice.char_to_line(last.min(slice.len_chars())),
        )
    });
    Some((text, lines))
}

/// Turns the buffer into `revision`, with its cursor in view. `false` when the buffer or its
/// window is gone.
fn show(editor: &mut Editor, doc_id: DocumentId, view_id: ViewId, revision: usize) -> bool {
    let scrolloff = editor.config().scrolloff;
    if !editor.tree.contains(view_id) {
        return false;
    }
    let view = editor.tree.get_mut(view_id);
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return false;
    };
    doc.jump_to_revision(view, revision);
    view.ensure_cursor_in_view(doc, scrolloff);
    true
}

/// `3s`, `5m`, `2h`, `4d`: how long ago.
fn age(since: Duration) -> String {
    let secs = since.as_secs();
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// `+3 −1 lines`, `+12 −4`: what a revision changed, short.
fn change(summary: &RevisionSummary) -> String {
    let parts = |plus: usize, minus: usize| -> String {
        let mut parts = Vec::new();
        if plus > 0 {
            parts.push(format!("+{plus}"));
        }
        if minus > 0 {
            parts.push(format!("−{minus}"));
        }
        parts.join(" ")
    };
    if summary.inserted_lines + summary.deleted_lines > 0 {
        let lines = summary.inserted_lines + summary.deleted_lines;
        let unit = if lines == 1 { "line" } else { "lines" };
        format!(
            "{} {unit}",
            parts(summary.inserted_lines, summary.deleted_lines)
        )
    } else {
        parts(summary.inserted_chars, summary.deleted_chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_short() {
        let ages: Vec<String> = [0, 4, 5, 59, 60, 3599, 3600, 86_400 * 3]
            .into_iter()
            .map(|secs| age(Duration::from_secs(secs)))
            .collect();
        assert_eq!(ages, ["now", "now", "5s", "59s", "1m", "59m", "1h", "3d"]);
    }

    #[test]
    fn changes_are_counted_in_lines_when_lines_changed() {
        let summary =
            |inserted_chars, deleted_chars, inserted_lines, deleted_lines| RevisionSummary {
                inserted_chars,
                deleted_chars,
                inserted_lines,
                deleted_lines,
                excerpt: String::new(),
            };
        assert_eq!(change(&summary(5, 2, 0, 0)), "+5 −2".to_string());
        assert_eq!(change(&summary(5, 0, 0, 0)), "+5".to_string());
        assert_eq!(change(&summary(40, 3, 3, 0)), "+3 lines".to_string());
        assert_eq!(change(&summary(4, 10, 0, 1)), "−1 line".to_string());
        assert_eq!(change(&summary(0, 0, 0, 0)), String::new());
    }
}
