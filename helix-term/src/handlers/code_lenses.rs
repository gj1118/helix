//! LSP code lenses (`textDocument/codeLens`): "Run", "Debug", "3 references" labels the
//! language server attaches to lines. Everything here is behind `editor.lsp.display-code-lenses`.

use std::{collections::HashSet, sync::Arc, time::Duration};

use futures_util::{stream::FuturesUnordered, StreamExt};
use helix_core::{syntax::config::LanguageServerFeature, Assoc};
use helix_event::{cancelable_future, register_hook};
use helix_lsp::{lsp, Client, LanguageServerId};
use helix_view::{
    document::CodeLens,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidOpen, LanguageServerExited,
        LanguageServerInitialized,
    },
    handlers::{lsp::CodeLensesEvent, Handlers},
    DocumentId, Editor,
};
use tokio::{sync::Semaphore, time::Instant};

use crate::job;

const DOCUMENT_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

/// How many `codeLens/resolve` requests may be in flight at once. A file can have hundreds of
/// lenses and servers resolve them one at a time anyway.
const MAX_CONCURRENT_RESOLVES: usize = 4;
static RESOLVE_SLOTS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_RESOLVES);

#[derive(Default)]
pub(super) struct CodeLensesHandler {
    docs: HashSet<DocumentId>,
}

impl helix_event::AsyncHook for CodeLensesHandler {
    type Event = CodeLensesEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let CodeLensesEvent(doc_id) = event;
        self.docs.insert(doc_id);
        Some(Instant::now() + DOCUMENT_CHANGE_DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        let docs = std::mem::take(&mut self.docs);

        job::dispatch_blocking(move |editor, _compositor| {
            for doc in docs {
                request_code_lenses(editor, doc);
            }
        });
    }
}

pub fn enabled(editor: &Editor) -> bool {
    let config = editor.config();
    config.lsp.enable && config.lsp.display_code_lenses
}

/// Asks every server that offers code lenses for a document's lenses.
fn request_code_lenses(editor: &mut Editor, doc_id: DocumentId) {
    if !enabled(editor) {
        return;
    }
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    let cancel = doc.code_lens_controller.restart();
    // Resolve requests refer to indices in the cache that is about to be replaced.
    doc.code_lens_resolve_controller.cancel();
    let version = doc.version();

    let mut seen_language_servers = HashSet::new();
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::CodeLens)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .filter_map(|language_server| {
            let text = doc.text().clone();
            let offset_encoding = language_server.offset_encoding();
            let language_server_id = language_server.id();
            let future = language_server.text_document_code_lens(doc.identifier())?;

            Some(async move {
                let lenses = future.await?.unwrap_or_default();
                let lenses: Vec<_> = lenses
                    .into_iter()
                    .filter_map(|lens| {
                        let char_idx = helix_lsp::util::lsp_pos_to_pos(
                            &text,
                            lens.range.start,
                            offset_encoding,
                        )?;
                        Some(CodeLens {
                            char_idx,
                            lens,
                            language_server_id,
                            resolve_attempted: false,
                        })
                    })
                    .collect();
                anyhow::Ok(lenses)
            })
        })
        .collect();

    if futures.is_empty() {
        doc.clear_code_lenses();
        return;
    }

    tokio::spawn(async move {
        let mut all_lenses = Vec::new();
        loop {
            match cancelable_future(futures.next(), &cancel).await {
                Some(Some(Ok(items))) => all_lenses.extend(items),
                Some(Some(Err(err))) => log::error!("code lens request failed: {err}"),
                Some(None) => break,
                None => return,
            }
        }
        // Same line, same order: lenses of one server keep the order they were sent in.
        all_lenses.sort_by_key(|lens| (lens.char_idx, lens.language_server_id));

        job::dispatch(move |editor, _| {
            attach_code_lenses(editor, doc_id, version, all_lenses, cancel.is_canceled());
        })
        .await;
    });
}

fn attach_code_lenses(
    editor: &mut Editor,
    doc_id: DocumentId,
    requested_version: i32,
    lenses: Vec<CodeLens>,
    canceled: bool,
) {
    if canceled || !enabled(editor) {
        return;
    }
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    // The text changed while the server was busy: the positions are for another text. The
    // change already scheduled a new request.
    if doc.version() != requested_version {
        return;
    }
    doc.code_lenses = lenses;
    doc.code_lenses_version = Some(requested_version);
    doc.code_lens_resolve_handle = Some(doc.code_lens_resolve_controller.restart());
    editor.needs_redraw = true;
    resolve_visible_code_lenses(editor);
}

/// Asks the server for the command of the lenses that are on screen and came without one.
///
/// Called when lenses arrive and before every frame, so scrolling reveals titles; a lens is only
/// ever asked for once.
pub fn resolve_visible_code_lenses(editor: &mut Editor) {
    if !enabled(editor) {
        return;
    }

    let mut wanted: Vec<(DocumentId, usize)> = Vec::new();
    for (view, _) in editor.tree.views() {
        let Some(doc) = editor.documents.get(&view.doc) else {
            continue;
        };
        if !doc.code_lenses_are_current() {
            continue;
        }
        let visible = view.line_range(doc);
        for (index, lens) in doc.code_lenses.iter().enumerate() {
            if lens.lens.command.is_some() || lens.resolve_attempted {
                continue;
            }
            let line = doc
                .text()
                .char_to_line(lens.char_idx.min(doc.text().len_chars()));
            if visible.contains(&line) && !wanted.contains(&(view.doc, index)) {
                wanted.push((view.doc, index));
            }
        }
    }

    for (doc_id, index) in wanted {
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            continue;
        };
        let version = doc.version();
        // One handle for all resolves of this cache: `restart` would cancel the earlier ones.
        let Some(cancel) = doc
            .code_lens_resolve_handle
            .clone()
            .filter(|handle| !handle.is_canceled())
        else {
            continue;
        };
        let lens = &mut doc.code_lenses[index];
        lens.resolve_attempted = true;
        let server_id = lens.language_server_id;
        let unresolved = lens.lens.clone();
        let Some(server) = editor.language_servers.get_by_id(server_id).cloned() else {
            continue;
        };
        if !server.code_lens_resolve_provider() {
            continue;
        }
        tokio::spawn(async move {
            let resolved = cancelable_future(resolve_lens(server, unresolved.clone()), &cancel)
                .await
                .and_then(|result| match result {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        log::error!("code lens resolve failed: {err}");
                        None
                    }
                });
            let Some(resolved) = resolved else { return };

            job::dispatch(move |editor, _| {
                if cancel.is_canceled() || !enabled(editor) {
                    return;
                }
                let Some(doc) = editor.documents.get_mut(&doc_id) else {
                    return;
                };
                if doc.version() != version {
                    return;
                }
                let Some(lens) = doc.code_lenses.get_mut(index) else {
                    return;
                };
                // Same lens: a server must not move it by resolving.
                if lens.lens.range == resolved.range {
                    lens.lens = resolved;
                    editor.needs_redraw = true;
                }
            })
            .await;
        });
    }
}

/// `codeLens/resolve`, limited to [`MAX_CONCURRENT_RESOLVES`] at a time. `None` when the server
/// answered without a command.
pub(crate) async fn resolve_lens(
    server: Arc<Client>,
    lens: lsp::CodeLens,
) -> helix_lsp::Result<Option<lsp::CodeLens>> {
    if lens.command.is_some() {
        return Ok(Some(lens));
    }
    // `Client::call` sends the request when it is called, not when it is polled: take the slot
    // first, otherwise all requests go out at once.
    let _slot = RESOLVE_SLOTS
        .acquire()
        .await
        .expect("the semaphore is never closed");
    let Some(future) = server.resolve_code_lens(lens) else {
        return Ok(None);
    };
    let resolved = future.await?;
    Ok(resolved.command.is_some().then_some(resolved))
}

/// `workspace/codeLens/refresh`: the server's answers changed, e.g. after it finished indexing.
pub fn refresh_for_language_server(editor: &mut Editor, server_id: LanguageServerId) {
    let docs: Vec<_> = editor
        .documents()
        .filter(|doc| doc.supports_language_server(server_id))
        .map(|doc| doc.id())
        .collect();
    for doc in docs {
        request_code_lenses(editor, doc);
    }
}

fn request_all(editor: &mut Editor) {
    let docs: Vec<_> = editor.documents().map(|doc| doc.id()).collect();
    for doc in docs {
        request_code_lenses(editor, doc);
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request_code_lenses(event.editor, event.doc);
        Ok(())
    });

    let tx = handlers.code_lenses.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // Keep the labels where they are while the debounce runs. Their commands and resolve data
        // are for the old text, which is why `code_lenses_version` stops matching.
        event.changes.update_positions(
            event
                .doc
                .code_lenses
                .iter_mut()
                .map(|lens| (&mut lens.char_idx, Assoc::After)),
        );

        if !event.ghost_transaction {
            event.doc.code_lens_controller.cancel();
            event.doc.code_lens_resolve_controller.cancel();
            helix_event::send_blocking(&tx, CodeLensesEvent(event.doc.id()));
        }

        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        request_all(event.editor);
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.code_lenses
                    .retain(|lens| lens.language_server_id != event.server_id);
            }
        }
        request_all(event.editor);
        Ok(())
    });

    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        let was = event.old.lsp.enable && event.old.lsp.display_code_lenses;
        let is = event.new.lsp.enable && event.new.lsp.display_code_lenses;
        if was && !is {
            for doc in event.editor.documents_mut() {
                doc.clear_code_lenses();
            }
        } else if !was && is {
            request_all(event.editor);
        }
        Ok(())
    });
}
