//! Panels plugins show next to a line (`buffer:set_panel`), modelled after
//! tiny-inline-diagnostic.nvim: drawn over the text, so they never move the lines of the
//! document or change scrolling, whatever the size of the view.
//!
//! The panel starts after the end of its line, behind an arrow, its messages wrapped to the
//! columns left; the rows they wrap onto, and the messages after the first, are drawn over the
//! lines below, aligned with the first row. With fewer than [`PluginPanel::min_width`] columns
//! left for the text, the panel goes under the line instead, from the left edge of the view.
//! The layout is computed afresh for every frame, so it follows resizes and edits at once.
//!
//! Every row has the caps on either side (rounded ones make it a pill). With
//! [`PluginPanel::corners`], a message wrapped over several rows is one block instead: its
//! first row has the top corners, its last the bottom ones, and the rows between straight
//! sides. The corners are glyphs, or [`PanelCorners::Round`]: rounded by the terminal where
//! it can draw images, square elsewhere.

use helix_core::unicode::width::UnicodeWidthStr;

/// A panel a plugin attached to a line of a document in a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPanel {
    /// A position on the line the panel belongs to.
    pub char_idx: usize,
    pub items: Vec<PanelItem>,
    /// A last row without an icon, such as "+2 more".
    pub footer: Option<String>,
    /// The text color and the background of the panel (hex colors; the theme's `ui.popup`
    /// when not set).
    pub fg: Option<String>,
    pub bg: Option<String>,
    /// Pointing from the line to the panel beside it, and to the line from the panel under it.
    pub arrow: String,
    pub under_arrow: String,
    /// The arrows' color (the panel's background when not set).
    pub arrow_fg: Option<String>,
    /// Drawn in the panel's background color on either side of each row (rounded caps, say).
    pub left_cap: String,
    pub right_cap: String,
    /// The corners of a message (or footer) wrapped over several rows, which is then drawn
    /// as one block rather than a row with caps for each of its lines.
    pub corners: Option<PanelCorners>,
    /// The text is wrapped to this many columns at most, also where more are left to the
    /// edge of the view (0: only there).
    pub max_width: u16,
    /// Fewer columns than this left for the text beside the line put the panel under it.
    pub min_width: u16,
    /// Whether the panel is shown in insert mode.
    pub show_in_insert: bool,
    /// Not drawn for now (while a prompt is open, say), but kept: the view still has a panel
    /// of a plugin, so Helix's own diagnostics in the corner stay away.
    pub hidden: bool,
}

/// The corners of a block of several rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelCorners {
    /// Glyphs drawn in the panel's background color, like the caps (an empty one is a
    /// square corner).
    Glyphs {
        top_left: String,
        top_right: String,
        bottom_left: String,
        bottom_right: String,
    },
    /// Rounded, with the radius of a cell's width (that of a rounded cap): drawn by the
    /// terminal as an image in each corner's cell where it can, square where it can't.
    Round,
}

impl PanelCorners {
    /// The glyphs of the top and of the bottom corner on the left, or on the `right`.
    fn glyphs(&self, right: bool) -> (&str, &str) {
        match self {
            PanelCorners::Glyphs {
                top_left,
                top_right,
                bottom_left,
                bottom_right,
            } => match right {
                false => (top_left, bottom_left),
                true => (top_right, bottom_right),
            },
            PanelCorners::Round => ("", ""),
        }
    }
}

/// A corner of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }

    pub fn is_bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }
}

/// One message of a panel, with its icon (a bullet, say) in front of its first row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelItem {
    pub icon: String,
    /// The icon's color (hex).
    pub icon_fg: Option<String>,
    pub text: String,
}

impl Default for PluginPanel {
    fn default() -> Self {
        Self {
            char_idx: 0,
            items: Vec::new(),
            footer: None,
            fg: None,
            bg: None,
            arrow: " ← ".to_string(),
            under_arrow: "╰ ".to_string(),
            arrow_fg: None,
            left_cap: String::new(),
            right_cap: String::new(),
            corners: None,
            max_width: 0,
            min_width: 30,
            show_in_insert: false,
            hidden: false,
        }
    }
}

/// What part of the panel a span is, for its style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPart {
    Arrow,
    Cap,
    /// The icon of the item with this index.
    Icon(usize),
    Text,
    /// A corner of a block with [`PanelCorners::Round`]: the cells the terminal rounds, or
    /// else part of the text's background.
    Corner(Corner),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSpan {
    /// The view column the span starts at.
    pub col: usize,
    pub text: String,
    pub part: PanelPart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    /// Rows from the last row of the panel's line: 0 is that row (beside the text), 1 the row
    /// under it, and so on.
    pub row: usize,
    pub spans: Vec<PanelSpan>,
}

fn width(text: &str) -> usize {
    text.width()
}

/// The rows of `panel` in a view `view_width` columns wide, for a line whose text ends at
/// column `line_end_col` of the view (`None` when the end is out of view, scrolled to the side).
pub fn layout(
    panel: &PluginPanel,
    line_end_col: Option<usize>,
    view_width: usize,
) -> Vec<PanelRow> {
    if panel.items.is_empty() && panel.footer.is_none() {
        return Vec::new();
    }
    let icon_width = panel
        .items
        .iter()
        .map(|item| width(&item.icon))
        .max()
        .unwrap_or(0);
    // The sides are as wide as the widest of what is drawn there: a cap or a corner (a
    // rounded one is a cell).
    let corners = panel.corners.as_ref();
    let round = matches!(corners, Some(PanelCorners::Round));
    let side = |cap: &str, right: bool| {
        let (top, bottom) = corners.map_or(("", ""), |corners| corners.glyphs(right));
        width(cap)
            .max(width(top))
            .max(width(bottom))
            .max(usize::from(round))
    };
    let left_width = side(&panel.left_cap, false);
    let right_width = side(&panel.right_cap, true);
    // No wider than the panel wants its text.
    let limit = |text_width: usize| match panel.max_width {
        0 => text_width,
        max => text_width.min(max as usize),
    };
    // The sides, the icon, and a space on either side of the text.
    let frame = left_width + icon_width + 2 + right_width;

    // Beside the line, one column after its end (the cursor can be there).
    let beside = line_end_col.and_then(|end| {
        let arrow_col = end.checked_add(1)?;
        let panel_col = arrow_col.checked_add(width(&panel.arrow))?;
        let text_width = view_width.checked_sub(panel_col)?.checked_sub(frame)?;
        (text_width >= panel.min_width as usize && text_width > 0).then_some((
            0,
            arrow_col,
            &panel.arrow,
            panel_col,
            limit(text_width),
        ))
    });
    let (first_row, arrow_col, arrow, panel_col, text_width) = match beside {
        Some(beside) => beside,
        None => {
            let panel_col = width(&panel.under_arrow);
            match view_width
                .checked_sub(panel_col)
                .and_then(|w| w.checked_sub(frame))
            {
                Some(text_width) if text_width > 0 => {
                    (1, 0, &panel.under_arrow, panel_col, limit(text_width))
                }
                _ => return Vec::new(),
            }
        }
    };

    // Each item's lines (with its icon on the first), then the footer's: the item, where
    // the line is among its lines, and the line.
    let mut lines: Vec<(Option<usize>, Place, String)> = Vec::new();
    let texts = panel.items.iter().map(|item| &item.text).enumerate();
    for (item, text) in texts
        .map(|(idx, text)| (Some(idx), text))
        .chain(panel.footer.iter().map(|footer| (None, footer)))
    {
        let wrapped = wrap(text, text_width);
        let last = wrapped.len() - 1;
        for (n, line) in wrapped.into_iter().enumerate() {
            let place = match (n, last) {
                (_, 0) => Place::Only,
                (0, _) => Place::First,
                (n, last) if n == last => Place::Last,
                _ => Place::Between,
            };
            lines.push((item, place, line));
        }
    }
    // A box: every row as wide as the widest.
    let box_width = lines
        .iter()
        .map(|(_, _, line)| width(line))
        .max()
        .unwrap_or(0);

    let mut rows = Vec::with_capacity(lines.len());
    for (n, (item, place, line)) in lines.into_iter().enumerate() {
        let first = matches!(place, Place::Only | Place::First);
        // What is drawn on either side: the caps, or for a block of several rows its
        // corners and, between them, straight sides.
        let (left, right) = match (corners, place) {
            (None, _) | (_, Place::Only) => (panel.left_cap.as_str(), panel.right_cap.as_str()),
            (Some(corners), Place::First) => (corners.glyphs(false).0, corners.glyphs(true).0),
            (Some(corners), Place::Last) => (corners.glyphs(false).1, corners.glyphs(true).1),
            (Some(_), Place::Between) => ("", ""),
        };
        // The corners the terminal rounds: a side's cells are theirs.
        let rounded = match place {
            Place::First if round => Some((Corner::TopLeft, Corner::TopRight)),
            Place::Last if round => Some((Corner::BottomLeft, Corner::BottomRight)),
            _ => None,
        };
        let mut spans = Vec::new();
        if n == 0 && !arrow.is_empty() {
            spans.push(PanelSpan {
                col: arrow_col,
                text: arrow.clone(),
                part: PanelPart::Arrow,
            });
        }
        let mut col = panel_col;
        let mut push = |text: String, part: PanelPart| {
            let w = width(&text);
            if w > 0 {
                spans.push(PanelSpan { col, text, part });
            }
            col += w;
        };
        match rounded {
            Some((corner, _)) => push(" ".repeat(left_width), PanelPart::Corner(corner)),
            None => {
                push(left.to_string(), PanelPart::Cap);
                push(" ".repeat(left_width - width(left)), PanelPart::Text);
            }
        }
        if icon_width > 0 {
            match item {
                Some(idx) if first => {
                    let icon = &panel.items[idx].icon;
                    let pad = icon_width - width(icon);
                    push(format!("{icon}{}", " ".repeat(pad)), PanelPart::Icon(idx));
                }
                _ => push(" ".repeat(icon_width), PanelPart::Text),
            }
        }
        let pad = box_width - width(&line);
        match rounded {
            Some((_, corner)) => {
                push(format!(" {line}{} ", " ".repeat(pad)), PanelPart::Text);
                push(" ".repeat(right_width), PanelPart::Corner(corner));
            }
            None => {
                push(
                    format!(
                        " {line}{} {}",
                        " ".repeat(pad),
                        " ".repeat(right_width - width(right))
                    ),
                    PanelPart::Text,
                );
                push(right.to_string(), PanelPart::Cap);
            }
        }
        rows.push(PanelRow {
            row: first_row + n,
            spans,
        });
    }
    rows
}

/// Where a line is among the lines of its message.
#[derive(Clone, Copy)]
enum Place {
    /// The message is one line.
    Only,
    First,
    Between,
    Last,
}

/// `text` broken into lines at most `max_width` columns wide (see
/// [`helix_core::wrap::wrap_words`]), without its blank lines.
fn wrap(text: &str, max_width: usize) -> Vec<String> {
    let mut lines: Vec<String> = helix_core::wrap::wrap_words(text, max_width, max_width)
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(texts: &[&str]) -> PluginPanel {
        PluginPanel {
            items: texts
                .iter()
                .map(|text| PanelItem {
                    icon: "●".to_string(),
                    icon_fg: None,
                    text: text.to_string(),
                })
                .collect(),
            arrow: " ← ".to_string(),
            under_arrow: "╰ ".to_string(),
            left_cap: "(".to_string(),
            right_cap: ")".to_string(),
            min_width: 10,
            ..Default::default()
        }
    }

    /// The rows as they would look on screen, from column 0.
    fn draw(rows: &[PanelRow]) -> Vec<(usize, String)> {
        rows.iter()
            .map(|row| {
                let mut line = String::new();
                for span in &row.spans {
                    while width(&line) < span.col {
                        line.push(' ');
                    }
                    line.push_str(&span.text);
                }
                (row.row, line)
            })
            .collect()
    }

    #[test]
    fn beside_the_line_one_column_after_its_end() {
        let rows = layout(&panel(&["Cannot find name"]), Some(6), 60);
        // `consol` ends at column 6; the cursor can be on column 6.
        assert_eq!(
            draw(&rows),
            [(0, "        ← (● Cannot find name )".to_string())]
        );
        assert_eq!(rows[0].spans[0].col, 7);
        assert_eq!(rows[0].spans[0].part, PanelPart::Arrow);
        assert_eq!(rows[0].spans[2].part, PanelPart::Icon(0));
    }

    #[test]
    fn wrapped_rows_and_more_items_go_over_the_lines_below_aligned() {
        // 30 columns: the panel starts at 10, the text gets 30 - 10 - (1 + 1 + 2 + 1) = 15;
        // the box is as wide as its widest row.
        let rows = layout(&panel(&["one two three four five", "second"]), Some(6), 30);
        assert_eq!(
            draw(&rows),
            [
                (0, "        ← (● one two three )".to_string()),
                (1, "          (  four five     )".to_string()),
                (2, "          (● second        )".to_string()),
            ]
        );
    }

    fn corners(sides: [&str; 4]) -> PanelCorners {
        let [top_left, top_right, bottom_left, bottom_right] = sides.map(str::to_string);
        PanelCorners::Glyphs {
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        }
    }

    /// Rounded corners are the terminal's to draw: the layout marks their cells (the first
    /// and the last cell of the first and last row of a block), which are spaces for a
    /// terminal that can't. A message of one row keeps the caps.
    #[test]
    fn round_corners_are_cells_of_their_own() {
        let mut blocks = panel(&["one two three four five six seven", "second"]);
        blocks.corners = Some(PanelCorners::Round);
        // the text gets 15 columns: three rows, the box 13 wide
        let rows = layout(&blocks, Some(6), 30);
        let drawn = draw(&rows);
        assert_eq!(drawn[0].1.trim_end(), "        ←  ● one two three");
        assert_eq!(drawn[1].1.trim_end(), "             four five six");
        assert_eq!(drawn[2].1.trim_end(), "             seven");
        assert_eq!(drawn[3].1, "          (● second        )");
        let corners = |row: &PanelRow| -> Vec<(usize, Corner)> {
            row.spans
                .iter()
                .filter_map(|span| match span.part {
                    PanelPart::Corner(corner) => Some((span.col, corner)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            corners(&rows[0]),
            [(10, Corner::TopLeft), (27, Corner::TopRight)]
        );
        assert_eq!(corners(&rows[1]), []);
        assert_eq!(
            corners(&rows[2]),
            [(10, Corner::BottomLeft), (27, Corner::BottomRight)]
        );
        assert_eq!(corners(&rows[3]), []);
        // every row ends in the same column
        let end = |row: &PanelRow| {
            let last = row.spans.last().unwrap();
            last.col + width(&last.text)
        };
        assert!(rows.iter().all(|row| end(row) == 28), "{rows:#?}");
        assert!(Corner::TopRight.is_right() && !Corner::TopRight.is_bottom());
        assert!(Corner::BottomLeft.is_bottom() && !Corner::BottomLeft.is_right());
        // without caps the corners still have a cell each
        blocks.left_cap.clear();
        blocks.right_cap.clear();
        let rows = layout(&blocks, Some(6), 30);
        assert_eq!(corners(&rows[0]).len(), 2);
    }

    /// `max_width` wraps the text sooner than the edge of the view does, beside the line and
    /// under it; 0 leaves it to the edge.
    #[test]
    fn the_text_is_no_wider_than_max_width() {
        let mut narrow = panel(&["one two three four five six seven eight"]);
        narrow.max_width = 12;
        // 80 columns: without the limit all of it fits on one row
        assert_eq!(
            draw(&layout(&narrow, Some(6), 80)),
            [
                (0, "        ← (● one two     )".to_string()),
                (1, "          (  three four  )".to_string()),
                (2, "          (  five six    )".to_string()),
                (3, "          (  seven eight )".to_string()),
            ]
        );
        // under the line too
        let under = layout(&narrow, None, 80);
        assert_eq!(under[0].row, 1);
        assert_eq!(under.len(), 4);
        // the edge of the view, when it is nearer
        assert_eq!(layout(&narrow, Some(6), 16).len(), 5);
        narrow.max_width = 0;
        assert_eq!(layout(&narrow, Some(6), 80).len(), 1);
    }

    /// With corners, a message wrapped over several rows is one block: corners on its first
    /// and last row and straight sides between, not a row with caps for each of its lines.
    /// A message of one row keeps the caps.
    #[test]
    fn with_corners_a_wrapped_message_is_one_block() {
        let mut blocks = panel(&[
            "one two three four five",
            "second",
            "aa bb cc dd ee ff gg hh ii jj kk ll",
        ]);
        blocks.corners = Some(corners(["/", "\\", "L", "J"]));
        blocks.footer = Some("+2 more".to_string());
        // 30 columns: the text gets 15 (see the test above); the box is 14 wide
        let rows = layout(&blocks, Some(6), 30);
        let row = |left: &str, icon: &str, text: &str, right: &str| {
            format!("{}{left}{icon} {text:<14} {right}", " ".repeat(10))
        };
        let drawn = draw(&rows);
        let expected = [
            row("/", "●", "one two three", "\\").replacen("          ", "        ← ", 1),
            row("L", " ", "four five", "J"),
            row("(", "●", "second", ")"),
            row("/", "●", "aa bb cc dd ee", "\\"),
            row(" ", " ", "ff gg hh ii jj", " "),
            row("L", " ", "kk ll", "J"),
            row("(", " ", "+2 more", ")"),
        ];
        for (n, (drawn, expected)) in drawn.iter().zip(&expected).enumerate() {
            assert_eq!(drawn, &(n, expected.clone()), "row {n}");
        }
        assert_eq!(drawn.len(), expected.len());

        // the corners are drawn like the caps; the sides between them are the panel's body
        let caps = |row: &PanelRow| -> Vec<String> {
            row.spans
                .iter()
                .filter(|span| span.part == PanelPart::Cap)
                .map(|span| span.text.clone())
                .collect()
        };
        assert_eq!(caps(&rows[3]), ["/", "\\"]);
        assert_eq!(caps(&rows[4]), Vec::<String>::new());
        assert_eq!(caps(&rows[5]), ["L", "J"]);
        assert_eq!(caps(&rows[2]), ["(", ")"]);
        // every row covers the same columns
        let extent = |row: &PanelRow| {
            let first = row.spans.iter().find(|span| span.part != PanelPart::Arrow);
            let last = row.spans.last().unwrap();
            (first.unwrap().col, last.col + width(&last.text))
        };
        assert!(rows.iter().all(|row| extent(row) == (10, 29)), "{rows:#?}");

        // without corners every row has the caps, as before
        blocks.corners = None;
        let rows = layout(&blocks, Some(6), 30);
        assert!(rows.iter().all(|row| caps(row) == ["(", ")"]));
    }

    /// Corners wider or narrower than the caps: the sides are as wide as the widest, the
    /// narrower padded on the inside, so the text of every row starts in the same column.
    #[test]
    fn corners_and_caps_of_different_widths_line_up() {
        let mut blocks = panel(&["one two three four five", "second"]);
        blocks.corners = Some(corners(["<<", ">>", "", ""]));
        let rows = layout(&blocks, Some(6), 32);
        let text_col = |row: &PanelRow| {
            let span = row
                .spans
                .iter()
                .find(|span| span.part == PanelPart::Text && span.text.trim() != "")
                .unwrap();
            span.col + (span.text.len() - span.text.trim_start().len())
        };
        // (`one`, `four` and `second` all start in column 14)
        assert_eq!(
            draw(&rows),
            [
                (0, "        ← <<● one two three >>".to_string()),
                (1, "              four five       ".to_string()),
                (2, "          ( ● second         )".to_string()),
            ]
        );
        assert_eq!(text_col(&rows[0]), text_col(&rows[2]));
    }

    #[test]
    fn too_few_columns_beside_the_line_put_it_under() {
        // 28 - (21 + 3) - 5 = -1 columns would be left beside the line; under it, the text
        // gets 28 - 2 - 5 = 21.
        let rows = layout(&panel(&["one two three four five"]), Some(20), 28);
        assert_eq!(
            draw(&rows),
            [
                (1, "╰ (● one two three four )".to_string()),
                (2, "  (  five               )".to_string()),
            ]
        );
        // Below `min_width` too, even if the text would fit: 34 - 10 - 5 = 19 columns.
        let mut min = panel(&["short"]);
        min.min_width = 20;
        assert_eq!(layout(&min, Some(6), 34)[0].row, 1);
        min.min_width = 19;
        assert_eq!(layout(&min, Some(6), 34)[0].row, 0);
        // The end of the line out of view (scrolled to the side).
        assert_eq!(layout(&panel(&["x"]), None, 40)[0].row, 1);
    }

    #[test]
    fn a_footer_has_no_icon_and_rows_are_one_box() {
        let mut with_footer = panel(&["a much longer message", "b"]);
        with_footer.footer = Some("+2 more".to_string());
        let rows = layout(&with_footer, Some(0), 80);
        assert_eq!(
            draw(&rows),
            [
                (0, "  ← (● a much longer message )".to_string()),
                (1, "    (● b                     )".to_string()),
                (2, "    (  +2 more               )".to_string()),
            ]
        );
        assert!(rows[2]
            .spans
            .iter()
            .all(|span| !matches!(span.part, PanelPart::Icon(_))));
    }

    #[test]
    fn long_words_line_breaks_and_wide_chars_wrap_by_columns() {
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("first\nsecond line", 20), ["first", "second line"]);
        assert_eq!(wrap("a\tb", 20), ["a b"]);
        assert_eq!(wrap("", 20), [""]);
        // 2 columns each
        assert_eq!(wrap("名前名前", 5), ["名前", "名前"]);
    }

    #[test]
    fn nothing_when_there_is_no_room_at_all_or_nothing_to_show() {
        assert!(layout(&panel(&["x"]), Some(6), 4).is_empty());
        assert!(layout(&panel(&[]), Some(6), 80).is_empty());
        assert!(layout(&panel(&["x"]), Some(usize::MAX), 80)[0].row == 1);
    }
}
