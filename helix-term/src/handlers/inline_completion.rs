//! Requests inline completion (ghost text) suggestions while typing.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use helix_core::syntax::config::LanguageServerFeature;
use helix_event::{cancelable_future, register_hook, send_blocking};
use helix_lsp::{lsp, util::pos_to_lsp_pos};
use helix_view::{
    document::Mode,
    events::{DocumentDidChange, DocumentFocusLost},
    handlers::{lsp::InlineCompletionEvent, Handlers},
    inline_completion::InlineCompletion,
    DocumentId, Editor, ViewId,
};
use tokio::time::Instant;

use crate::{compositor::Compositor, config::Config, events::OnModeSwitch, job, ui::EditorView};

pub(super) struct InlineCompletionHandler {
    config: Arc<ArcSwap<Config>>,
    pending: Option<InlineCompletionEvent>,
}

impl InlineCompletionHandler {
    pub fn new(config: Arc<ArcSwap<Config>>) -> Self {
        Self {
            config,
            pending: None,
        }
    }
}

impl helix_event::AsyncHook for InlineCompletionHandler {
    type Event = InlineCompletionEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let manual = event.manual;
        self.pending = Some(event);
        if manual {
            self.finish_debounce();
            None
        } else {
            let delay: Duration = self.config.load().editor.inline_completion.trigger_delay;
            Some(Instant::now() + delay)
        }
    }

    fn finish_debounce(&mut self) {
        let Some(event) = self.pending.take() else {
            return;
        };
        job::dispatch_blocking(move |editor, compositor| {
            request_inline_completion(editor, compositor, event);
        });
    }
}

fn request_inline_completion(
    editor: &mut Editor,
    compositor: &mut Compositor,
    InlineCompletionEvent { doc, view, manual }: InlineCompletionEvent,
) {
    if editor.mode() != Mode::Insert || !editor.tree.contains(view) {
        return;
    }
    // Suggestions pause while a completion menu is open.
    if compositor
        .find::<EditorView>()
        .is_some_and(|editor_view| editor_view.completion.is_some())
    {
        return;
    }
    let Some(document) = editor.documents.get_mut(&doc) else {
        return;
    };
    let selection = document.selection(view);
    // Only a single cursor is supported.
    if selection.len() != 1 {
        return;
    }
    let text = document.text().clone();
    let cursor = selection.primary().cursor(text.slice(..));
    let version = document.version();

    let Some(server) = document
        .language_servers_with_feature(LanguageServerFeature::InlineCompletion)
        .next()
    else {
        return;
    };
    let offset_encoding = server.offset_encoding();
    let server_id = server.id();
    let trigger_kind = if manual {
        lsp::InlineCompletionTriggerKind::Invoked
    } else {
        lsp::InlineCompletionTriggerKind::Automatic
    };
    let Some(future) = server.inline_completion(
        document.identifier(),
        pos_to_lsp_pos(&text, cursor, offset_encoding),
        trigger_kind,
    ) else {
        return;
    };

    // Starting a request cancels the previous one.
    let cancel = document.inline_completions.controller.restart();
    tokio::spawn(async move {
        let response = match cancelable_future(future, &cancel).await {
            Some(Ok(response)) => response,
            Some(Err(err)) => {
                log::debug!("inline completion request failed: {err}");
                return;
            }
            None => return,
        };
        let items = match response {
            Some(lsp::InlineCompletionResponse::Array(items)) => items,
            Some(lsp::InlineCompletionResponse::List(list)) => list.items,
            None => Vec::new(),
        };
        let suggestions: Vec<InlineCompletion> = items
            .into_iter()
            .filter_map(|item| {
                InlineCompletion::from_lsp(&text, cursor, item, offset_encoding, server_id)
            })
            .collect();
        job::dispatch(move |editor, _| attach(editor, doc, view, version, cursor, suggestions))
            .await;
    });
}

fn attach(
    editor: &mut Editor,
    doc: DocumentId,
    view: ViewId,
    version: i32,
    cursor: usize,
    suggestions: Vec<InlineCompletion>,
) {
    if editor.mode() != Mode::Insert || !editor.tree.contains(view) {
        return;
    }
    let Some(document) = editor.documents.get_mut(&doc) else {
        return;
    };
    // Typing or moving since the request makes the answer stale.
    let selection = document.selection(view);
    if document.version() != version
        || selection.len() != 1
        || selection.primary().cursor(document.text().slice(..)) != cursor
    {
        return;
    }
    document.inline_completions.set(view, version, suggestions);
    helix_event::request_redraw();
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.inline_completion.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // The suggestion no longer matches the text.
        event.doc.inline_completions.clear();
        if !event.ghost_transaction && event.doc.config.load().inline_completion.auto_trigger {
            send_blocking(
                &tx,
                InlineCompletionEvent {
                    doc: event.doc.id(),
                    view: event.view,
                    manual: false,
                },
            );
        }
        Ok(())
    });

    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if event.old_mode == Mode::Insert {
            let editor = &mut *event.cx.editor;
            for doc in editor.documents.values_mut() {
                doc.inline_completions.clear();
            }
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.documents.get_mut(&event.doc) {
            doc.inline_completions.clear();
        }
        Ok(())
    });
}
