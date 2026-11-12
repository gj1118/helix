//! The changes between two texts, for showing them side by side: changed blocks of lines
//! (hunks), the words that changed inside them, and the mapping of a line of one side to the
//! other side.
//!
//! Lines are compared without their line endings (`\n` and `\r\n` are the same), and, with
//! `ignore_whitespace`, without any whitespace (indentation, spaces inside the line, trailing
//! spaces). Hunk boundaries slide to indentation boundaries (`imara-diff`'s indent heuristic),
//! so that a function added before another one is one hunk with whole lines.

use std::ops::Range;

use imara_diff::{Algorithm, Diff, IndentHeuristic, IndentLevel, InternedInput, Interner};
use ropey::RopeSlice;

/// How two texts are compared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// Lines that differ only in whitespace are the same, and whitespace changes inside a
    /// changed line are not emphasized.
    pub ignore_whitespace: bool,
}

/// What a hunk does to the left side to make the right side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    /// Lines only on the right.
    Added,
    /// Lines only on the left.
    Removed,
    /// Lines on both sides, different.
    Changed,
}

/// A block of lines that differs between the two sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// The lines of the left side (empty for an addition: where they would be).
    pub left: Range<usize>,
    /// The lines of the right side (empty for a removal: where they would be).
    pub right: Range<usize>,
}

/// The words that changed inside a changed hunk (see [`hunk_words`]), as char ranges of the
/// whole text of each side, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HunkWords {
    pub left: Vec<Range<usize>>,
    pub right: Vec<Range<usize>>,
}

impl HunkWords {
    /// The changed words of `side`.
    pub fn side(&self, side: Side) -> &[Range<usize>] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }
}

impl DiffHunk {
    pub fn kind(&self) -> HunkKind {
        if self.left.is_empty() {
            HunkKind::Added
        } else if self.right.is_empty() {
            HunkKind::Removed
        } else {
            HunkKind::Changed
        }
    }

    /// The lines of `side`.
    pub fn lines(&self, side: Side) -> Range<usize> {
        match side {
            Side::Left => self.left.clone(),
            Side::Right => self.right.clone(),
        }
    }
}

/// One side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// The longest texts (in lines) diffed with the histogram algorithm; longer ones use Myers.
const HISTOGRAM_MAX_LINES: usize = 5_000;

/// Hunks with more lines than this on a side get no word emphasis (it costs, and shows
/// little).
const MAX_WORD_DIFF_LINES: usize = 200;
/// Nor hunks with more characters than this on a side: a few long lines (a minified file)
/// would be hundreds of thousands of words, which take the word diff seconds or more.
const MAX_WORD_DIFF_CHARS: usize = 16_000;

/// The hunks that turn `left` into `right`, in order: lines only (the changed words of a hunk
/// are [`hunk_words`], for the hunks that are shown).
pub fn diff_hunks(left: RopeSlice, right: RopeSlice, options: DiffOptions) -> Vec<DiffHunk> {
    // each text copied once, its lines compared as slices of the copy (hashing the lines of
    // the rope in place is slower, and a copy per line allocates for each)
    let (left_text, right_text) = (String::from(left), String::from(right));
    let (left_lines, right_lines) = (line_strs(left, &left_text), line_strs(right, &right_text));
    let diff = if options.ignore_whitespace {
        let key = |line: &&str| -> String { line.chars().filter(|c| !c.is_whitespace()).collect() };
        let input = interned(left_lines.iter().map(key), right_lines.iter().map(key));
        line_diff(&input, |line: &String| line.bytes().collect::<Vec<_>>())
    } else {
        let input = interned(left_lines.into_iter(), right_lines.into_iter());
        line_diff(&input, |line: &&str| line.bytes().collect::<Vec<_>>())
    };
    diff.hunks()
        .map(|hunk| DiffHunk {
            left: hunk.before.start as usize..hunk.before.end as usize,
            right: hunk.after.start as usize..hunk.after.end as usize,
        })
        .collect()
}

/// `before` and `after` interned.
fn interned<T: std::hash::Hash + Eq>(
    before: impl Iterator<Item = T>,
    after: impl Iterator<Item = T>,
) -> InternedInput<T> {
    let mut input = InternedInput {
        before: Vec::new(),
        after: Vec::new(),
        interner: Interner::new(0),
    };
    input.update_before(before);
    input.update_after(after);
    input
}

/// The diff of the lines of `input`, the hunks slid to indentation boundaries; `indent` is
/// the start of a line, for its indentation.
fn line_diff<T: std::hash::Hash + Eq>(
    input: &InternedInput<T>,
    indent: impl Fn(&T) -> Vec<u8>,
) -> Diff {
    // Histogram reads better on code (it doesn't line up on `}` and blank lines) but slows
    // down on big files with many changes (50 ms for 50,000 lines), where Myers (git's
    // default) stays around a millisecond
    let algorithm = if input.before.len().max(input.after.len()) <= HISTOGRAM_MAX_LINES {
        Algorithm::Histogram
    } else {
        Algorithm::Myers
    };
    let mut diff = Diff::compute(algorithm, input);
    diff.postprocess_with_heuristic(
        input,
        IndentHeuristic::new(|token| {
            IndentLevel::for_ascii_line(indent(&input.interner[token]), 8)
        }),
    );
    diff
}

/// The lines of `text` (not the empty one after a final line ending: see [`line_count`])
/// without their line endings, as slices of `copy`, the
/// whole of `text`.
fn line_strs<'a>(text: RopeSlice<'a>, copy: &'a str) -> Vec<&'a str> {
    let count = line_count(text);
    let trim = |line: &'a str| line.trim_end_matches(['\n', '\r']);
    // the rope's lines end at `\n` (Helix builds ropey without its other line breaks): split
    // the copy, which is much faster than asking the rope where each line starts
    let lines: Vec<&str> = copy.split_inclusive('\n').map(trim).collect();
    if lines.len() == count {
        return lines;
    }
    (0..count)
        .map(|line| trim(&copy[text.line_to_byte(line)..text.line_to_byte(line + 1)]))
        .collect()
}

/// The words that changed between the two sides of `hunk` (of `left` and `right`): none for
/// additions and removals, nor for hunks too big to be worth it (see [`MAX_WORD_DIFF_LINES`]
/// and [`MAX_WORD_DIFF_CHARS`]).
pub fn hunk_words(
    left: RopeSlice,
    right: RopeSlice,
    hunk: &DiffHunk,
    options: DiffOptions,
) -> HunkWords {
    let small = |text: RopeSlice, lines: &Range<usize>| {
        !lines.is_empty()
            && lines.len() <= MAX_WORD_DIFF_LINES
            && lines.end <= text.len_lines()
            && line_chars(text, lines.clone()).len() <= MAX_WORD_DIFF_CHARS
    };
    if !small(left, &hunk.left) || !small(right, &hunk.right) {
        return HunkWords::default();
    }
    let (left, right) = changed_words(left, hunk.left.clone(), right, hunk.right.clone(), options);
    HunkWords { left, right }
}

/// The chars of `lines` of `text`.
fn line_chars(text: RopeSlice, lines: Range<usize>) -> Range<usize> {
    let start = text.line_to_char(lines.start);
    let end = if lines.end >= text.len_lines() {
        text.len_chars()
    } else {
        text.line_to_char(lines.end)
    };
    start..end
}

/// How many lines `text` has, not counting the empty one after a final line ending (an
/// empty text has none).
pub fn line_count(text: RopeSlice) -> usize {
    if text.len_chars() == 0 {
        return 0;
    }
    let lines = text.len_lines();
    if lines > 1 && text.line(lines - 1).len_chars() == 0 {
        lines - 1
    } else {
        lines
    }
}

/// A word of a line: a run of letters, digits and `_`, a run of whitespace, or one other
/// character; as a char range of the whole text.
fn words(text: RopeSlice, lines: Range<usize>) -> Vec<Range<usize>> {
    let Range { start, end } = line_chars(text, lines);
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut words = Vec::new();
    let mut word_start = start;
    let mut previous: Option<Class> = None;
    for (offset, c) in text.slice(start..end).chars().enumerate() {
        let idx = start + offset;
        let current = class(c);
        let continues = match &previous {
            Some(prev) => *prev == current && current != Class::Other && c != '\n' && c != '\r',
            None => false,
        };
        if !continues && previous.is_some() {
            words.push(word_start..idx);
            word_start = idx;
        }
        previous = Some(current);
        if c == '\n' || c == '\r' {
            // a line ending is a word of its own, never merged
            previous = Some(Class::Other);
        }
    }
    if previous.is_some() {
        words.push(word_start..end);
    }
    words
}

/// The char ranges of the words that changed between the lines `left_lines` of `left` and
/// `right_lines` of `right`, on each side; adjacent ones merged, line endings left out.
fn changed_words(
    left: RopeSlice,
    left_lines: Range<usize>,
    right: RopeSlice,
    right_lines: Range<usize>,
    options: DiffOptions,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let keep = |text: RopeSlice, range: &Range<usize>| -> bool {
        let first = text.char(range.start);
        !(options.ignore_whitespace && first.is_whitespace())
    };
    let left_words: Vec<Range<usize>> = words(left, left_lines)
        .into_iter()
        .filter(|range| keep(left, range))
        .collect();
    let right_words: Vec<Range<usize>> = words(right, right_lines)
        .into_iter()
        .filter(|range| keep(right, range))
        .collect();
    let mut input: InternedInput<String> = InternedInput::default();
    input.update_before(
        left_words
            .iter()
            .map(|range| left.slice(range.clone()).to_string()),
    );
    input.update_after(
        right_words
            .iter()
            .map(|range| right.slice(range.clone()).to_string()),
    );
    let diff = Diff::compute(Algorithm::Myers, &input);
    let mut left_changed = Vec::new();
    let mut right_changed = Vec::new();
    for hunk in diff.hunks() {
        let side = |words: &[Range<usize>], text: RopeSlice, tokens: Range<u32>| {
            words[tokens.start as usize..tokens.end as usize]
                .iter()
                .filter(|range| {
                    let c = text.char(range.start);
                    c != '\n' && c != '\r'
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        left_changed.extend(side(&left_words, left, hunk.before));
        right_changed.extend(side(&right_words, right, hunk.after));
    }
    (merge(left_changed), merge(right_changed))
}

/// `ranges` (in order) with the touching ones merged.
fn merge(ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => merged.push(range),
        }
    }
    merged
}

/// The line of the `to` side that corresponds to `line` of the other side: the same line
/// moved by the hunks before it, or, inside a hunk, the line at the same distance into the
/// other side of the hunk (its last line, or where it would be when that side is empty).
pub fn map_line(hunks: &[DiffHunk], line: usize, to: Side) -> usize {
    let from = to.other();
    let mut delta: isize = 0;
    for hunk in hunks {
        let (from_lines, to_lines) = (hunk.lines(from), hunk.lines(to));
        if line < from_lines.start {
            break;
        }
        if line < from_lines.end {
            let into = line - from_lines.start;
            return to_lines.start + into.min(to_lines.len().saturating_sub(1));
        }
        delta = to_lines.end as isize - from_lines.end as isize;
    }
    (line as isize + delta).max(0) as usize
}

/// The index of the hunk with `line` on `side`, or of the first one after it, and whether
/// `line` is in it.
pub fn hunk_at(hunks: &[DiffHunk], line: usize, side: Side) -> (usize, bool) {
    let index = hunks.partition_point(|hunk| {
        let lines = hunk.lines(side);
        // an empty side sits before its line
        lines.end <= line && !(lines.is_empty() && lines.start == line)
    });
    let inside = hunks.get(index).is_some_and(|hunk| {
        let lines = hunk.lines(side);
        lines.contains(&line)
    });
    (index, inside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rope;

    fn hunks(left: &str, right: &str, options: DiffOptions) -> Vec<DiffHunk> {
        diff_hunks(
            Rope::from(left).slice(..),
            Rope::from(right).slice(..),
            options,
        )
    }

    fn lines_of(hunks: &[DiffHunk]) -> Vec<(Range<usize>, Range<usize>, HunkKind)> {
        hunks
            .iter()
            .map(|hunk| (hunk.left.clone(), hunk.right.clone(), hunk.kind()))
            .collect()
    }

    #[test]
    fn hunks_of_additions_removals_and_changes() {
        let left = "a\nb\nc\nd\ne\n";
        let right = "a\nB\nc\ne\nf\ng\n";
        let found = hunks(left, right, DiffOptions::default());
        assert_eq!(
            lines_of(&found),
            [
                (1..2, 1..2, HunkKind::Changed),
                (3..4, 3..3, HunkKind::Removed),
                (5..5, 4..6, HunkKind::Added),
            ]
        );
        assert!(hunks(left, left, DiffOptions::default()).is_empty());
        // the line endings don't count
        assert!(hunks("a\nb\n", "a\r\nb\r\n", DiffOptions::default()).is_empty());
        // nor the final line ending
        assert!(hunks("a\nb", "a\nb\n", DiffOptions::default()).is_empty());
    }

    #[test]
    fn whitespace_counts_unless_ignored() {
        let left = "fn main() {\n    run();\n}\n";
        let right = "fn main()  {\n  run( );\n}\n";
        assert_eq!(
            lines_of(&hunks(left, right, DiffOptions::default())),
            [(0..2, 0..2, HunkKind::Changed)]
        );
        let ignoring = DiffOptions {
            ignore_whitespace: true,
        };
        assert!(hunks(left, right, ignoring).is_empty());
    }

    #[test]
    fn changed_words_are_emphasized() {
        let left = "let total = price * count;\nkeep\n";
        let right = "let total = cost * count + tax;\nkeep\n";
        let found = hunks(left, right, DiffOptions::default());
        assert_eq!(found.len(), 1);
        let text = |s: &str, ranges: &[Range<usize>]| -> Vec<String> {
            ranges
                .iter()
                .map(|r| s.chars().skip(r.start).take(r.len()).collect())
                .collect()
        };
        let words = |left: &str, right: &str, hunk: &DiffHunk| -> HunkWords {
            hunk_words(
                Rope::from(left).slice(..),
                Rope::from(right).slice(..),
                hunk,
                DiffOptions::default(),
            )
        };
        let found_words: HunkWords = words(left, right, &found[0]);
        assert_eq!(text(left, &found_words.left), ["price"]);
        assert_eq!(text(right, &found_words.right), ["cost", " + tax"]);
        assert_eq!(found_words.side(Side::Left), &found_words.left[..]);
        // added and removed lines have no word emphasis
        let added = hunks("a\n", "a\nb\n", DiffOptions::default());
        let none: HunkWords = words("a\n", "a\nb\n", &added[0]);
        assert_eq!(none, HunkWords::default());
    }

    /// Hunks too big for a word diff (many lines, or a few very long ones: a minified file)
    /// get none, at once.
    #[test]
    fn big_hunks_get_no_word_emphasis() {
        let line = |prefix: &str| -> String {
            (0..100_000)
                .map(|i| format!("{prefix}{i}=b+c;"))
                .collect::<String>()
                + "\n"
        };
        let (left, right) = (Rope::from(line("a")), Rope::from(line("q")));
        let found = diff_hunks(left.slice(..), right.slice(..), DiffOptions::default());
        assert_eq!(lines_of(&found), [(0..1, 0..1, HunkKind::Changed)]);
        let started = std::time::Instant::now();
        let words: HunkWords = hunk_words(
            left.slice(..),
            right.slice(..),
            &found[0],
            DiffOptions::default(),
        );
        assert_eq!(words, HunkWords::default());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        let many = |c: &str| -> String { (0..300).map(|i| format!("{c}{i}\n")).collect() };
        let (left, right) = (Rope::from(many("x")), Rope::from(many("y")));
        let found = diff_hunks(left.slice(..), right.slice(..), DiffOptions::default());
        assert_eq!(lines_of(&found), [(0..300, 0..300, HunkKind::Changed)]);
        let words: HunkWords = hunk_words(
            left.slice(..),
            right.slice(..),
            &found[0],
            DiffOptions::default(),
        );
        assert_eq!(words, HunkWords::default());
    }

    #[test]
    fn lines_map_to_the_other_side() {
        let left = "a\nb\nc\nd\ne\n";
        let right = "a\nB\nc\ne\nf\ng\n";
        let found = hunks(left, right, DiffOptions::default());
        // before, inside and after the hunks
        assert_eq!(map_line(&found, 0, Side::Right), 0);
        assert_eq!(map_line(&found, 1, Side::Right), 1);
        assert_eq!(map_line(&found, 3, Side::Right), 3); // the removed `d`: where it was
        assert_eq!(map_line(&found, 4, Side::Right), 3); // `e`
        assert_eq!(map_line(&found, 3, Side::Left), 4); // and back
        assert_eq!(map_line(&found, 5, Side::Left), 5); // `g`, an addition: after `e`
        assert_eq!(hunk_at(&found, 0, Side::Left), (0, false));
        assert_eq!(hunk_at(&found, 1, Side::Left), (0, true));
        assert_eq!(hunk_at(&found, 2, Side::Left), (1, false));
        assert_eq!(hunk_at(&found, 3, Side::Left), (1, true));
        assert_eq!(hunk_at(&found, 4, Side::Right), (2, true));
        assert_eq!(hunk_at(&found, 4, Side::Left), (2, false));
    }

    /// Past [`HISTOGRAM_MAX_LINES`] the diff is Myers': the same hunks for a simple change.
    #[test]
    fn big_texts_get_the_same_hunks() {
        let text = |changed: usize| -> String {
            (0..6_000)
                .map(|i| match i {
                    _ if i == changed => format!("line {i} changed\n"),
                    _ => format!("line {i}\n"),
                })
                .collect()
        };
        let (left, right) = (text(usize::MAX), text(4_321));
        let found: Vec<DiffHunk> = hunks(&left, &right, DiffOptions::default());
        assert_eq!(
            lines_of(&found),
            [(4_321..4_322, 4_321..4_322, HunkKind::Changed)]
        );
        let removed = left.replace("line 100\n", "");
        let found: Vec<DiffHunk> = hunks(&left, &removed, DiffOptions::default());
        assert_eq!(lines_of(&found), [(100..101, 100..100, HunkKind::Removed)]);
    }

    #[test]
    fn line_count_ignores_the_line_after_a_final_line_ending() {
        assert_eq!(line_count(Rope::from("a\nb\n").slice(..)), 2);
        assert_eq!(line_count(Rope::from("a\nb").slice(..)), 2);
        assert_eq!(line_count(Rope::from("").slice(..)), 0);
        assert_eq!(line_count(Rope::from("\n").slice(..)), 1);
        // against an empty text, everything is added
        let found = hunks("", "a\nb\n", DiffOptions::default());
        assert_eq!(lines_of(&found), [(0..0, 0..2, HunkKind::Added)]);
    }
}
