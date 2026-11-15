use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \nfoo foo foo foo \nfoo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \nfooo fooo fooo \nfooo fooo fooo \nfooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n....foo3 foo4 \n....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \nfoo3 foo4 foo5 \nfoo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxxx5xxxx6xxxx7xxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n....x3xxxx4xxxxx5\n....xxxx6xxxx7xxx\n....8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxxxxx9xxx\n"),
        "    xxxx1xxxx \n....2xxxx3xxxx4xx\n....xx5xxxx6xxxx7\n....xxxx8xxxxxxx9\n....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxxx5xxxx6xxxx7xxxxx8xxxx9xx\n"),
        "    xxxx1xxx 2xxx\n....x3xxxx4xxxxx5\n....xxxx6xxxx7xxx\n....xx8xxxx9xx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \nábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \nfoo Xoo foo foo \nfoo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \nfoo foo foo foo \nfoo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

/// The graphemes of a whole walk: (char index, visual position, text, virtual).
fn walk(text: &str, annotations: &TextAnnotations) -> Vec<(usize, crate::Position, String, bool)> {
    let text = crate::Rope::from(text);
    let text_fmt = TextFormat::new_test(false);
    DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, annotations, 0)
        .map(|g| (g.char_idx, g.visual_pos, g.raw.to_string(), g.is_virtual()))
        .collect()
}

/// Texts with plain runs, tabs, line breaks, combining marks, wide chars, emoji.
const SKIP_TEXTS: &[&str] = &[
    "plain ascii text that goes on\nand a second line\n",
    "tab\there and e\u{301}combining, 名前 wide, 👍🏽 emoji end\n",
    "ab\r\ncd\n\nlast",
    "x",
    "",
];

#[test]
fn skipping_plain_text_matches_the_walk() {
    let inline = [
        InlineAnnotation::new(5, "<hint>"),
        InlineAnnotation::new(12, "!"),
    ];
    let overlays = [Overlay::new(8, "#")];
    let mut annotated = TextAnnotations::default();
    annotated.add_inline_annotations(&inline, None);
    annotated.add_overlay(&overlays, None);
    let plain = TextAnnotations::default();

    for annotations in [&plain, &annotated] {
        for &source in SKIP_TEXTS {
            let full = walk(source, annotations);
            let text = crate::Rope::from(source);
            let text_fmt = TextFormat::new_test(false);
            for limit in 0..=text.len_chars() + 1 {
                for (char_limit, col_limit) in [(limit, usize::MAX), (usize::MAX, limit)] {
                    let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
                        text.slice(..),
                        &text_fmt,
                        annotations,
                        0,
                    );
                    let skipped: usize = formatter.skip_plain_text(char_limit, col_limit);
                    let at: usize = formatter.next_char_pos();
                    assert_eq!(at, skipped);
                    assert!(at <= char_limit.max(0), "{source:?}: past the char limit");
                    let rest: Vec<(usize, crate::Position, String, bool)> = formatter
                        .map(|g| (g.char_idx, g.visual_pos, g.raw.to_string(), g.is_virtual()))
                        .collect();
                    // the walk goes on where the full walk is at that char: same graphemes,
                    // same positions
                    let resume = full.iter().position(|g| g.0 >= at).unwrap_or(full.len());
                    assert_eq!(
                        rest,
                        full[resume..].to_vec(),
                        "{source:?} skipping to char {char_limit} col {col_limit}"
                    );
                }
            }
        }
    }
}

#[test]
fn positions_match_the_walk_on_long_plain_lines() {
    use crate::position::{
        char_idx_at_visual_block_offset, coords_at_pos, visual_offset_from_block,
    };
    use helix_stdx::rope::RopeSliceExt;
    let text_fmt = TextFormat::new_test(false);
    let annotations = TextAnnotations::default();
    let source = format!("{}\n{}é\tend\n", "a".repeat(300), "b".repeat(200));
    let text = crate::Rope::from(source.as_str());
    let full = walk(&source, &annotations);
    for &(char_idx, visual_pos, _, _) in &full {
        let (pos, block_start): (crate::Position, usize) =
            visual_offset_from_block(text.slice(..), 0, char_idx, &text_fmt, &annotations);
        assert_eq!((pos, block_start), (visual_pos, 0), "char {char_idx}");
        if char_idx < text.len_chars() {
            let coords: crate::Position = coords_at_pos(text.slice(..), char_idx);
            let line = text.char_to_line(char_idx);
            let graphemes = text
                .slice(text.line_to_char(line)..char_idx)
                .graphemes()
                .count();
            assert_eq!(
                coords,
                crate::Position::new(line, graphemes),
                "char {char_idx}"
            );
        }
    }
    // the char at each column of each row: the walk's grapheme there
    for row in 0..2 {
        for col in [0, 1, 150, 199, 200, 201, 202, 205, 299, 300, 400] {
            let (found, virtual_rows): (usize, usize) = char_idx_at_visual_block_offset(
                text.slice(..),
                0,
                row,
                col,
                &text_fmt,
                &annotations,
            );
            let expected = full
                .iter()
                .filter(|g| g.1.row == row && !g.3)
                .take_while(|g| g.1.col <= col)
                .last()
                .map(|g| g.0)
                .unwrap();
            assert_eq!((found, virtual_rows), (expected, 0), "row {row} col {col}");
        }
    }
}

#[test]
fn soft_wrap_blocks_of_long_lines_match_the_walk() {
    use super::SOFT_WRAP_BLOCK;
    use crate::position::visual_offset_from_block;
    let text_fmt = TextFormat::new_test(true);
    let annotations = TextAnnotations::default();
    // a line of words three blocks and a half long (a tab, a combining mark in it), then a short
    // line
    let mut long = String::new();
    let mut i = 0;
    while long.len() < SOFT_WRAP_BLOCK * 3 + SOFT_WRAP_BLOCK / 2 {
        long.push_str(&format!("word{i} "));
        if i % 1000 == 7 {
            long.push_str("\te\u{301} ");
        }
        i += 1;
    }
    let source = format!("{long}\nshort line\n");
    let text = crate::Rope::from(source.as_str());
    let slice = text.slice(..);

    // the whole walk: where each grapheme is
    let full: Vec<(usize, crate::Position)> =
        DocumentFormatter::new_at_prev_checkpoint(slice, &text_fmt, &annotations, 0)
            .filter(|g| !g.is_virtual())
            .map(|g| (g.char_idx, g.visual_pos))
            .collect();
    let row_of = |char_idx: usize| -> crate::Position {
        full.iter().find(|(c, _)| *c == char_idx).unwrap().1
    };

    // the blocks: a new visual line at each one's start
    let line_end = long.chars().count();
    let mut starts: Vec<usize> = vec![0];
    for (char_idx, pos) in &full {
        if *char_idx > 0 && *char_idx < line_end && pos.col == 0 {
            let start = super::block_start(slice, &text_fmt, 0, *char_idx);
            if start == *char_idx {
                starts.push(start);
            }
        }
    }
    assert_eq!(starts.len(), 4, "{starts:?}");

    // from a block's start, each of its graphemes is where the whole walk has it
    let mut checked = 0;
    for &(char_idx, pos) in full.iter().filter(|(c, _)| {
        c % 997 == 0 || starts.iter().any(|s| s.abs_diff(*c) <= 2) || *c >= line_end
    }) {
        let (in_block, block): (crate::Position, usize) =
            visual_offset_from_block(slice, char_idx, char_idx, &text_fmt, &annotations);
        let expected_block = *starts.iter().rfind(|&&s| s <= char_idx).unwrap();
        // after the long line: its own line's start
        let expected_block = if char_idx > line_end {
            slice.line_to_char(slice.char_to_line(char_idx))
        } else {
            expected_block
        };
        assert_eq!(block, expected_block, "char {char_idx}");
        let block_pos = row_of(block);
        assert_eq!(
            in_block,
            crate::Position::new(pos.row - block_pos.row, pos.col),
            "char {char_idx}"
        );
        checked += 1;
    }
    assert!(checked > 50);
}

#[test]
fn moving_up_across_soft_wrap_blocks_matches_the_walk() {
    use super::SOFT_WRAP_BLOCK;
    use crate::position::char_idx_at_visual_offset;
    let text_fmt = TextFormat::new_test(true);
    let annotations = TextAnnotations::default();
    let long: String = (0..SOFT_WRAP_BLOCK)
        .map(|i| format!("w{} ", i % 10))
        .collect();
    let text = crate::Rope::from(format!("{long}\n").as_str());
    let slice = text.slice(..);
    let full: Vec<(usize, crate::Position)> =
        DocumentFormatter::new_at_prev_checkpoint(slice, &text_fmt, &annotations, 0)
            .filter(|g| !g.is_virtual())
            .map(|g| (g.char_idx, g.visual_pos))
            .collect();
    // the char at `col` of `row`: of the grapheme there or to its left, else the row's first
    let at = |row: usize, col: usize| -> usize {
        let on_row = || full.iter().filter(|(_, p)| p.row == row);
        on_row()
            .rfind(|(_, p)| p.col <= col)
            .or_else(|| on_row().next())
            .unwrap()
            .0
    };
    // from chars around each block's start, rows up: the walk's char there
    let starts: Vec<usize> = (1..4)
        .map(|block| super::block_boundary(slice, 0, block))
        .filter(|&s| s < long.len())
        .collect();
    assert!(!starts.is_empty());
    for &start in &starts {
        for char_idx in [start - 1, start, start + 1, start + 40] {
            let pos = full.iter().find(|(c, _)| *c == char_idx).unwrap().1;
            for (up, col) in [(1usize, 0usize), (1, 5), (3, 0)] {
                let (found, virtual_rows): (usize, usize) = char_idx_at_visual_offset(
                    slice,
                    char_idx,
                    -(up as isize),
                    col,
                    &text_fmt,
                    &annotations,
                );
                assert_eq!(
                    (found, virtual_rows),
                    (at(pos.row - up, col), 0),
                    "from char {char_idx}, {up} rows up, column {col}"
                );
            }
        }
    }
}
