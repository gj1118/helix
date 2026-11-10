//! Panels plugins show next to a line (`buffer:set_panel`), modelled after
//! tiny-inline-diagnostic.nvim: drawn over the text, so they never move the lines of the
//! document or change scrolling, whatever the size of the view.
//!
//! The panel starts after the end of its line, behind an arrow, its messages wrapped to the
//! columns left; the rows they wrap onto, and the messages after the first, are drawn over the
//! lines below, aligned with the first row. With fewer than [`PluginPanel::min_width`] columns
//! left for the text, the panel goes under the line instead, from the left edge of the view.
//! The layout is computed afresh for every frame, so it follows resizes and edits at once.

use helix_core::unicode::segmentation::UnicodeSegmentation;
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
    /// Fewer columns than this left for the text beside the line put the panel under it.
    pub min_width: u16,
    /// Whether the panel is shown in insert mode.
    pub show_in_insert: bool,
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
            min_width: 30,
            show_in_insert: false,
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
    // The caps, the icon, and a space on either side of the text.
    let frame = width(&panel.left_cap) + icon_width + 2 + width(&panel.right_cap);

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
            text_width,
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
                    (1, 0, &panel.under_arrow, panel_col, text_width)
                }
                _ => return Vec::new(),
            }
        }
    };

    // Each item's lines (with its icon on the first), then the footer's.
    let mut lines: Vec<(Option<usize>, bool, String)> = Vec::new();
    for (idx, item) in panel.items.iter().enumerate() {
        for (n, line) in wrap(&item.text, text_width).into_iter().enumerate() {
            lines.push((Some(idx), n == 0, line));
        }
    }
    if let Some(footer) = &panel.footer {
        for line in wrap(footer, text_width) {
            lines.push((None, false, line));
        }
    }
    // A box: every row as wide as the widest.
    let box_width = lines
        .iter()
        .map(|(_, _, line)| width(line))
        .max()
        .unwrap_or(0);

    let mut rows = Vec::with_capacity(lines.len());
    for (n, (item, first, line)) in lines.into_iter().enumerate() {
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
        push(panel.left_cap.clone(), PanelPart::Cap);
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
        push(format!(" {line}{} ", " ".repeat(pad)), PanelPart::Text);
        push(panel.right_cap.clone(), PanelPart::Cap);
        rows.push(PanelRow {
            row: first_row + n,
            spans,
        });
    }
    rows
}

/// `text` broken into lines at most `max_width` columns wide: at its line breaks, between
/// words where possible, within a word longer than a line. Tabs become spaces.
fn wrap(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.replace('\t', "    ").lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let sep = usize::from(!line.is_empty());
            if width(&line) + sep + width(word) <= max_width {
                if sep == 1 {
                    line.push(' ');
                }
                line.push_str(word);
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word longer than a line is cut where the line is full.
            for grapheme in word.graphemes(true) {
                if width(&line) + width(grapheme) > max_width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(grapheme);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
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
