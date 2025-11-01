use std::{mem, time::Duration};

use helix_event::register_hook;
use helix_vcs::FileBlame;
use helix_view::{
    events::{DocumentDidOpen, EditorConfigDidChange},
    handlers::{BlameEvent, Handlers},
    DocumentId,
};
use tokio::time::Instant;

use crate::job;

#[derive(Default)]
pub struct BlameHandler {
    pending_path: Option<std::path::PathBuf>,
    doc_id: DocumentId,
    show_blame_for_line_in_statusline: Option<u32>,
}

impl helix_event::AsyncHook for BlameHandler {
    type Event = BlameEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.doc_id = event.doc_id;
        self.show_blame_for_line_in_statusline = event.line;
        self.pending_path = Some(event.path);
        Some(Instant::now() + Duration::from_millis(50))
    }

    fn finish_debounce(&mut self) {
        let doc_id = self.doc_id;
        let line_blame = self.show_blame_for_line_in_statusline;
        let path = mem::take(&mut self.pending_path);
        if let Some(path) = path {
            job::dispatch_blocking(move |editor, _| {
                let Some(doc) = editor.document_mut(doc_id) else {
                    return;
                };
                let result = FileBlame::try_new(path);
                doc.file_blame = Some(result);
                if !editor.config().inline_blame.auto_fetch {
                    if let Some(line) = line_blame {
                        crate::commands::blame_line_impl(editor, doc_id, line);
                    } else {
                        editor.set_status("Blame for this file is now available")
                    }
                }
            });
        }
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
                        },
                    );
                }
            }
        }
        Ok(())
    });
}
