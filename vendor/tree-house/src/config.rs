use once_cell::sync::Lazy;
use regex::Regex;
use tree_sitter::{query, Grammar};

use crate::highlighter::{Highlight, HighlightQuery};
use crate::injections_query::{InjectionLanguageMarker, InjectionsQuery};
use crate::Language;

use std::fmt::Write;

#[derive(Debug)]
pub struct LanguageConfig {
    pub grammar: Grammar,
    pub highlight_query: HighlightQuery,
    pub injection_query: InjectionsQuery,
}

impl LanguageConfig {
    pub fn new(
        grammar: Grammar,
        highlight_query_text: &str,
        injection_query_text: &str,
        local_query_text: &str,
    ) -> Result<Self, query::ParseError> {
        // PATCH (helix fork): the two queries are compiled at the same time. Analyzing a
        // large query takes tree-sitter a long time (Rust's: about 150 ms each), and the
        // editor waits for both before it shows the first file of a language.
        let compile_highlights =
            || HighlightQuery::new(grammar, highlight_query_text, local_query_text);
        let (injection_query, highlight_query) = std::thread::scope(|scope| {
            let highlights = std::thread::Builder::new()
                .name("highlight-query".into())
                .spawn_scoped(scope, compile_highlights);
            let injections = InjectionsQuery::new(grammar, injection_query_text, local_query_text);
            let highlights = match highlights {
                Ok(thread) => thread
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                // no thread to be had: one after the other
                Err(_) => compile_highlights(),
            };
            (injections, highlights)
        });
        // NOTE: the error of the injection queries comes first since the local query is passed
        // as-is to `Query::new` in `InjectionsQuery::new`. This ensures that the more readable
        // error bubbles up first if the locals queries have an issue.
        let injection_query = injection_query?;
        let highlight_query = highlight_query?;

        Ok(Self {
            grammar,
            highlight_query,
            injection_query,
        })
    }

    pub fn configure(&self, mut f: impl FnMut(&str) -> Option<Highlight>) {
        self.highlight_query.configure(&mut f);
        self.injection_query.configure(&mut f);
    }
}

static INHERITS_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r";+\s*inherits\s*:?\s*([a-z_,()-]+)\s*").unwrap());

/// reads a query by invoking `read_query_text`, handles any `inherits` directives
pub fn read_query(language: &str, mut read_query_text: impl FnMut(&str) -> String) -> String {
    fn read_query_impl(language: &str, read_query_text: &mut impl FnMut(&str) -> String) -> String {
        let query = read_query_text(language);

        // replaces all "; inherits <language>(,<language>)*" with the queries of the given language(s)
        INHERITS_REGEX
            .replace_all(&query, |captures: &regex::Captures| {
                captures[1]
                    .split(',')
                    .fold(String::new(), |mut output, language| {
                        // `write!` to a String cannot fail.
                        write!(
                            output,
                            "\n{}\n",
                            read_query_impl(language, &mut *read_query_text)
                        )
                        .unwrap();
                        output
                    })
            })
            .into_owned()
    }
    read_query_impl(language, &mut read_query_text)
}

pub trait LanguageLoader {
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language>;
    fn get_config(&self, lang: Language) -> Option<&LanguageConfig>;
}

impl<T> LanguageLoader for &'_ T
where
    T: LanguageLoader,
{
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language> {
        T::language_for_marker(self, marker)
    }

    fn get_config(&self, lang: Language) -> Option<&LanguageConfig> {
        T::get_config(self, lang)
    }
}
