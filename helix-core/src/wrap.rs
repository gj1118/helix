use smartstring::{LazyCompact, SmartString};
use textwrap::{Options, WordSplitter::NoHyphenation};
use unicode_segmentation::UnicodeSegmentation;

use crate::unicode::width::UnicodeWidthStr;

/// Given a slice of text, return the text re-wrapped to fit it
/// within the given width.
pub fn reflow_hard_wrap(text: &str, text_width: usize) -> SmartString<LazyCompact> {
    let options = Options::new(text_width)
        .word_splitter(NoHyphenation)
        .word_separator(textwrap::WordSeparator::AsciiSpace);
    textwrap::refill(text, options).into()
}

/// `text` wrapped at word boundaries: the first line to `first_width` columns, the others to
/// `width`. A word longer than a line is broken (nothing is dropped); tabs are spaces, and
/// each line of `text` starts a new line.
pub fn wrap_words(text: &str, first_width: usize, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let line_width = |lines: &Vec<String>| if lines.is_empty() { first_width } else { width };
    for paragraph in text.replace('\t', "    ").lines() {
        let mut current = String::new();
        let mut current_width = 0;
        for word in paragraph.split_whitespace() {
            let word_width = word.width();
            let gap = usize::from(!current.is_empty());
            if current_width + gap + word_width <= line_width(&lines) {
                if gap == 1 {
                    current.push(' ');
                }
                current.push_str(word);
                current_width += gap + word_width;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            // the word alone, broken over lines when longer than one
            for grapheme in word.graphemes(true) {
                let grapheme_width = grapheme.width();
                if current_width + grapheme_width > line_width(&lines) && !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push_str(grapheme);
                current_width += grapheme_width;
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap_words;

    #[test]
    fn words_wrap_with_a_first_line_of_its_own() {
        let lines: Vec<String> = wrap_words("one two three four", 7, 9);
        assert_eq!(lines, ["one two", "three", "four"]);
        // a word longer than a line is broken, nothing dropped
        let long: Vec<String> = wrap_words("abcdefghij", 4, 4);
        assert_eq!(long, ["abcd", "efgh", "ij"]);
        // line breaks start lines, blank ones stay; tabs are spaces
        let breaks: Vec<String> = wrap_words("a\n\nb\tc", 10, 10);
        assert_eq!(breaks, ["a", "", "b c"]);
        // wide characters by their width
        let wide: Vec<String> = wrap_words("名前名前", 5, 5);
        assert_eq!(wide, ["名前", "名前"]);
        let empty: Vec<String> = wrap_words("", 5, 5);
        assert_eq!(empty, [""]);
    }
}
