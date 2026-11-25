//! The quicklist commands: collecting locations (from the cursor, or from a picker with
//! `Ctrl-q`), walking them and the quicklist picker. See `helix_view::quicklist` for the list
//! itself.
//!
//! Each window also has a location list of its own (Neovim's `:lopen`): the same commands
//! work on it, the entries opening in that window.

use std::path::PathBuf;

use anyhow::{bail, Context as _};
use helix_core::{hashmap, line_ending::line_end_char_index, Range, Selection};
use helix_view::{
    align_view,
    editor::Action,
    quicklist::{Entry, Position, Quicklist, Target},
    Align, DocumentId, Editor, ViewId,
};

use super::{push_jump, Context};
use crate::{
    ctrl,
    ui::{self, picker::PathOrId, Picker, PickerColumn},
};

/// A list of locations: the editor's quicklist, or the location list of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Quick,
    Location(ViewId),
}

impl List {
    /// The location list of the focused window.
    pub fn focused_location(editor: &Editor) -> Self {
        List::Location(editor.tree.focus)
    }

    /// The list; `None` for the location list of a window that was closed.
    pub fn get(self, editor: &Editor) -> Option<&Quicklist> {
        match self {
            List::Quick => Some(&editor.quicklist),
            List::Location(view) => editor.tree.try_get(view).map(|view| &view.location_list),
        }
    }

    pub fn get_mut(self, editor: &mut Editor) -> Option<&mut Quicklist> {
        match self {
            List::Quick => Some(&mut editor.quicklist),
            List::Location(view) => {
                editor.tree.try_get(view)?;
                Some(&mut editor.tree.get_mut(view).location_list)
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            List::Quick => "quicklist",
            List::Location(_) => "location list",
        }
    }

    fn title(self) -> &'static str {
        match self {
            List::Quick => "Quicklist",
            List::Location(_) => "Location list",
        }
    }

    /// What to do to fill it, said when it is empty.
    fn empty_hint(self) -> &'static str {
        match self {
            List::Quick => "The quicklist is empty, add locations with Alt-q or Ctrl-q in a picker",
            List::Location(_) => {
                "The location list of this window is empty: Alt-q in a picker, :ladd or \
                 :ldiagnostics fill it"
            }
        }
    }
}

/// How much of a line is kept as an entry's text.
const MAX_TEXT_CHARS: usize = 200;

fn line_text(text: helix_core::RopeSlice, line: usize) -> String {
    if line >= text.len_lines() {
        return String::new();
    }
    // Only the kept part of the line is read: a picker over a huge minified file
    // turns into many entries on one very long line.
    let kept: String = text
        .line(line)
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take(MAX_TEXT_CHARS)
        .collect();
    kept.trim_end().to_owned()
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

/// The diagnostics of the focused buffer, in order, as entries (`:ldiagnostics`): each
/// selects its range, and its text is `error: message`.
pub fn diagnostics_entries(editor: &Editor) -> Vec<Entry> {
    use helix_core::diagnostic::Severity;
    let (_, doc) = helix_view::current_ref!(editor);
    let target = target_of(editor, doc.id());
    let len = doc.text().len_chars();
    let mut diagnostics: Vec<&helix_core::Diagnostic> = doc.diagnostics().iter().collect();
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                Some(Severity::Error) => "error",
                Some(Severity::Warning) => "warning",
                Some(Severity::Info) => "info",
                Some(Severity::Hint) => "hint",
                None => "note",
            };
            let message = diagnostic.message.lines().next().unwrap_or_default();
            let (start, end) = (
                diagnostic.range.start.min(len),
                diagnostic.range.end.min(len),
            );
            Entry::new(
                target.clone(),
                Position::Selection(Selection::single(start, end.max(start))),
                diagnostic.line,
                format!("{severity}: {message}"),
            )
        })
        .collect()
}

/// Make `entries` the list `list` (`Ctrl-q` and `Alt-q` of pickers, `:ldiagnostics`).
pub fn set_list(editor: &mut Editor, list: List, entries: Vec<Entry>) {
    let count = entries.len();
    let Some(target) = list.get_mut(editor) else {
        return;
    };
    target.replace(entries);
    let title = list.title();
    editor.set_status(match count {
        0 => format!("Cleared the {}", list.name()),
        1 => format!("{title}: 1 location"),
        n => format!("{title}: {n} locations"),
    });
}

pub fn add_to_quicklist(cx: &mut Context) {
    add_to(cx.editor, List::Quick)
}

/// Adds the cursor's place to the location list of the focused window (`:ladd`).
pub fn add_to_location_list(cx: &mut Context) {
    let list = List::focused_location(cx.editor);
    add_to(cx.editor, list)
}

pub fn add_to(editor: &mut Editor, list: List) {
    let entry = current_entry(editor);
    let Some(target) = list.get_mut(editor) else {
        return;
    };
    target.push(entry);
    let count = target.len();
    editor.set_status(format!("Added to the {} ({count})", list.name()));
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
/// An entry of a location list opens in its window.
pub fn visit_in(
    editor: &mut Editor,
    list: List,
    index: usize,
    action: Action,
) -> anyhow::Result<()> {
    let Some(entry) = list
        .get(editor)
        .and_then(|entries| entries.entries().get(index).cloned())
    else {
        bail!("There is no such {} entry", list.name());
    };
    if let List::Location(view) = list {
        editor.focus(view);
    }
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
    // (a location list follows its window into the split it was opened in)
    let list = match (list, action) {
        (List::Location(_), Action::HorizontalSplit | Action::VerticalSplit) => {
            List::focused_location(editor)
        }
        _ => list,
    };
    let Some(entries) = list.get_mut(editor) else {
        return Ok(());
    };
    entries.set_current(index);
    let total = entries.len();
    editor.set_status(format!("{} {}/{}", list.title(), index + 1, total));
    Ok(())
}

/// Visit the entry `count` entries away from the current one, only counting the entries in the
/// focused file or buffer when `in_file` is set.
fn step(cx: &mut Context, forward: bool, in_file: bool) {
    step_in(cx, List::Quick, forward, in_file)
}

fn step_in(cx: &mut Context, list: List, forward: bool, in_file: bool) {
    // `]q`/`[q` visit the entries of the quickfix panel while it is shown.
    if list == List::Quick && !in_file && cx.editor.quickfix_panel.visible {
        let count = cx.count();
        cx.callback.push(Box::new(move |compositor, cx| {
            compositor.quickfix_step(forward, count, cx);
        }));
        return;
    }
    if list.get(cx.editor).is_none_or(Quicklist::is_empty) {
        cx.editor.set_status(list.empty_hint());
        return;
    }
    let target = target_of(cx.editor, doc!(cx.editor).id());
    let count = cx.count();
    let index = list.get(cx.editor).and_then(|entries| {
        entries.step(forward, count, |entry| !in_file || entry.target == target)
    });
    let Some(index) = index else {
        cx.editor
            .set_status(format!("No {} entries in this file", list.name()));
        return;
    };
    if let Err(err) = visit_in(cx.editor, list, index, Action::Replace) {
        cx.editor.set_error(format!("{err:#}"));
    }
}

/// `]L`: the next entry of the focused window's location list.
pub fn location_list_next(cx: &mut Context) {
    let list = List::focused_location(cx.editor);
    step_in(cx, list, true, false)
}

/// `[L`: the previous entry of the focused window's location list.
pub fn location_list_prev(cx: &mut Context) {
    let list = List::focused_location(cx.editor);
    step_in(cx, list, false, false)
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

fn items(editor: &Editor, list: List) -> Vec<QuickItem> {
    let Some(entries) = list.get(editor) else {
        return Vec::new();
    };
    let current = entries.current();
    entries
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

fn index_of(editor: &Editor, list: List, id: u64) -> Option<usize> {
    list.get(editor)?
        .entries()
        .iter()
        .position(|entry| entry.id == id)
}

pub fn quicklist_picker(cx: &mut Context) {
    match picker(cx.editor) {
        Some(picker) => ui::quickfix::show_from_command(cx, picker),
        None => cx.editor.set_status(List::Quick.empty_hint()),
    }
}

/// `Space L`, `:lopen`: the location list of the focused window, in a picker (or the
/// quickfix panel).
pub fn location_list_picker(cx: &mut Context) {
    let list = List::focused_location(cx.editor);
    match picker_of(cx.editor, list) {
        Some(picker) => ui::quickfix::show_from_command(cx, picker),
        None => cx.editor.set_status(list.empty_hint()),
    }
}

/// The picker of the quicklist; `None` when it is empty.
pub fn picker(editor: &Editor) -> Option<Picker<QuickItem, ()>> {
    picker_of(editor, List::Quick)
}

/// The picker of `list`; `None` when it is empty.
pub fn picker_of(editor: &Editor, list: List) -> Option<Picker<QuickItem, ()>> {
    if list.get(editor).is_none_or(Quicklist::is_empty) {
        return None;
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
            move |cx: &mut crate::compositor::Context, item: &QuickItem, _: std::sync::Arc<()>, _| {
                if let Some(index) = index_of(cx.editor, list, item.id) {
                    if let Some(entries) = list.get_mut(cx.editor) {
                        entries.remove(index);
                    }
                }
            },
        ) as ui::picker::PickerKeyHandler<_, _>,
    };

    let picker = Picker::new(
        columns,
        1,
        items(editor, list),
        (),
        move |cx, item: &QuickItem, action| {
            let Some(index) = index_of(cx.editor, list, item.id) else {
                return;
            };
            if let Err(err) = visit_in(cx.editor, list, index, action) {
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
    .with_key_handlers(helix_view::component_keys::Component::Quicklist, handlers)
    .with_live_refresh(
        move |editor| list.get(editor).map_or(0, Quicklist::version),
        Some(Box::new(move |editor, injector| {
            for item in items(editor, list) {
                let _ = injector.push(item);
            }
        })),
        |item| item.id.to_string(),
    )
    .with_title(list.title());
    Some(picker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Rope;

    #[test]
    fn line_text_is_trimmed_and_bounded() {
        let long = "x".repeat(MAX_TEXT_CHARS * 10);
        let rope = Rope::from(format!("  \tab c  \r\n{long}\n"));
        let text = rope.slice(..);
        assert_eq!(line_text(text, 0), "ab c");
        assert_eq!(line_text(text, 1), "x".repeat(MAX_TEXT_CHARS));
        assert_eq!(line_text(text, 2), "");
        assert_eq!(line_text(text, 99), "");
    }
}
