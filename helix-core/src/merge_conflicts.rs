//! The conflicts git leaves in a file it could not merge: blocks of lines between conflict
//! markers, your side (`<<<<<<<`), the merge base (`|||||||`, with `merge.conflictStyle =
//! diff3` or `zdiff3`) and their side (`=======` to `>>>>>>>`); and their resolutions.

use std::ops::Range;

use ropey::RopeSlice;

/// A conflict block of a text, by lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The whole block, its markers included.
    pub lines: Range<usize>,
    /// Your side's lines (between `<<<<<<<` and `|||||||` or `=======`).
    pub ours: Range<usize>,
    /// The merge base's lines (between `|||||||` and `=======`), when the file shows them.
    pub base: Option<Range<usize>>,
    /// Their side's lines (between `=======` and `>>>>>>>`).
    pub theirs: Range<usize>,
}

/// How a conflict is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Your side's lines.
    Ours,
    /// Their side's lines.
    Theirs,
    /// Yours, then theirs.
    Both,
}

/// The length of git's markers (`merge.conflictMarkerSize`, 7 unless set otherwise).
const MARKER: usize = 7;

/// Whether `line` is the marker made of `c`: the character `MARKER` times, then the end of the
/// line or a space and a label.
fn is_marker(line: RopeSlice, c: char, labelled: bool) -> bool {
    let mut chars = line.chars();
    for _ in 0..MARKER {
        if chars.next() != Some(c) {
            return false;
        }
    }
    match chars.next() {
        None | Some('\n' | '\r') => true,
        Some(' ') => labelled,
        _ => false,
    }
}

/// The conflict blocks of `text`, in order. A block not closed is no conflict.
pub fn find_conflicts(text: RopeSlice) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    for (index, line) in text.lines().enumerate() {
        if is_marker(line, '<', true) {
            open = Some((index, None, None));
            continue;
        }
        let Some((start, base, middle)) = &mut open else {
            continue;
        };
        if middle.is_none() && base.is_none() && is_marker(line, '|', true) {
            *base = Some(index);
        } else if middle.is_none() && is_marker(line, '=', false) {
            *middle = Some(index);
        } else if let (Some(middle), true) = (*middle, is_marker(line, '>', true)) {
            let start = *start;
            let ours_end = base.unwrap_or(middle);
            conflicts.push(Conflict {
                lines: start..index + 1,
                ours: start + 1..ours_end,
                base: base.map(|base| base + 1..middle),
                theirs: middle + 1..index,
            });
            open = None;
        }
    }
    conflicts
}

/// The conflict with `line` in it, if any.
pub fn conflict_at(conflicts: &[Conflict], line: usize) -> Option<&Conflict> {
    conflicts
        .iter()
        .find(|conflict| conflict.lines.contains(&line))
}

/// `conflict` of `text` resolved: the char range of the block (markers and all) and the text to
/// put there.
pub fn resolve(
    text: RopeSlice,
    conflict: &Conflict,
    resolution: Resolution,
) -> (Range<usize>, String) {
    let chars = |lines: &Range<usize>| -> Range<usize> {
        let at = |line: usize| {
            if line >= text.len_lines() {
                text.len_chars()
            } else {
                text.line_to_char(line)
            }
        };
        at(lines.start)..at(lines.end)
    };
    let side = |lines: &Range<usize>| -> String { text.slice(chars(lines)).to_string() };
    let replacement = match resolution {
        Resolution::Ours => side(&conflict.ours),
        Resolution::Theirs => side(&conflict.theirs),
        Resolution::Both => side(&conflict.ours) + side(&conflict.theirs).as_str(),
    };
    (chars(&conflict.lines), replacement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rope;

    const MERGED: &str = "keep\n<<<<<<< HEAD\nmine\nmine too\n=======\ntheirs\n>>>>>>> other\nkeep\n<<<<<<< HEAD\na\n||||||| base\nb\n=======\nc\n>>>>>>> other\n";

    #[test]
    fn conflicts_are_found() {
        let text = Rope::from(MERGED);
        let found: Vec<Conflict> = find_conflicts(text.slice(..));
        assert_eq!(
            found,
            [
                Conflict {
                    lines: 1..7,
                    ours: 2..4,
                    base: None,
                    theirs: 5..6,
                },
                Conflict {
                    lines: 8..15,
                    ours: 9..10,
                    base: Some(11..12),
                    theirs: 13..14,
                },
            ]
        );
        assert_eq!(conflict_at(&found, 4).map(|c| c.lines.clone()), Some(1..7));
        assert_eq!(conflict_at(&found, 7), None);
        // markers inside text, or not closed, are no conflicts
        let text = Rope::from("<<<<<<<< eight\n=======\n>>>>>>> x\n<<<<<<< open\n");
        let found: Vec<Conflict> = find_conflicts(text.slice(..));
        assert!(found.is_empty());
    }

    #[test]
    fn conflicts_are_resolved() {
        let text = Rope::from(MERGED);
        let found = find_conflicts(text.slice(..));
        let (range, ours): (Range<usize>, String) =
            resolve(text.slice(..), &found[0], Resolution::Ours);
        assert_eq!(
            text.slice(range.clone()).to_string().lines().next(),
            Some("<<<<<<< HEAD")
        );
        assert_eq!(ours, "mine\nmine too\n");
        let (_, theirs) = resolve(text.slice(..), &found[0], Resolution::Theirs);
        assert_eq!(theirs, "theirs\n");
        let (_, both) = resolve(text.slice(..), &found[1], Resolution::Both);
        assert_eq!(both, "a\nc\n");
    }
}
