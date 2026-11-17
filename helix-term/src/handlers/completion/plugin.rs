//! The completion sources of plugins (`helix.completion.register_source`): asked on the
//! editor's thread (plugin code runs there), their items join the menu as one response.

use std::{borrow::Cow, sync::Arc};

use helix_core::{self as core, chars::char_is_word, completion::CompletionProvider, Transaction};
use helix_view::{
    document::SavePoint,
    handlers::completion::ResponseContext,
    plugin_ui::{PluginCompletionItem, PluginCompletionRequest},
    Editor,
};

use super::{request::TriggerKind, CompletionItem, CompletionItems, CompletionResponse, Trigger};

/// Each source's name with its items.
pub(super) type SourceItems = Vec<(String, Vec<PluginCompletionItem>)>;

/// What the plugins' sources gave for the cursor of the focused buffer, and the length (in
/// chars) of the word their items replace. `None` without a source that wants it.
pub(super) fn ask(editor: &mut Editor, trigger: Trigger) -> Option<(SourceItems, usize)> {
    if editor.plugin_completion_sources.is_empty() {
        return None;
    }
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let line = text.char_to_line(cursor);
    let line_start = text.line_to_char(line);
    let before: String = text.slice(line_start..cursor).into();
    let prefix_len = before
        .chars()
        .rev()
        .take_while(|c| char_is_word(*c))
        .count();
    let prefix: String = before
        .chars()
        .skip(before.chars().count() - prefix_len)
        .collect();
    let request = PluginCompletionRequest {
        document: doc.id(),
        line,
        col: cursor - line_start,
        prefix,
        before,
        manual: trigger.kind == TriggerKind::Manual,
    };
    let mut hook = editor.plugin_completion_hook.take()?;
    let results = hook(editor, &request);
    editor.plugin_completion_hook.get_or_insert(hook);
    let any = results.iter().any(|(_, items)| !items.is_empty());
    any.then_some((results, prefix_len))
}

/// The response of the plugins' items: each replaces the word before every cursor.
pub(super) fn response(
    editor: &Editor,
    results: Vec<(String, Vec<PluginCompletionItem>)>,
    prefix_len: usize,
    savepoint: Arc<SavePoint>,
) -> CompletionResponse {
    let (view, doc) = current_ref!(editor);
    let rope = doc.text().clone();
    let text = rope.slice(..);
    let selection = doc.selection(view.id).clone();
    let items = results
        .into_iter()
        .flat_map(|(source, items)| items.into_iter().map(move |item| (source.clone(), item)))
        .map(|(source, item)| {
            let insert = item.insert.unwrap_or_else(|| item.label.clone());
            let transaction = Transaction::change_by_selection(&rope, &selection, |range| {
                let cursor = range.cursor(text);
                (
                    cursor - prefix_len.min(cursor),
                    cursor,
                    Some(insert.as_str().into()),
                )
            });
            CompletionItem::Other(core::CompletionItem {
                transaction,
                label: Cow::Owned(item.label),
                kind: Cow::Owned(item.kind.unwrap_or(source)),
                documentation: item.documentation,
                snippet: None,
                provider: CompletionProvider::Plugin,
            })
        })
        .collect();
    CompletionResponse {
        items: CompletionItems::Other(items),
        provider: CompletionProvider::Plugin,
        context: ResponseContext {
            is_incomplete: false,
            priority: 0,
            savepoint,
        },
    }
}
