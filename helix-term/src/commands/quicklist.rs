//! The quicklist commands: collecting locations (from the cursor, or from a picker with
//! `Ctrl-q`), walking them and the quicklist picker. See `helix_view::quicklist` for the list
//! itself.

use std::path::PathBuf;

use anyhow::{bail, Context as _};
use helix_core::{hashmap, line_ending::line_end_char_index, Range, Selection};
use helix_view::{
    align_view,
    editor::Action,
    quicklist::{Entry, Position, Target},
    Align, DocumentId, Editor,
};

use super::{push_jump, Context};
use crate::{
    ctrl,
    ui::{self, overlay::overlaid, picker::PathOrId, Picker, PickerColumn},
};

/// How much of a line is kept as an entry's text.
const MAX_TEXT_CHARS: usize = 200;

fn line_text(text: helix_core::RopeSlice, line: usize) -> String {
    if line >= text.len_lines() {
        return String::new();
    }
    text.line(line)
        .to_string()
        .trim()
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect()
}

/// The target that stands for the document `id`: its file if it has one.
fn target_of(editor: &Editor, id: DocumentId) -> Target {
    match editor.documents.get(&id).and_then(|doc| doc.path()) {
        Some(path) => Target::Path(path.to_owned()),
        None => Target::Doc(id),
    }
}

/// An entry for the cursor and selections of the focused view.
fn current_entry(editor: &Editor) -> Entry {
    let (view, doc) = helix_view::current_ref!(editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone();
    let line = selection.primary().cursor_line(text);
    Entry::new(
        target_of(editor, doc.id()),
        Position::Selection(selection),
        line,
        line_text(text, line),
    )
}

/// Entries for the (matched) results of a picker. `locations` yields where each result is.
pub fn entries_from_locations<'a>(
    editor: &Editor,
    locations: impl IntoIterator<Item = (PathOrId<'a>, Option<(usize, usize)>)>,
) -> Vec<Entry> {
    locations
        .into_iter()
        .map(|(location, lines)| {
            let (start, end) = lines.unwrap_or((0, 0));
            let (target, text) = match location {
                PathOrId::Path(path) => {
                    let text = editor
                        .document_by_path(path)
                        .map(|doc| line_text(doc.text().slice(..), start))
                        .unwrap_or_default();
                    (Target::Path(path.to_owned()), text)
                }
                PathOrId::Id(id) => {
                    let text = editor
                        .documents
                        .get(&id)
                        .map(|doc| line_text(doc.text().slice(..), start))
                        .unwrap_or_default();
                    (target_of(editor, id), text)
                }
            };
            Entry::new(target, Position::Lines { start, end }, start, text)
        })
        .collect()
}

/// Make `entries` the quicklist (the `Ctrl-q` action of pickers).
pub fn set_from_picker(editor: &mut Editor, entries: Vec<Entry>) {
    let count = entries.len();
    editor.quicklist.replace(entries);
    editor.set_status(match count {
        0 => "Cleared the quicklist".to_owned(),
        1 => "Quicklist: 1 location".to_owned(),
        n => format!("Quicklist: {n} locations"),
    });
}

pub fn add_to_quicklist(cx: &mut Context) {
    let entry = current_entry(cx.editor);
    cx.editor.quicklist.push(entry);
    let count = cx.editor.quicklist.len();
    cx.editor
        .set_status(format!("Added to the quicklist ({count})"));
}

/// Move the current selection of the focused view to where `entry` points.
fn place_cursor(editor: &mut Editor, position: &Position) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let selection = match position {
        Position::Lines { start, end } => {
            let last = text.len_lines().saturating_sub(1);
            let (start, end) = ((*start).min(last), (*end).min(last));
            Selection::single(
                text.line_to_char(start),
                line_end_char_index(&text, end).max(text.line_to_char(start)),
            )
        }
        Position::Selection(selection) => {
            let len = text.len_chars();
            selection
                .clone()
                .transform(|range| Range::new(range.anchor.min(len), range.head.min(len)))
        }
    };
    doc.set_selection(view.id, selection);
    align_view(doc, view, Align::Center);
}

/// Open the entry `index` and make it the current one. The place we leave is added to the
/// jumplist.
pub fn visit(editor: &mut Editor, index: usize, action: Action) -> anyhow::Result<()> {
    let Some(entry) = editor.quicklist.entries().get(index).cloned() else {
        bail!("There is no such quicklist entry");
    };
    {
        let (view, doc) = current!(editor);
        push_jump(view, doc);
    }
    match &entry.target {
        Target::Path(path) => {
            editor
                .open(path, action)
                .with_context(|| format!("Could not open {}", path.display()))?;
        }
        Target::Doc(id) => {
            if !editor.documents.contains_key(id) {
                bail!("The buffer of this quicklist entry was closed");
            }
            editor.switch(*id, action);
        }
    }
    place_cursor(editor, &entry.position);
    editor.quicklist.set_current(index);
    let total = editor.quicklist.len();
    editor.set_status(format!("Quicklist {}/{}", index + 1, total));
    Ok(())
}

/// Visit the entry `count` entries away from the current one, only counting the entries in the
/// focused file or buffer when `in_file` is set.
fn step(cx: &mut Context, forward: bool, in_file: bool) {
    if cx.editor.quicklist.is_empty() {
        cx.editor
            .set_status("The quicklist is empty, add locations with Alt-q or Ctrl-q in a picker");
        return;
    }
    let target = target_of(cx.editor, doc!(cx.editor).id());
    let index = cx.editor.quicklist.step(forward, cx.count(), |entry| {
        !in_file || entry.target == target
    });
    let Some(index) = index else {
        cx.editor.set_status("No quicklist entries in this file");
        return;
    };
    if let Err(err) = visit(cx.editor, index, Action::Replace) {
        cx.editor.set_error(format!("{err:#}"));
    }
}

pub fn quicklist_next(cx: &mut Context) {
    step(cx, true, false)
}

pub fn quicklist_prev(cx: &mut Context) {
    step(cx, false, false)
}

pub fn quicklist_next_in_file(cx: &mut Context) {
    step(cx, true, true)
}

pub fn quicklist_prev_in_file(cx: &mut Context) {
    step(cx, false, true)
}

// --- picker ----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct QuickItem {
    /// Identifies the entry, the index in the list changes when others are removed.
    id: u64,
    current: bool,
    location: String,
    text: String,
    /// What the preview shows.
    path: Option<PathBuf>,
    doc: Option<DocumentId>,
    line: usize,
}

fn items(editor: &Editor) -> Vec<QuickItem> {
    let current = editor.quicklist.current();
    editor
        .quicklist
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let (name, path, doc) = match &entry.target {
                Target::Path(path) => (
                    helix_stdx::path::get_relative_path(path)
                        .to_string_lossy()
                        .into_owned(),
                    Some(path.clone()),
                    None,
                ),
                Target::Doc(id) => match editor.documents.get(id) {
                    Some(doc) => (doc.display_name().into_owned(), None, Some(*id)),
                    None => ("[closed buffer]".to_owned(), None, None),
                },
            };
            // Prefer what is in the buffer now over what was there when the entry was added.
            let live = match &entry.target {
                Target::Path(path) => editor.document_by_path(path),
                Target::Doc(id) => editor.documents.get(id),
            }
            .map(|doc| line_text(doc.text().slice(..), entry.line))
            .filter(|text| !text.is_empty());
            QuickItem {
                id: entry.id,
                current: current == Some(index),
                location: format!("{name}:{}", entry.line + 1),
                text: live.unwrap_or_else(|| entry.text.clone()),
                path,
                doc,
                line: entry.line,
            }
        })
        .collect()
}

fn index_of(editor: &Editor, id: u64) -> Option<usize> {
    editor
        .quicklist
        .entries()
        .iter()
        .position(|entry| entry.id == id)
}

pub fn quicklist_picker(cx: &mut Context) {
    if cx.editor.quicklist.is_empty() {
        cx.editor
            .set_status("The quicklist is empty, add locations with Alt-q or Ctrl-q in a picker");
        return;
    }
    let columns = [
        PickerColumn::new("", |item: &QuickItem, _: &()| {
            if item.current { "*" } else { " " }.into()
        })
        .without_filtering(),
        PickerColumn::new("location", |item: &QuickItem, _: &()| {
            item.location.as_str().into()
        }),
        PickerColumn::new("text", |item: &QuickItem, _: &()| item.text.as_str().into()),
    ];

    let handlers: ui::picker::PickerKeyHandlers<QuickItem, ()> = hashmap! {
        ctrl!('x') => Box::new(
            |cx: &mut crate::compositor::Context, item: &QuickItem, _: std::sync::Arc<()>, _| {
                if let Some(index) = index_of(cx.editor, item.id) {
                    cx.editor.quicklist.remove(index);
                }
            },
        ) as ui::picker::PickerKeyHandler<_, _>,
    };

    let picker = Picker::new(
        columns,
        1,
        items(cx.editor),
        (),
        |cx, item: &QuickItem, action| {
            let Some(index) = index_of(cx.editor, item.id) else {
                return;
            };
            if let Err(err) = visit(cx.editor, index, action) {
                cx.editor.set_error(format!("{err:#}"));
            }
        },
    )
    .with_preview(|_editor, item| {
        let location = match (&item.path, item.doc) {
            (Some(path), _) => PathOrId::Path(path.as_path()),
            (None, Some(id)) => PathOrId::Id(id),
            (None, None) => return None,
        };
        Some((location, Some((item.line, item.line))))
    })
    .with_key_handlers(handlers)
    .with_live_refresh(
        |editor| editor.quicklist.version(),
        Some(Box::new(|editor, injector| {
            for item in items(editor) {
                let _ = injector.push(item);
            }
        })),
        |item| item.id.to_string(),
    )
    .with_title("Quicklist");
    cx.push_layer(Box::new(overlaid(
        picker,
        cx.editor.config().fullscreen_overlay,
    )));
}
