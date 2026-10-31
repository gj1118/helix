//! Code lens labels after the end of the line they belong to.

use std::collections::HashMap;

use helix_core::Position;
use helix_view::theme::Style;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Blank columns between the end of the line (or of what other decorations put there) and the
/// label. The offset a decoration is given is already one column past the text, hence 1 for the
/// two spaces that are seen.
const GAP: usize = 1;

pub struct CodeLensDecoration {
    /// The label of every document line that has lenses.
    labels: HashMap<usize, String>,
    style: Style,
}

impl CodeLensDecoration {
    pub fn new(labels: HashMap<usize, String>, style: Style) -> Self {
        Self { labels, style }
    }
}

/// `▶ Run · Debug` from the titles of the lenses of one line; `None` when none has a title yet.
pub fn label<'a>(titles: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let titles: Vec<String> = titles
        .into_iter()
        // A title is one line of text: control characters (and the line/paragraph separators
        // some servers send) would break the layout.
        .map(|title| {
            title
                .chars()
                .map(|c| {
                    if c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect::<String>()
        })
        .filter(|title| !title.trim().is_empty())
        .collect();
    (!titles.is_empty()).then(|| format!("▶ {}", titles.join(" · ")))
}

impl Decoration for CodeLensDecoration {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // Soft wrapping spreads a line over several rows; the label goes after the last one.
        if !pos.is_last_visual_line {
            return Position::new(0, 0);
        }
        let Some(label) = self.labels.get(&pos.doc_line) else {
            return Position::new(0, 0);
        };

        // Where the text of the line (plus what earlier decorations drew) ends, in columns of
        // the whole line, then in columns of the viewport.
        let end_of_line = virt_off.col;
        let start = end_of_line + GAP;
        let Some(start_on_screen) = start.checked_sub(renderer.offset.col) else {
            // scrolled so far to the right that the label is off to the left
            return Position::new(0, 0);
        };
        let width = (renderer.viewport.width as usize).saturating_sub(start_on_screen);
        // Not even room for the arrow and a letter: leave the line alone.
        if width < 3 {
            return Position::new(0, 0);
        }

        let (stopped_at, _) = renderer.set_string_truncated(
            renderer.viewport.x + start_on_screen as u16,
            pos.visual_line,
            label,
            width,
            |_| self.style,
            true,
            false,
        );

        // `stopped_at` is a column of the terminal; the decoration reports columns of the line.
        let stopped_in_viewport = stopped_at.saturating_sub(renderer.viewport.x) as usize;
        let line_end_on_screen = end_of_line.saturating_sub(renderer.offset.col);
        Position::new(0, stopped_in_viewport.saturating_sub(line_end_on_screen))
    }
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn label_joins_titles() {
        assert_eq!(label(["Run", "Debug"]).as_deref(), Some("▶ Run · Debug"));
    }

    #[test]
    fn label_ignores_empty_titles_and_control_characters() {
        assert_eq!(label(["", "  "]), None);
        assert_eq!(label([]), None);
        assert_eq!(
            label(["3\nreferences", "\u{2028}"]).as_deref(),
            Some("▶ 3 references")
        );
    }
}
