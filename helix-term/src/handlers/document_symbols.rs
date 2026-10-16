use std::time::Duration;

use helix_core::syntax::config::LanguageServerFeature;
use helix_core::{syntax::QueryMatchIterEvent, RopeSlice};
use helix_event::{cancelable_future, register_hook, send_blocking};
use helix_lsp::lsp::{self, DocumentSymbolResponse};
use helix_lsp::OffsetEncoding;
use helix_view::document::ThinDocumentSymbol;
use helix_view::{
    document::Mode,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidOpen, LanguageServerExited,
        LanguageServerInitialized, SelectionDidChange,
    },
    handlers::{lsp::DocumentSymbolsEvent, Handlers},
    DocumentId, Editor,
};
use tokio::time::Instant;

use crate::{events::OnModeSwitch, job};

const DOCUMENT_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct DocumentSymbolsHandler {
    docs: Vec<DocumentId>,
}

impl helix_event::AsyncHook for DocumentSymbolsHandler {
    type Event = DocumentSymbolsEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let DocumentSymbolsEvent(doc_id) = event;
        if !self.docs.contains(&doc_id) {
            self.docs.push(doc_id);
        }
        Some(Instant::now() + DOCUMENT_CHANGE_DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        let docs = std::mem::take(&mut self.docs);

        job::dispatch_blocking(move |editor, _compositor| {
            for doc_id in docs {
                update_document_symbols(editor, doc_id);
            }
        });
    }
}

/// Refresh the cached document symbols using the best available source:
/// a hierarchical LSP if one supports document symbols, otherwise tree-sitter
/// tags (or a flat LSP response, which is nested locally).
fn update_document_symbols(editor: &mut Editor, doc_id: DocumentId) {
    if !editor.config().breadcrumb.enable {
        return;
    }

    // Avoid extra latency while typing; leaving insert mode re-requests.
    if editor.mode() == Mode::Insert {
        return;
    }

    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    if doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
        .is_some()
    {
        request_document_symbols(editor, doc_id);
    } else {
        compute_tree_sitter_symbols(editor, doc_id);
    }
}

/// Request symbols from the first LSP server supporting `DocumentSymbols`.
fn request_document_symbols(editor: &mut Editor, doc_id: DocumentId) {
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    // Get the first LSP Server that supports `DocumentSymbols`.
    let Some(language_server) = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
    else {
        return;
    };

    let offset_encoding = language_server.offset_encoding();
    let Some(future) = language_server.document_symbols(doc.identifier()) else {
        return;
    };
    let cancel = doc.document_symbols_controller.restart();

    tokio::spawn(async move {
        let Some(Ok(Some(response))) = cancelable_future(future, &cancel).await else {
            return;
        };

        job::dispatch(move |editor, _| {
            let Some(doc) = editor.document_mut(doc_id) else {
                return;
            };
            match response {
                DocumentSymbolResponse::Nested(symbols) => {
                    doc.set_document_symbols(symbols, offset_encoding);
                }
                DocumentSymbolResponse::Flat(symbols) => {
                    // Servers without hierarchical support still provide
                    // enough information; nest the flat list locally.
                    let tree = symbols
                        .into_iter()
                        .map(|info| ThinDocumentSymbol {
                            name: info.name.into(),
                            kind: info.kind,
                            range: info.location.range,
                            children: None,
                        })
                        .collect();
                    doc.set_symbol_tree(ThinDocumentSymbol::nest_flat(tree), offset_encoding);
                }
            }
            update_breadcrumbs_for_all_views(editor, doc_id);
        })
        .await;
    });
}

/// Compute symbols synchronously from tree-sitter tag queries (`tags.scm`).
/// The resulting tags are flat, so they are nested by range containment.
fn compute_tree_sitter_symbols(editor: &mut Editor, doc_id: DocumentId) {
    let flat = {
        let loader = editor.syn_loader.load();

        let Some(doc) = editor.document_mut(doc_id) else {
            return;
        };
        let Some(syntax) = doc.syntax() else {
            return;
        };
        let text = doc.text().clone();

        let mut flat: Vec<ThinDocumentSymbol> = Vec::new();
        let mut tags_iter = syntax.tags(text.slice(..), &loader, ..);

        while let Some(event) = tags_iter.next() {
            let QueryMatchIterEvent::Match(mat) = event else {
                continue;
            };
            let query = match loader.tag_query(tags_iter.current_language()) {
                Some(tag_query) => &tag_query.query,
                None => continue,
            };

            // Find the @definition.* and optional @name captures in this match.
            let mut def_capture = None::<(lsp::SymbolKind, std::ops::Range<u32>)>;
            let mut name_range = None::<std::ops::Range<u32>>;
            let name_capture = query.get_capture("name");

            for node in mat.nodes.iter() {
                let capture_name = query.capture_name(node.capture);
                if let Some(kind) = capture_name
                    .strip_prefix("definition.")
                    .and_then(tag_kind_to_symbol_kind)
                {
                    def_capture = Some((kind, node.node.byte_range()));
                } else if name_capture == Some(node.capture) {
                    name_range = Some(node.node.byte_range());
                }
            }

            let Some((kind, def_byte_range)) = def_capture else {
                continue;
            };
            let name_byte_range = name_range.unwrap_or_else(|| def_byte_range.clone());

            let text_slice: RopeSlice = text.slice(..);
            let name_start = text_slice.byte_to_char(name_byte_range.start as usize);
            let name_end = text_slice.byte_to_char(name_byte_range.end as usize);
            let def_start = text_slice.byte_to_char(def_byte_range.start as usize);
            let def_end = text_slice.byte_to_char(def_byte_range.end as usize);

            let name = text_slice.slice(name_start..name_end).to_string();

            // lsp positions use utf-8 columns here; the cache stores the matching
            // offset encoding so cursor comparisons stay consistent.
            let (start_line, start_character) = char_pos(&text, def_start);
            let (end_line, end_character) = char_pos(&text, def_end);

            flat.push(ThinDocumentSymbol {
                name: name.into(),
                kind,
                range: lsp::Range {
                    start: lsp::Position {
                        line: start_line as u32,
                        character: start_character as u32,
                    },
                    end: lsp::Position {
                        line: end_line as u32,
                        character: end_character as u32,
                    },
                },
                children: None,
            });
        }

        flat
    };

    if let Some(doc) = editor.document_mut(doc_id) {
        doc.set_symbol_tree(ThinDocumentSymbol::nest_flat(flat), OffsetEncoding::Utf8);
    }
    update_breadcrumbs_for_all_views(editor, doc_id);
}

/// Convert a char index into `(line, character)` with utf-8 columns.
fn char_pos(text: &helix_core::Rope, char_idx: usize) -> (usize, usize) {
    let line = text.char_to_line(char_idx);
    (line, char_idx - text.line_to_char(line))
}

/// Map a tree-sitter `tags.scm` definition kind to an LSP symbol kind so the
/// breadcrumb rendering/theming stays shared between both sources.
fn tag_kind_to_symbol_kind(name: &str) -> Option<lsp::SymbolKind> {
    use lsp::SymbolKind;
    Some(match name {
        "class" | "type" => SymbolKind::CLASS,
        "constant" => SymbolKind::CONSTANT,
        "enum" => SymbolKind::ENUM,
        "field" => SymbolKind::FIELD,
        "function" | "macro" => SymbolKind::FUNCTION,
        "interface" => SymbolKind::INTERFACE,
        "module" | "section" => SymbolKind::MODULE,
        "struct" => SymbolKind::STRUCT,
        _ => return None,
    })
}

/// Recompute the breadcrumb trail for every view showing this document.
fn update_breadcrumbs_for_all_views(editor: &mut Editor, doc_id: DocumentId) {
    let view_ids: Vec<_> = editor
        .tree
        .views()
        .filter(|(view, _)| view.doc == doc_id)
        .map(|(view, _)| view.id)
        .collect();
    if let Some(doc) = editor.document_mut(doc_id) {
        for view_id in view_ids {
            doc.update_breadcrumbs_for_view(view_id);
        }
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        // Only gather symbols here. The breadcrumb trail itself is computed
        // when a view attaches to the document (`ensure_view_init`), since the
        // focused view at open time may belong to a different document.
        update_document_symbols(event.editor, event.doc);
        Ok(())
    });

    let tx = handlers.document_symbols.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if !event.ghost_transaction {
            // Cancel the ongoing request, if present.
            event.doc.document_symbols_controller.cancel();
            send_blocking(&tx, DocumentSymbolsEvent(event.doc.id()));
        }
        // Recompute the trail from the cached symbols for immediate feedback.
        if event.doc.config.load().breadcrumb.enable {
            let view_id = event.view;
            event.doc.update_breadcrumbs_for_view(view_id);
        }
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();
        for doc_id in doc_ids {
            update_document_symbols(event.editor, doc_id);
        }
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.clear_document_symbols();
            }
        }
        Ok(())
    });

    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !event.old.breadcrumb.enable && event.new.breadcrumb.enable {
            let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();
            for doc_id in doc_ids {
                update_document_symbols(event.editor, doc_id);
            }
            return Ok(());
        }

        if event.old.breadcrumb.enable && !event.new.breadcrumb.enable {
            for doc in event.editor.documents_mut() {
                doc.clear_document_symbols();
            }
        }

        Ok(())
    });

    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        // Walking the cached symbol tree is cheap, so keep the trail live on
        // every cursor move without any debouncing.
        if event.doc.config.load().breadcrumb.enable {
            let view_id = event.view;
            event.doc.update_breadcrumbs_for_view(view_id);
        }
        Ok(())
    });

    let tx = handlers.document_symbols.clone();
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if event.old_mode == Mode::Insert {
            // Requests were skipped while typing; refresh now that the
            // document settled.
            let editor = &mut *event.cx.editor;
            let view_id = editor.tree.focus;
            let Some(view) = editor.tree.try_get(view_id) else {
                return Ok(());
            };
            let doc_id = view.doc;
            if let Some(doc) = editor.document_mut(doc_id) {
                doc.update_breadcrumbs_for_view(view_id);
            }
            send_blocking(&tx, DocumentSymbolsEvent(doc_id));
        }
        Ok(())
    });
}
