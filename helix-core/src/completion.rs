use std::borrow::Cow;

use crate::{diagnostic::LanguageServerId, snippets::RenderedSnippet, Transaction};

#[derive(Debug, PartialEq, Clone)]
pub struct CompletionItem {
    pub transaction: Transaction,
    pub label: Cow<'static, str>,
    pub kind: Cow<'static, str>,
    /// Containing Markdown
    pub documentation: Option<String>,
    /// Set for snippet items: the tabstops of the text `transaction` inserts, so accepting the
    /// completion can start a snippet session.
    pub snippet: Option<RenderedSnippet>,
    pub provider: CompletionProvider,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum CompletionProvider {
    Lsp(LanguageServerId),
    Path,
    Word,
    Snippet,
}

impl From<LanguageServerId> for CompletionProvider {
    fn from(id: LanguageServerId) -> Self {
        CompletionProvider::Lsp(id)
    }
}
