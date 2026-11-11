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

use helix_event::register_hook;
use helix_lsp::LanguageServerId;
use helix_plugin::{EventData, EventType, EventValue, PluginEvent};
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

/// The events queued since the last call.
pub(crate) fn take() -> Vec<PluginEvent> {
    std::mem::take(&mut *pending())
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
        if !event.ghost_transaction {
            let doc = &event.doc;
            queue_once(EventType::OnBufferChanged, doc.id(), || {
                doc.path().map(PathBuf::from)
            });
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidClose<'_>| {
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

        // buffers (a new one has `buffer_open`, a closed one `buffer_close`)
        let mut docs = HashMap::with_capacity(self.docs.len());
        for doc in editor.documents() {
            let state = Self::doc_state(doc);
            let old = self.docs.get(&doc.id());
            let id = doc.id();
            if let Some(old) = old {
                if old.path != state.path {
                    push(
                        EventType::OnBufferRename,
                        vec![
                            ("document_id", id.into()),
                            ("path", path_value(state.path.as_deref())),
                            ("old_path", path_value(old.path.as_deref())),
                        ],
                    );
                }
                if old.language != state.language {
                    push(
                        EventType::OnLanguageChange,
                        vec![
                            ("document_id", id.into()),
                            ("language", state.language.clone().into()),
                            ("old_language", old.language.clone().into()),
                        ],
                    );
                }
            }
            let old_servers: &[LanguageServerId] = old.map_or(&[], |old| &old.servers);
            for &server in state.servers.iter().filter(|s| !old_servers.contains(s)) {
                let mut fields = vec![("document_id", id.into())];
                fields.extend(server_fields(editor, server));
                push(EventType::OnLspAttach, fields);
            }
            for &server in old_servers.iter().filter(|s| !state.servers.contains(s)) {
                let mut fields = vec![("document_id", id.into())];
                fields.extend(server_fields(editor, server));
                push(EventType::OnLspDetach, fields);
            }
            docs.insert(id, state);
        }
        self.docs = docs;

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
