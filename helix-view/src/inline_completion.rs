//! Inline completion: ghost-text suggestions from a language server
//! (`textDocument/inlineCompletion`) that are only inserted when accepted.

use helix_core::{line_ending::line_end_char_index, Rope};
use helix_event::TaskController;
use helix_lsp::{lsp, util::lsp_range_to_range, LanguageServerId, OffsetEncoding};

use crate::ViewId;

/// One suggestion, anchored at the cursor it was requested for.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineCompletion {
    /// Char index the suggestion is anchored at.
    pub cursor: usize,
    /// The text inserted at `cursor` when the suggestion is accepted.
    pub text: String,
    /// Preview rows: the lines of `text`, the last one followed by the
    /// unchanged rest of the cursor's line.
    pub lines: Vec<String>,
    pub server: LanguageServerId,
    /// Command the server wants executed after the suggestion is inserted.
    pub command: Option<lsp::Command>,
}

impl InlineCompletion {
    /// Turns a server item into a suggestion at `cursor`.
    ///
    /// The replaced range has to cover the text typed so far (a prefix of the
    /// suggestion) and may include text after the cursor that the suggestion
    /// already ends with. Anything else (snippets, other ranges, nothing new
    /// to show) is not supported and yields `None`.
    pub fn from_lsp(
        text: &Rope,
        cursor: usize,
        item: lsp::InlineCompletionItem,
        offset_encoding: OffsetEncoding,
        server: LanguageServerId,
    ) -> Option<Self> {
        if item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET) {
            return None;
        }
        let slice = text.slice(..);
        let line = slice.char_to_line(cursor);
        let line_end = line_end_char_index(&slice, line);

        let (start, end) = match &item.range {
            Some(range) => {
                let range = lsp_range_to_range(text, *range, offset_encoding)?;
                (range.from(), range.to())
            }
            None => (cursor, cursor),
        };
        if start > cursor || end < cursor || end > line_end || slice.char_to_line(start) != line {
            return None;
        }

        let typed: String = slice.slice(start..cursor).into();
        let existing_suffix: String = slice.slice(cursor..end).into();
        let rest = item.insert_text.strip_prefix(typed.as_str())?;
        let ghost = rest.strip_suffix(existing_suffix.as_str())?;
        if ghost.is_empty() {
            return None;
        }

        let rest_of_line: String = slice.slice(cursor..line_end).into();
        let mut lines: Vec<String> = ghost.split('\n').map(str::to_owned).collect();
        lines.last_mut()?.push_str(&rest_of_line);
        // CRLF suggestions: the preview never shows the carriage return.
        for line in &mut lines {
            if line.ends_with('\r') {
                line.pop();
            }
        }

        Some(Self {
            cursor,
            text: ghost.to_owned(),
            lines,
            server,
            command: item.command,
        })
    }
}

/// The suggestions for a document and the view they were requested from.
#[derive(Default)]
pub struct InlineCompletions {
    items: Vec<InlineCompletion>,
    index: usize,
    view: Option<ViewId>,
    /// Document version the suggestions were computed for.
    version: i32,
    /// Cancels the request in flight.
    pub controller: TaskController,
}

impl InlineCompletions {
    pub fn set(&mut self, view: ViewId, version: i32, items: Vec<InlineCompletion>) {
        self.index = 0;
        self.view = (!items.is_empty()).then_some(view);
        self.version = version;
        self.items = items;
    }

    /// The suggestion to show, provided it is still valid for this document
    /// `version` and `cursor`.
    pub fn current(&self, view: ViewId, version: i32, cursor: usize) -> Option<&InlineCompletion> {
        if self.view != Some(view) || self.version != version {
            return None;
        }
        self.items
            .get(self.index)
            .filter(|item| item.cursor == cursor)
    }

    /// Drops the suggestions and cancels the request in flight.
    pub fn clear(&mut self) {
        self.controller.cancel();
        self.items.clear();
        self.index = 0;
        self.view = None;
    }

    /// Shows the next (or previous) alternative.
    pub fn cycle(&mut self, view: ViewId, forward: bool) {
        if self.view != Some(view) || self.items.is_empty() {
            return;
        }
        let len = self.items.len();
        self.index = if forward {
            (self.index + 1) % len
        } else {
            (self.index + len - 1) % len
        };
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(insert_text: &str, range: Option<(u32, u32, u32, u32)>) -> lsp::InlineCompletionItem {
        lsp::InlineCompletionItem {
            insert_text: insert_text.into(),
            filter_text: None,
            range: range.map(|(sl, sc, el, ec)| {
                lsp::Range::new(lsp::Position::new(sl, sc), lsp::Position::new(el, ec))
            }),
            command: None,
            insert_text_format: None,
        }
    }

    fn build(
        text: &str,
        cursor: usize,
        item: lsp::InlineCompletionItem,
    ) -> Option<InlineCompletion> {
        InlineCompletion::from_lsp(
            &Rope::from(text),
            cursor,
            item,
            OffsetEncoding::Utf8,
            LanguageServerId::default(),
        )
    }

    #[test]
    fn plain_insertion_at_the_cursor() {
        let c = build("let x = ", 8, item("foo(bar)", None)).unwrap();
        assert_eq!(c.text, "foo(bar)");
        assert_eq!(c.lines, ["foo(bar)"]);
    }

    #[test]
    fn replacement_range_starting_before_the_cursor_shows_only_the_rest() {
        // "fn ma" typed, the server suggests "fn main() {" for the range 0..5
        let c = build("fn ma", 5, item("fn main() {", Some((0, 0, 0, 5)))).unwrap();
        assert_eq!(c.text, "in() {");
        assert_eq!(c.cursor, 5);
    }

    #[test]
    fn suggestion_not_extending_the_typed_text_is_rejected() {
        assert!(build("fn ma", 5, item("let x", Some((0, 0, 0, 5)))).is_none());
    }

    #[test]
    fn text_after_the_cursor_that_the_suggestion_ends_with_is_kept() {
        // cursor between the parentheses: "foo(|)"
        let c = build("foo()", 4, item("foo(a, b)", Some((0, 0, 0, 5)))).unwrap();
        assert_eq!(c.text, "a, b");
        assert_eq!(c.lines, ["a, b)"]);
    }

    #[test]
    fn multi_line_suggestion_keeps_the_rest_of_the_line_on_the_last_row() {
        let c = build("if x {} // tail", 6, item("\n    body\n", None)).unwrap();
        assert_eq!(c.lines.len(), 3);
        assert_eq!(c.lines[2], "} // tail");
    }

    #[test]
    fn snippets_and_empty_suggestions_are_not_supported() {
        let mut snippet = item("foo($1)", None);
        snippet.insert_text_format = Some(lsp::InsertTextFormat::SNIPPET);
        assert!(build("", 0, snippet).is_none());
        assert!(build("abc", 3, item("abc", Some((0, 0, 0, 3)))).is_none());
    }

    #[test]
    fn cycling_and_validity() {
        let mut state = InlineCompletions::default();
        let view = ViewId::default();
        let a = build("x", 1, item("y", None)).unwrap();
        let b = build("x", 1, item("z", None)).unwrap();
        state.set(view, 3, vec![a, b]);
        assert_eq!(state.current(view, 3, 1).unwrap().text, "y");
        // stale: edited document or moved cursor
        assert!(state.current(view, 4, 1).is_none());
        assert!(state.current(view, 3, 2).is_none());
        state.cycle(view, true);
        assert_eq!(state.current(view, 3, 1).unwrap().text, "z");
        state.cycle(view, true);
        assert_eq!(state.current(view, 3, 1).unwrap().text, "y");
        state.cycle(view, false);
        assert_eq!(state.current(view, 3, 1).unwrap().text, "z");
        state.clear();
        assert!(state.current(view, 3, 1).is_none());
    }
}
