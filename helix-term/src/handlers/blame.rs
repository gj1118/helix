use std::{collections::HashMap, mem, path::PathBuf, time::Duration};

use helix_event::register_hook;
use helix_vcs::FileBlame;
use helix_view::{
    events::{DocumentDidOpen, EditorConfigDidChange},
    handlers::{BlameEvent, Handlers},
    DocumentId,
};
use tokio::time::Instant;

use crate::job;

/// A blame request waiting for the debounce to run out.
struct PendingBlame {
    path: PathBuf,
    show_blame_for_line_in_statusline: Option<u32>,
    popup: bool,
}

#[derive(Default)]
pub struct BlameHandler {
    /// One request per document, so a burst of events (opening several files, turning
    /// auto-fetch on, `:reload-all`) blames every document rather than only the last.
    pending: HashMap<DocumentId, PendingBlame>,
}

impl helix_event::AsyncHook for BlameHandler {
    type Event = BlameEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.pending.insert(
            event.doc_id,
            PendingBlame {
                path: event.path,
                show_blame_for_line_in_statusline: event.line,
                popup: event.popup,
            },
        );
        Some(Instant::now() + Duration::from_millis(50))
    }

    fn finish_debounce(&mut self) {
        let pending = mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        // The editor knows each document's workspace trust; the blame itself walks the
        // file's history, which can take seconds on a long one, so it runs off the UI
        // thread and only the result is handed back to the editor.
        job::dispatch_blocking(move |editor, _| {
            for (doc_id, pending) in pending {
                let Some(doc) = editor.document(doc_id) else {
                    continue;
                };
                // Untrusted workspaces open the repository with reduced trust, so its
                // local config (e.g. filter drivers) is not honoured.
                let trust_full = editor
                    .workspace_trust
                    .query(
                        doc.workspace_root(),
                        helix_loader::workspace_trust::TrustQuery::Git,
                    )
                    .is_trusted();
                tokio::task::spawn_blocking(move || {
                    let result = FileBlame::try_new(pending.path, trust_full);
                    job::dispatch_blocking(move |editor, _| {
                        let Some(doc) = editor.document_mut(doc_id) else {
                            return;
                        };
                        doc.file_blame = Some(result);
                        if !editor.config().inline_blame.auto_fetch {
                            if let Some(line) = pending.show_blame_for_line_in_statusline {
                                crate::commands::blame_line_impl(
                                    editor,
                                    doc_id,
                                    line,
                                    pending.popup,
                                );
                            } else {
                                editor.set_status("Blame for this file is now available")
                            }
                        }
                    });
                });
            }
        });
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if event.editor.config().inline_blame.auto_fetch {
            helix_event::send_blocking(
                &tx,
                BlameEvent {
                    path: event.path.to_path_buf(),
                    doc_id: event.doc,
                    line: None,
                    popup: false,
                },
            );
        }
        Ok(())
    });
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut EditorConfigDidChange<'_>| {
        let has_enabled_inline_blame = !event.old_config.inline_blame.auto_fetch
            && event.editor.config().inline_blame.auto_fetch;

        if has_enabled_inline_blame {
            // request blame for all documents, since any of them could have
            // outdated blame
            for doc in event.editor.documents() {
                if let Some(path) = doc.path() {
                    helix_event::send_blocking(
                        &tx,
                        BlameEvent {
                            path: path.to_path_buf(),
                            doc_id: doc.id(),
                            line: None,
                            popup: false,
                        },
                    );
                }
            }
        }
        Ok(())
    });
}
