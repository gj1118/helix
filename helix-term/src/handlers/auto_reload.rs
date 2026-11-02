use std::fs;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use helix_event::{register_hook, send_blocking};
use helix_view::document::Mode;
use helix_view::events::DocumentDidOpen;
use helix_view::handlers::{AutoReloadEvent, Handlers};
use helix_view::{Document, Editor};
use tokio::time::Instant;

use crate::commands;
use crate::events::OnModeSwitch;
use crate::job::{self, Jobs};

#[derive(Debug)]
pub(super) struct AutoReloadHandler {
    reload_pending: Arc<AtomicBool>,
}

impl AutoReloadHandler {
    pub fn new() -> AutoReloadHandler {
        AutoReloadHandler {
            reload_pending: Default::default(),
        }
    }
}

impl helix_event::AsyncHook for AutoReloadHandler {
    type Event = AutoReloadEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        existing_debounce: Option<Instant>,
    ) -> Option<Instant> {
        match event {
            Self::Event::CheckForChanges { after } => {
                Some(Instant::now() + Duration::from_millis(after))
            }
            Self::Event::LeftInsertMode | Self::Event::EditorFocused => {
                if existing_debounce.is_some() {
                    // If the event happened more recently than the debounce, let the
                    // debounce run down before checking for changes.
                    existing_debounce
                } else {
                    // Otherwise if there is a reload pending, check immediately.
                    if self.reload_pending.load(Ordering::Relaxed) {
                        self.finish_debounce();
                    }
                    None
                }
            }
        }
    }

    fn finish_debounce(&mut self) {
        let reload_pending = self.reload_pending.clone();
        job::dispatch_blocking(move |editor, _compositor| {
            if editor.mode() == Mode::Insert {
                // Avoid reloading while in insert mode since this mixes up
                // the modification indicator and prevents future saves.
                reload_pending.store(true, atomic::Ordering::Relaxed);
            } else {
                prompt_to_reload_if_needed(editor);
                reload_pending.store(false, atomic::Ordering::Relaxed);
            }
        });
    }
}

/// Requests a reload if any documents have been modified externally.
fn prompt_to_reload_if_needed(editor: &mut Editor) {
    // If there are externally modified documents, try to reload them
    if count_externally_modified_documents(editor.documents()) > 0 {
        let mut cx = crate::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut Jobs::new(),
            plugin_manager: None,
        };

        // Only the documents that changed on disk and have no unsaved edits: reloading
        // a modified buffer would replace the edits and mark it clean.
        match commands::typed::reload_documents(&mut cx, is_externally_modified) {
            Ok(_) => cx.editor.set_status("Reloaded modified documents"),
            Err(err) => cx
                .editor
                .set_error(format!("Failed to reload document: {err}")),
        }
    }

    // Reset the debounce timer to allow for the next check.
    let config = editor.config.load();
    if config.auto_reload.periodic.enable {
        let interval = config.auto_reload.periodic.interval;
        send_blocking(
            &editor.handlers.auto_reload,
            AutoReloadEvent::CheckForChanges { after: interval },
        );
    }
}

pub fn count_externally_modified_documents<'a>(docs: impl Iterator<Item = &'a Document>) -> usize {
    docs.filter(|doc| is_externally_modified(doc)).count()
}

/// Whether `doc` has no unsaved changes and its file changed on disk since it was last
/// read or written.
fn is_externally_modified(doc: &Document) -> bool {
    if doc.is_modified() {
        return false;
    }
    let Some(path) = doc.path() else {
        return false;
    };
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified > doc.get_last_saved_time())
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.auto_reload.clone();
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let config = event.editor.config.load();
        if config.auto_reload.periodic.enable {
            let interval = config.auto_reload.periodic.interval;
            send_blocking(&tx, AutoReloadEvent::CheckForChanges { after: interval });
        }
        Ok(())
    });

    let tx = handlers.auto_reload.clone();
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if event.old_mode == Mode::Insert {
            send_blocking(&tx, AutoReloadEvent::LeftInsertMode)
        }
        Ok(())
    });
}
