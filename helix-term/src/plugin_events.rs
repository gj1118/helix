//! The editor events plugins hear of.
//!
//! Most are queued as they happen and handed to the plugins before the next frame
//! ([`take`]); the plugins' handlers then run with the editor to themselves. Some have no
//! hook of their own (a window opened, a buffer renamed, the theme changed...): [`Watch`]
//! finds them by comparing a small snapshot of the editor from one frame to the next.
//!
//! Nothing is queued, nor compared, for an event no plugin has a handler for
//! ([`EventType::is_subscribed`]): `insert_char`, `command` and `key_press` come with every
//! key.

use helix_core::{ChangeSet, Rope};
use helix_event::register_hook;
use helix_lsp::LanguageServerId;
use helix_plugin::{EventData, EventType, EventValue, PluginEvent, TextChange};
use helix_view::{
    events::{
        ConfigDidChange, DiagnosticsDidChange, DocumentDidChange, DocumentDidClose,
        DocumentDidOpen, DocumentFocusLost, LanguageServerExited, LanguageServerInitialized,
        SelectionDidChange,
    },
    DocumentId, Editor, ViewId,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::events::{OnModeSwitch, PostCommand, PostInsertChar};

helix_event::runtime_local! {
    static PENDING: Mutex<Vec<PluginEvent>> = Mutex::new(Vec::new());
    /// The changes of each buffer since its last `buffer_changed`: its text before them and
    /// the changes composed.
    static CHANGES: Mutex<Vec<(DocumentId, Rope, ChangeSet)>> = Mutex::new(Vec::new());
}

/// Most changes a `buffer_changed` lists: more (a replace-all of many matches) are given as
/// one change spanning them.
const MAX_CHANGES: usize = 1_000;

fn changes() -> std::sync::MutexGuard<'static, Vec<(DocumentId, Rope, ChangeSet)>> {
    match CHANGES.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Adds the changes of a transaction to the buffer's since its last `buffer_changed`.
fn record_changes(document_id: DocumentId, old_text: &Rope, changes_set: &ChangeSet) {
    let mut all = changes();
    let index = all.iter().position(|(id, ..)| *id == document_id);
    let entry = index.map(|index| all.swap_remove(index));
    match entry {
        // (a change the plugins did not hear of in between: start over from this one)
        Some((_, before, composed)) if composed.len_after() == changes_set.len() => {
            all.push((document_id, before, composed.compose(changes_set.clone())));
        }
        _ => all.push((document_id, old_text.clone(), changes_set.clone())),
    }
}

/// The changes `changes_set` makes to `old_text`, as plugins get them: in order, adjacent
/// ones merged, at most [`MAX_CHANGES`] (more are one change from the first to the last).
pub(crate) fn text_changes(old_text: &Rope, changes_set: &ChangeSet) -> Vec<TextChange> {
    // (old_from, old_to, inserted chars)
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (from, to, text) in changes_set.changes_iter() {
        let text = text.map(|text| text.to_string()).unwrap_or_default();
        match spans.last_mut() {
            Some(last) if last.1 == from => {
                last.1 = to;
                last.2.push_str(&text);
            }
            _ => spans.push((from, to, text)),
        }
    }
    if spans.is_empty() {
        return Vec::new();
    }
    let mut new_text = old_text.clone();
    changes_set.apply(&mut new_text);
    if spans.len() > MAX_CHANGES {
        let first = spans.first().expect("not empty").0;
        let last = spans.last().expect("not empty").1;
        let delta = new_text.len_chars() as isize - old_text.len_chars() as isize;
        let new_last = (last as isize + delta) as usize;
        spans = vec![(first, last, new_text.slice(first..new_last).to_string())];
    }
    // The line of the last char of a change (its start line when empty).
    let end_line = |text: &Rope, from: usize, to: usize| {
        text.char_to_line(if to > from { to - 1 } else { from }) + 1
    };
    let mut delta: isize = 0;
    spans
        .into_iter()
        .map(|(old_from, old_to, text)| {
            let from = (old_from as isize + delta) as usize;
            let to = from + text.chars().count();
            delta += (to - from) as isize - (old_to - old_from) as isize;
            TextChange {
                from,
                to,
                old_from,
                old_to,
                start_line: new_text.char_to_line(from) + 1,
                end_line: end_line(&new_text, from, to),
                old_start_line: old_text.char_to_line(old_from) + 1,
                old_end_line: end_line(old_text, old_from, old_to),
                removed: old_text.slice(old_from..old_to).to_string(),
                text,
            }
        })
        .collect()
}

/// Most events a frame queues: a macro replaying many thousands of commands is not turned
/// into as many handler calls (the first ones are kept).
const MAX_PENDING: usize = 10_000;

fn pending() -> std::sync::MutexGuard<'static, Vec<PluginEvent>> {
    match PENDING.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Queues an event (when some plugin has a handler for it), fired before the next frame.
pub(crate) fn queue(event: PluginEvent) {
    if !event.event_type.is_subscribed() {
        return;
    }
    let mut events = pending();
    if events.len() < MAX_PENDING {
        events.push(event);
    }
}

/// Queues an event with named fields.
pub(crate) fn queue_fields(event_type: EventType, fields: Vec<(&'static str, EventValue)>) {
    if event_type.is_subscribed() {
        queue(PluginEvent::with_fields(event_type, fields));
    }
}

/// Queues a document event unless one of the same type for the same document is pending
/// already: a macro or a long command changes a document (or its selections) many thousands
/// of times between two frames, the plugins hear of it once.
fn queue_once(
    event_type: EventType,
    document_id: DocumentId,
    path: impl FnOnce() -> Option<PathBuf>,
) {
    if !event_type.is_subscribed() {
        return;
    }
    let mut events = pending();
    let is_pending = events.iter().any(|pending| {
        pending.event_type == event_type
            && matches!(
                pending.data,
                EventData::Buffer { document_id: id, .. } if id == document_id
            )
    });
    if !is_pending && events.len() < MAX_PENDING {
        events.push(PluginEvent {
            event_type,
            data: EventData::Buffer {
                document_id,
                path: path(),
            },
        });
    }
}

/// Queues an event with no fields unless one is pending already.
fn queue_once_plain(event_type: EventType) {
    if !event_type.is_subscribed() {
        return;
    }
    let mut events = pending();
    if !events
        .iter()
        .any(|pending| pending.event_type == event_type)
    {
        events.push(PluginEvent::with_fields(event_type, Vec::new()));
    }
}

/// The events queued since the last call. A `buffer_changed` gets the changes made since the
/// last one.
pub(crate) fn take() -> Vec<PluginEvent> {
    let mut events = std::mem::take(&mut *pending());
    if events
        .iter()
        .any(|event| event.event_type == EventType::OnBufferChanged)
    {
        let mut all = changes();
        for event in &mut events {
            if event.event_type != EventType::OnBufferChanged {
                continue;
            }
            if let EventData::Buffer { document_id, path } = &event.data {
                let changes = all
                    .iter()
                    .position(|(id, ..)| id == document_id)
                    .map(|index| {
                        let (_, before, composed) = all.swap_remove(index);
                        text_changes(&before, &composed)
                    })
                    .unwrap_or_default();
                event.data = EventData::BufferChanged {
                    document_id: *document_id,
                    path: path.clone(),
                    changes,
                };
            }
        }
    }
    events
}

/// A number for a language server, unique among the running ones (its slot).
pub(crate) fn language_server_number(id: LanguageServerId) -> usize {
    // Displayed as `<slot>v<version>`.
    id.to_string()
        .split('v')
        .next()
        .and_then(|slot| slot.parse().ok())
        .unwrap_or_default()
}

fn path_value(path: Option<&std::path::Path>) -> EventValue {
    path.map(|p| p.to_string_lossy().into_owned()).into()
}

/// The language server fields: its number and name (when it still exists).
fn server_fields(editor: &Editor, id: LanguageServerId) -> Vec<(&'static str, EventValue)> {
    vec![
        (
            "language_server_id",
            EventValue::Int(language_server_number(id) as i64),
        ),
        (
            "name",
            editor
                .language_server_by_id(id)
                .map(|server| server.name().to_string())
                .into(),
        ),
    ]
}

/// Queues the editor events plugins subscribe to; they are fired before the next frame.
pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        queue(PluginEvent {
            event_type: EventType::OnBufferOpen,
            data: EventData::Buffer {
                document_id: event.doc,
                path: Some(event.path.clone()),
            },
        });
        helix_event::request_redraw();
        Ok(())
    });

    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        let doc = &event.doc;
        queue_once(EventType::OnSelectionChange, doc.id(), || {
            doc.path().map(PathBuf::from)
        });
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // Previews (of a completion, say) are undone right away.
        if !event.ghost_transaction && EventType::OnBufferChanged.is_subscribed() {
            let doc = &event.doc;
            record_changes(doc.id(), event.old_text, event.changes);
            queue_once(EventType::OnBufferChanged, doc.id(), || {
                doc.path().map(PathBuf::from)
            });
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let closed = event.doc.id();
        changes().retain(|(id, ..)| *id != closed);
        queue(PluginEvent {
            event_type: EventType::OnBufferClose,
            data: EventData::Buffer {
                document_id: event.doc.id(),
                path: event.doc.path().map(PathBuf::from),
            },
        });
        helix_event::request_redraw();
        Ok(())
    });

    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let path = event
            .editor
            .document(event.doc)
            .and_then(|doc| doc.path().map(PathBuf::from));
        queue(PluginEvent {
            event_type: EventType::OnBufferFocusLost,
            data: EventData::Buffer {
                document_id: event.doc,
                path,
            },
        });
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        queue_fields(
            EventType::OnLspInitialized,
            server_fields(event.editor, event.server_id),
        );
        helix_event::request_redraw();
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        queue_fields(
            EventType::OnLspExit,
            server_fields(event.editor, event.server_id),
        );
        helix_event::request_redraw();
        Ok(())
    });

    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        let path = event
            .editor
            .document(event.doc)
            .and_then(|doc| doc.path().map(PathBuf::from));
        queue(PluginEvent {
            event_type: EventType::OnLspDiagnostic,
            data: EventData::Buffer {
                document_id: event.doc,
                path,
            },
        });
        Ok(())
    });

    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if EventType::OnModeChange.is_subscribed() {
            queue(PluginEvent {
                event_type: EventType::OnModeChange,
                data: EventData::ModeChange {
                    old_mode: format!("{:?}", event.old_mode),
                    new_mode: format!("{:?}", event.new_mode),
                },
            });
            helix_event::request_redraw();
        }
        Ok(())
    });

    register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
        if EventType::OnInsertChar.is_subscribed() {
            let editor = &event.cx.editor;
            let doc = editor.tree.try_get(editor.tree.focus).map(|view| view.doc);
            queue_fields(
                EventType::OnInsertChar,
                vec![
                    ("char", event.c.to_string().into()),
                    (
                        "document_id",
                        doc.map_or(EventValue::Nil, EventValue::Document),
                    ),
                ],
            );
        }
        Ok(())
    });

    register_hook!(move |event: &mut PostCommand<'_, '_>| {
        if EventType::OnCommand.is_subscribed() {
            queue_fields(
                EventType::OnCommand,
                vec![("name", event.command.name().into())],
            );
        }
        Ok(())
    });

    register_hook!(move |_event: &mut ConfigDidChange<'_>| {
        queue_once_plain(EventType::OnConfigChange);
        Ok(())
    });
}

/// What [`Watch`] remembers of a diff.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DiffState {
    left_view: ViewId,
    left_doc: DocumentId,
    right_doc: DocumentId,
    changes: usize,
}

impl DiffState {
    fn of(session: &helix_view::diff_view::DiffSession) -> Self {
        DiffState {
            left_view: session.left_view,
            left_doc: session.left_doc,
            right_doc: session.right_doc,
            changes: std::sync::Arc::as_ptr(&session.hunks) as *const () as usize,
        }
    }

    fn fields(self, right_view: ViewId) -> Vec<(&'static str, EventValue)> {
        vec![
            ("left_window_id", self.left_view.into()),
            ("right_window_id", right_view.into()),
            ("left_document_id", self.left_doc.into()),
            ("right_document_id", self.right_doc.into()),
        ]
    }
}

/// What [`Watch`] remembers of a document.
#[derive(Debug, Clone, PartialEq)]
struct DocState {
    path: Option<PathBuf>,
    language: Option<String>,
    servers: Vec<LanguageServerId>,
}

/// The events without a hook of their own, found by comparing the editor with its state
/// at the last frame.
#[derive(Default)]
pub(crate) struct Watch {
    /// Window -> its document and scroll position.
    windows: HashMap<ViewId, (DocumentId, helix_view::view::ViewPosition)>,
    focus: Option<ViewId>,
    docs: HashMap<DocumentId, DocState>,
    theme: String,
    cwd: PathBuf,
    /// The path under the explorer's cursor.
    explorer_selected: Option<PathBuf>,
    /// The diffs shown, by their right window: their left window, their documents, and their
    /// changes (by address: they are replaced, not changed, when worked out again).
    diffs: HashMap<ViewId, DiffState>,
    /// Whether the snapshot is out of date (nobody listened at the last frame): the next
    /// frame takes one without events.
    stale: bool,
}

/// The events a [`Watch`] reports.
const WATCHED: &[EventType] = &[
    EventType::OnWindowOpen,
    EventType::OnWindowClose,
    EventType::OnWindowFocus,
    EventType::OnScroll,
    EventType::OnBufferRename,
    EventType::OnLanguageChange,
    EventType::OnThemeChange,
    EventType::OnCwdChange,
    EventType::OnLspAttach,
    EventType::OnLspDetach,
    EventType::OnExplorerSelect,
    EventType::OnDiffOpen,
    EventType::OnDiffUpdate,
    EventType::OnDiffClose,
];

impl Watch {
    /// A watch of the editor as it is (no event for what exists already).
    pub(crate) fn new(editor: &Editor) -> Self {
        let mut watch = Watch::default();
        watch.snapshot(editor);
        watch
    }

    fn doc_state(doc: &helix_view::Document) -> DocState {
        DocState {
            path: doc.path().map(PathBuf::from),
            language: doc.language_name().map(String::from),
            servers: doc.language_servers().map(|ls| ls.id()).collect(),
        }
    }

    fn diffs(editor: &Editor) -> HashMap<ViewId, DiffState> {
        editor
            .diff_sessions
            .iter()
            .map(|session| (session.right_view, DiffState::of(session)))
            .collect()
    }

    fn snapshot(&mut self, editor: &Editor) {
        self.windows = editor
            .tree
            .views()
            .map(|(view, _)| {
                let position = editor
                    .document(view.doc)
                    .map(|doc| doc.view_offset(view.id))
                    .unwrap_or_default();
                (view.id, (view.doc, position))
            })
            .collect();
        self.focus = editor.tree.try_get(editor.tree.focus).map(|view| view.id);
        self.docs = editor
            .documents()
            .map(|doc| (doc.id(), Self::doc_state(doc)))
            .collect();
        self.theme = editor.theme.name().to_string();
        self.cwd = helix_stdx::env::current_working_dir();
        self.explorer_selected = editor.plugin_explorer.selected.clone();
        self.diffs = Self::diffs(editor);
        self.stale = false;
    }

    /// The events since the last call (in a stable order: windows, buffers, the rest).
    pub(crate) fn events(&mut self, editor: &Editor) -> Vec<PluginEvent> {
        if !WATCHED.iter().any(|event| event.is_subscribed()) {
            self.stale = true;
            return Vec::new();
        }
        if self.stale {
            self.snapshot(editor);
            return Vec::new();
        }
        let mut events = Vec::new();
        let mut push = |event_type: EventType, fields: Vec<(&'static str, EventValue)>| {
            if event_type.is_subscribed() {
                events.push(PluginEvent::with_fields(event_type, fields));
            }
        };

        // windows
        let mut windows = HashMap::with_capacity(self.windows.len());
        for (view, _) in editor.tree.views() {
            let position = editor
                .document(view.doc)
                .map(|doc| doc.view_offset(view.id))
                .unwrap_or_default();
            match self.windows.get(&view.id) {
                None => push(
                    EventType::OnWindowOpen,
                    vec![
                        ("window_id", view.id.into()),
                        ("document_id", view.doc.into()),
                    ],
                ),
                // Scrolled: in the same buffer (another buffer in the window is no scroll).
                Some((doc, old)) if *doc == view.doc && *old != position => push(
                    EventType::OnScroll,
                    vec![
                        ("window_id", view.id.into()),
                        ("document_id", view.doc.into()),
                        ("top_char", EventValue::Int(position.anchor as i64)),
                    ],
                ),
                _ => {}
            }
            windows.insert(view.id, (view.doc, position));
        }
        let mut closed: Vec<_> = self
            .windows
            .iter()
            .filter(|(id, _)| !windows.contains_key(id))
            .map(|(id, (doc, _))| (*id, *doc))
            .collect();
        closed.sort_by_key(|(id, _)| format!("{id:?}"));
        for (id, doc) in closed {
            push(
                EventType::OnWindowClose,
                vec![("window_id", id.into()), ("document_id", doc.into())],
            );
        }
        self.windows = windows;

        let focus = editor.tree.try_get(editor.tree.focus).map(|view| view.id);
        if focus != self.focus {
            if let Some(view) = focus.and_then(|id| editor.tree.try_get(id)) {
                push(
                    EventType::OnWindowFocus,
                    vec![
                        ("window_id", view.id.into()),
                        ("document_id", view.doc.into()),
                        (
                            "previous_window_id",
                            self.focus.map_or(EventValue::Nil, EventValue::View),
                        ),
                    ],
                );
            }
            self.focus = focus;
        }

        // buffers (a new one has `buffer_open`, a closed one `buffer_close`), compared in
        // place: a frame allocates only for what changed
        for doc in editor.documents() {
            let id = doc.id();
            // a new buffer: its servers are attached, nothing else changed
            let old = self.docs.entry(id).or_insert_with(|| DocState {
                servers: Vec::new(),
                ..Self::doc_state(doc)
            });
            let path: Option<&std::path::Path> = doc.path().map(AsRef::as_ref);
            if old.path.as_deref() != path {
                push(
                    EventType::OnBufferRename,
                    vec![
                        ("document_id", id.into()),
                        ("path", path_value(path)),
                        ("old_path", path_value(old.path.as_deref())),
                    ],
                );
                old.path = path.map(PathBuf::from);
            }
            let language = doc.language_name();
            if old.language.as_deref() != language {
                push(
                    EventType::OnLanguageChange,
                    vec![
                        ("document_id", id.into()),
                        ("language", language.map(String::from).into()),
                        ("old_language", old.language.clone().into()),
                    ],
                );
                old.language = language.map(String::from);
            }
            if !doc
                .language_servers()
                .map(|ls| ls.id())
                .eq(old.servers.iter().copied())
            {
                let servers: Vec<LanguageServerId> =
                    doc.language_servers().map(|ls| ls.id()).collect();
                for &server in servers.iter().filter(|s| !old.servers.contains(s)) {
                    let mut fields = vec![("document_id", id.into())];
                    fields.extend(server_fields(editor, server));
                    push(EventType::OnLspAttach, fields);
                }
                for &server in old.servers.iter().filter(|s| !servers.contains(s)) {
                    let mut fields = vec![("document_id", id.into())];
                    fields.extend(server_fields(editor, server));
                    push(EventType::OnLspDetach, fields);
                }
                old.servers = servers;
            }
        }
        self.docs.retain(|id, _| editor.documents.contains_key(id));

        let theme = editor.theme.name();
        if theme != self.theme {
            push(
                EventType::OnThemeChange,
                vec![
                    ("theme", theme.into()),
                    ("old_theme", self.theme.clone().into()),
                ],
            );
            self.theme = theme.to_string();
        }

        let cwd = helix_stdx::env::current_working_dir();
        if cwd != self.cwd {
            push(
                EventType::OnCwdChange,
                vec![
                    ("path", path_value(Some(&cwd))),
                    ("old_path", path_value(Some(&self.cwd))),
                ],
            );
            self.cwd = cwd;
        }

        let selected = &editor.plugin_explorer.selected;
        if *selected != self.explorer_selected {
            if let Some(path) = selected {
                push(
                    EventType::OnExplorerSelect,
                    vec![("path", path_value(Some(path)))],
                );
            }
            self.explorer_selected = selected.clone();
        }

        // diffs: opened, changes worked out again, closed
        let diffs = Self::diffs(editor);
        for session in &editor.diff_sessions {
            let right_view = session.right_view;
            let state = diffs[&right_view];
            match self.diffs.get(&right_view) {
                Some(old) if old.left_doc == state.left_doc => {
                    if old.changes != state.changes {
                        push(EventType::OnDiffUpdate, state.fields(right_view));
                    }
                }
                // a diff in the window (another one replaced it: closed, then opened)
                Some(old) => {
                    push(EventType::OnDiffClose, old.fields(right_view));
                    push(EventType::OnDiffOpen, state.fields(right_view));
                }
                None => push(EventType::OnDiffOpen, state.fields(right_view)),
            }
        }
        let mut closed: Vec<_> = self
            .diffs
            .iter()
            .filter(|(view, _)| !diffs.contains_key(view))
            .map(|(view, state)| (*view, *state))
            .collect();
        closed.sort_by_key(|(view, _)| format!("{view:?}"));
        for (view, state) in closed {
            push(EventType::OnDiffClose, state.fields(view));
        }
        self.diffs = diffs;

        events
    }
}

/// The `mouse` event of a mouse event (`None` for a move without a button: too many, and
/// nothing to act on).
pub(crate) fn mouse_event(event: &helix_view::input::MouseEvent) -> Option<PluginEvent> {
    use helix_view::input::{MouseButton, MouseEventKind};
    let button = |button: &MouseButton| match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };
    let (kind, button) = match &event.kind {
        MouseEventKind::Down(b) => ("down", Some(button(b))),
        MouseEventKind::Up(b) => ("up", Some(button(b))),
        MouseEventKind::Drag(b) => ("drag", Some(button(b))),
        MouseEventKind::Moved => return None,
        MouseEventKind::ScrollDown => ("scroll_down", None),
        MouseEventKind::ScrollUp => ("scroll_up", None),
        MouseEventKind::ScrollLeft => ("scroll_left", None),
        MouseEventKind::ScrollRight => ("scroll_right", None),
    };
    Some(PluginEvent::with_fields(
        EventType::OnMouse,
        vec![
            ("kind", kind.into()),
            ("button", button.map(String::from).into()),
            ("row", EventValue::Int(event.row as i64)),
            ("col", EventValue::Int(event.column as i64)),
            ("modifiers", modifiers(event.modifiers).into()),
        ],
    ))
}

/// Key modifiers as in key names: `"C-S"` for Ctrl and Shift, `""` for none.
fn modifiers(modifiers: helix_view::keyboard::KeyModifiers) -> String {
    use helix_view::keyboard::KeyModifiers;
    [
        (KeyModifiers::CONTROL, "C"),
        (KeyModifiers::ALT, "A"),
        (KeyModifiers::SHIFT, "S"),
    ]
    .into_iter()
    .filter(|(modifier, _)| modifiers.contains(*modifier))
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Transaction;

    fn change(text: &Rope, edits: Vec<(usize, usize, Option<&str>)>) -> ChangeSet {
        Transaction::change(
            text,
            edits
                .into_iter()
                .map(|(from, to, text)| (from, to, text.map(Into::into))),
        )
        .changes()
        .clone()
    }

    #[test]
    fn changes_are_given_in_the_new_and_the_old_text() {
        let text = Rope::from("one\ntwo\nthree\n");
        // "two" -> "2\n2" (a line more), then "three" removed
        let set = change(&text, vec![(4, 7, Some("2\n2")), (8, 13, None)]);
        let changes = text_changes(&text, &set);
        assert_eq!(changes.len(), 2_usize);
        assert_eq!(
            changes[0],
            TextChange {
                from: 4,
                to: 7,
                old_from: 4,
                old_to: 7,
                start_line: 2,
                end_line: 3,
                old_start_line: 2,
                old_end_line: 2,
                text: "2\n2".into(),
                removed: "two".into(),
            }
        );
        // after the first one, which was as long: the same chars, a line further down
        assert_eq!(
            changes[1],
            TextChange {
                from: 8,
                to: 8,
                old_from: 8,
                old_to: 13,
                start_line: 4,
                end_line: 4,
                old_start_line: 3,
                old_end_line: 3,
                text: String::new(),
                removed: "three".into(),
            }
        );
    }

    #[test]
    fn composed_changes_are_merged_and_shifted() {
        let text = Rope::from("abc");
        let first = change(&text, vec![(0, 0, Some("xx"))]);
        let mut after = text.clone();
        first.apply(&mut after);
        // typed further: "xxabc" becomes "xxyab"
        let second = change(&after, vec![(2, 2, Some("y")), (4, 5, None)]);
        let changes = text_changes(&text, &first.compose(second));
        let summary: Vec<(usize, usize, usize, usize, String, String)> = changes
            .into_iter()
            .map(|c| (c.from, c.to, c.old_from, c.old_to, c.text, c.removed))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, 3, 0, 0, "xxy".to_string(), String::new()),
                (5, 5, 2, 3, String::new(), "c".to_string()),
            ]
        );
    }

    #[test]
    fn too_many_changes_are_one_spanning_them() {
        let text = Rope::from("a".repeat(MAX_CHANGES * 2 + 2));
        let edits = (0..=MAX_CHANGES)
            .map(|i| (i * 2, i * 2 + 1, Some("bb")))
            .collect();
        let changes = text_changes(&text, &change(&text, edits));
        assert_eq!(changes.len(), 1_usize);
        let change = &changes[0];
        assert_eq!(
            (change.from, change.old_from, change.old_to),
            (0, 0, MAX_CHANGES * 2 + 1)
        );
        assert_eq!(change.to, (MAX_CHANGES + 1) * 3 - 1);
        assert_eq!(change.removed.len(), MAX_CHANGES * 2 + 1);
        assert!(change.text.starts_with("bbabb"));
    }

    #[test]
    fn no_change_is_no_changes() {
        let text = Rope::from("same");
        assert_eq!(
            text_changes(&text, &ChangeSet::new(text.slice(..))),
            Vec::<TextChange>::new()
        );
    }
}
