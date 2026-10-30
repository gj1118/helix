//! Completion of user-defined snippets by their prefixes.
//!
//! Typing (a start of) a snippet's prefix offers the snippet; accepting it replaces the typed
//! prefix and starts a snippet session like an LSP snippet completion does. A manual
//! completion request without anything typed lists every snippet with a prefix.

use std::{borrow::Cow, sync::Arc};

use helix_core::{
    self as core, chars::char_is_word, completion::CompletionProvider, Rope, Selection,
};
use helix_view::{
    document::{Mode, SavePoint},
    handlers::completion::ResponseContext,
    snippets::SnippetDef,
    Editor,
};

use super::{request::TriggerKind, CompletionItem, CompletionItems, CompletionResponse, Trigger};

const COMPLETION_KIND: &str = "snippet";

/// How many characters before the cursor are considered to be a typed prefix.
const MAX_PREFIX_CHARS: usize = 64;

/// The longest part of `prefix` that `before` (the text before the cursor) ends with, in
/// characters. A prefix that starts with a word character must start at a word boundary, so
/// `xfor` doesn't offer a `for` snippet.
fn typed_prefix_len(before: &[char], prefix: &str) -> usize {
    let prefix: Vec<char> = prefix.chars().collect();
    (1..=prefix.len().min(before.len()))
        .rev()
        .find(|&len| {
            let start = before.len() - len;
            before[start..] == prefix[..len]
                && !(char_is_word(prefix[0]) && start > 0 && char_is_word(before[start - 1]))
        })
        .unwrap_or(0)
}

/// The snippets to offer after `before`, each with the prefix to show and the number of typed
/// characters the snippet replaces.
fn matches<'a>(
    defs: impl Iterator<Item = &'a Arc<SnippetDef>>,
    before: &[char],
    manual: bool,
) -> Vec<(&'a Arc<SnippetDef>, &'a str, usize)> {
    let mut found = Vec::new();
    for def in defs {
        let best = def
            .prefixes
            .iter()
            .map(|prefix| (prefix.as_str(), typed_prefix_len(before, prefix)))
            .filter(|(_, len)| *len > 0)
            .max_by_key(|(_, len)| *len);
        match best {
            Some((prefix, len)) => found.push((def, prefix, len)),
            // A manual request with nothing to match lists everything.
            None if manual => {
                if let Some(prefix) = def.prefixes.first() {
                    found.push((def, prefix.as_str(), 0));
                }
            }
            None => {}
        }
    }
    found
}

fn documentation(def: &SnippetDef) -> String {
    let mut doc = String::new();
    if let Some(description) = &def.description {
        doc.push_str(description);
        doc.push_str("\n\n");
    }
    doc.push_str("```\n");
    doc.push_str(&def.body);
    doc.push_str("\n```");
    doc
}

pub(super) fn completion(
    editor: &Editor,
    trigger: Trigger,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse> {
    if !editor.config().snippet_completion || editor.snippets.is_empty() {
        return None;
    }
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    let selection: Selection = doc.selection(view.id).clone();
    let cursor = selection.primary().cursor(text);

    let line_start = text.line_to_char(text.char_to_line(cursor));
    let start = line_start.max(cursor.saturating_sub(MAX_PREFIX_CHARS));
    let before: Vec<char> = text.slice(start..cursor).chars().collect();

    let manual = trigger.kind == TriggerKind::Manual;
    let found = matches(
        editor.snippets.for_language(doc.language_name()),
        &before,
        manual,
    );
    if found.is_empty() {
        return None;
    }

    let rope: Rope = doc.text().clone();
    let items = found
        .into_iter()
        .filter_map(|(def, prefix, typed_len)| {
            let snippet = def.parse().ok()?;
            let typed: Vec<char> = before[before.len() - typed_len..].to_vec();
            let clipboard = def
                .body
                .contains("CLIPBOARD")
                .then(|| editor.registers.first('+', editor).map(Cow::into_owned))
                .flatten();
            let mut ctx = doc.snippet_ctx(view.id, Mode::Insert, clipboard);
            let slice = rope.slice(..);
            let (transaction, mapped, rendered) = snippet.render(
                &rope,
                &selection,
                |range| {
                    let cursor = range.cursor(slice);
                    // Only cursors that have the same text typed get it replaced.
                    let start = cursor.saturating_sub(typed_len);
                    let same = cursor >= typed_len
                        && slice.slice(start..cursor).chars().eq(typed.iter().copied());
                    (if same { start } else { cursor }, cursor)
                },
                &mut ctx,
            );
            let transaction = transaction.with_selection(
                rendered.first_selection(selection.primary().direction(), mapped.primary_index()),
            );
            Some(CompletionItem::Other(core::CompletionItem {
                transaction,
                label: prefix.to_owned().into(),
                kind: Cow::Borrowed(COMPLETION_KIND),
                documentation: Some(documentation(def)),
                snippet: Some(rendered),
                provider: CompletionProvider::Snippet,
            }))
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return None;
    }

    Some(move || CompletionResponse {
        items: CompletionItems::Other(items),
        provider: CompletionProvider::Snippet,
        context: ResponseContext {
            is_incomplete: false,
            priority: 0,
            savepoint,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn def(name: &str, prefixes: &[&str]) -> Arc<SnippetDef> {
        Arc::new(SnippetDef {
            name: name.into(),
            body: "x".into(),
            prefixes: prefixes.iter().map(|p| p.to_string()).collect(),
            scopes: Vec::new(),
            description: None,
            source: PathBuf::new(),
            workspace: false,
        })
    }

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn longest_typed_prefix() {
        assert_eq!(typed_prefix_len(&chars("fo"), "forloop"), 2);
        assert_eq!(typed_prefix_len(&chars("  fo"), "forloop"), 2);
        assert_eq!(typed_prefix_len(&chars("x.fo"), "forloop"), 2);
        // Not at a word boundary.
        assert_eq!(typed_prefix_len(&chars("xfo"), "forloop"), 0);
        // But a prefix starting with a symbol can follow anything.
        assert_eq!(typed_prefix_len(&chars("x#!"), "#!sh"), 2);
        assert_eq!(typed_prefix_len(&chars("ab"), "forloop"), 0);
        assert_eq!(typed_prefix_len(&chars(""), "forloop"), 0);
        // The whole prefix typed.
        assert_eq!(typed_prefix_len(&chars("a for"), "for"), 3);
    }

    #[test]
    fn matching_snippets() {
        let defs = [
            def("loop", &["forloop", "loop"]),
            def("fn", &["fn"]),
            def("bare", &[]),
        ];
        let found = matches(defs.iter(), &chars("  fo"), false);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].0.name.as_str(), found[0].1, found[0].2),
            ("loop", "forloop", 2)
        );

        // The prefix with the longest typed part wins.
        let found = matches(defs.iter(), &chars("loop"), false);
        assert_eq!((found[0].1, found[0].2), ("loop", 4));

        assert!(matches(defs.iter(), &chars(""), false).is_empty());
        // Manual request without matches lists the prefixed snippets.
        let found = matches(defs.iter(), &chars(""), true);
        assert_eq!(
            found.iter().map(|f| (f.1, f.2)).collect::<Vec<_>>(),
            [("forloop", 0), ("fn", 0)]
        );
    }
}
