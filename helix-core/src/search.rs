use helix_stdx::rope::RopeSliceExt;

use crate::{
    graphemes::{
        nth_next_folded_grapheme_boundary, nth_next_grapheme_boundary,
        nth_prev_folded_grapheme_boundary, nth_prev_grapheme_boundary,
    },
    text_folding::{ropex::RopeSliceFoldExt, FoldAnnotations},
    RopeSlice,
};

pub trait GraphemeMatcher {
    fn grapheme_match(&self, g: RopeSlice) -> bool;
}

impl GraphemeMatcher for char {
    fn grapheme_match(&self, g: RopeSlice) -> bool {
        g == RopeSlice::from(self.encode_utf8(&mut [0; 4]) as &str)
    }
}

impl<F: Fn(RopeSlice) -> bool> GraphemeMatcher for F {
    fn grapheme_match(&self, g: RopeSlice) -> bool {
        (*self)(g)
    }
}

pub fn find_nth_next(
    text: RopeSlice,
    matcher: impl GraphemeMatcher,
    pos: usize,
    mut n: usize,
) -> Option<usize> {
    // nothing after the end (`t` with the cursor on the last character starts past it)
    if n == 0 || pos >= text.len_chars() {
        return None;
    }

    let mut count = 0;
    let bpos = text.char_to_byte(pos);
    for (i, g) in text.graphemes_at(bpos).skip(1).enumerate() {
        if matcher.grapheme_match(g) {
            count = i + 1;
            n -= 1;
            if n == 0 {
                break;
            }
        }
    }

    (n == 0).then(|| nth_next_grapheme_boundary(text, pos, count))
}

pub fn find_nth_prev(
    text: RopeSlice,
    matcher: impl GraphemeMatcher,
    pos: usize,
    mut n: usize,
) -> Option<usize> {
    if n == 0 || pos > text.len_chars() {
        return None;
    }

    let mut count = 0;
    let bpos = text.char_to_byte(pos);
    for (i, g) in text.graphemes_at(bpos).reversed().enumerate() {
        if matcher.grapheme_match(g) {
            count = i + 1;
            n -= 1;
            if n == 0 {
                break;
            }
        }
    }

    (n == 0).then(|| nth_prev_grapheme_boundary(text, pos, count))
}

pub fn find_folded_nth_next(
    text: RopeSlice,
    annotations: &FoldAnnotations,
    matcher: impl GraphemeMatcher,
    pos: usize,
    mut n: usize,
) -> Option<usize> {
    if n == 0 || pos >= text.len_chars() {
        return None;
    }

    let mut count = 0;
    for (i, g) in text
        .folded_graphemes_at(annotations, text.char_to_byte(pos))
        .skip(1)
        .enumerate()
    {
        if matcher.grapheme_match(g) {
            count = i + 1;
            n -= 1;
            if n == 0 {
                break;
            }
        }
    }

    (n == 0).then(|| nth_next_folded_grapheme_boundary(text, annotations, pos, count))
}

pub fn find_folded_nth_prev(
    text: RopeSlice,
    annotations: &FoldAnnotations,
    matcher: impl GraphemeMatcher,
    pos: usize,
    mut n: usize,
) -> Option<usize> {
    if n == 0 || pos > text.len_chars() {
        return None;
    }

    let mut count = 0;
    for (i, g) in text
        .folded_graphemes_at(annotations, text.char_to_byte(pos))
        .reversed()
        .enumerate()
    {
        if matcher.grapheme_match(g) {
            count = i + 1;
            n -= 1;
            if n == 0 {
                break;
            }
        }
    }

    (n == 0).then(|| nth_prev_folded_grapheme_boundary(text, annotations, pos, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_from_past_the_end_finds_nothing() {
        let text = RopeSlice::from("ab\n");
        let len: usize = text.len_chars();
        assert_eq!(len, 3);
        // from the end and from past it: nothing after
        let at_end: Option<usize> = find_nth_next(text, 'a', len, 1);
        assert_eq!(at_end, None);
        let past: Option<usize> = find_nth_next(text, 'a', len + 1, 1);
        assert_eq!(past, None);
        let prev_past: Option<usize> = find_nth_prev(text, 'a', len + 1, 1);
        assert_eq!(prev_past, None);
        // in the text, still found
        let next: Option<usize> = find_nth_next(text, 'b', 0, 1);
        assert_eq!(next, Some(1));
        let prev: Option<usize> = find_nth_prev(text, 'a', len, 1);
        assert_eq!(prev, Some(0));
    }

    #[test]
    fn a_folded_search_from_past_the_end_finds_nothing() {
        let text = RopeSlice::from("ab\n");
        let folds = FoldAnnotations::default();
        let past: Option<usize> = find_folded_nth_next(text, &folds, 'a', 4, 1);
        assert_eq!(past, None);
        let prev_past: Option<usize> = find_folded_nth_prev(text, &folds, 'a', 4, 1);
        assert_eq!(prev_past, None);
        let next: Option<usize> = find_folded_nth_next(text, &folds, 'b', 0, 1);
        assert_eq!(next, Some(1));
    }
}
