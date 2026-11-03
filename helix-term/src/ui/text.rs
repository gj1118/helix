use crate::compositor::{Component, Context};
use tui::buffer::Buffer as Surface;

use helix_view::graphics::Rect;

pub struct Text {
    pub(crate) contents: tui::text::Text<'static>,
    size: (u16, u16),
    viewport: (u16, u16),
}

impl Text {
    pub fn new(contents: String) -> Self {
        Self {
            contents: tui::text::Text::from(contents),
            size: (0, 0),
            viewport: (0, 0),
        }
    }
}

impl From<tui::text::Text<'static>> for Text {
    fn from(contents: tui::text::Text<'static>) -> Self {
        Self {
            contents,
            size: (0, 0),
            viewport: (0, 0),
        }
    }
}

impl Component for Text {
    fn render(&mut self, area: Rect, surface: &mut Surface, _cx: &mut Context) {
        use tui::widgets::{Paragraph, Widget, Wrap};

        let par = Paragraph::new(&self.contents).wrap(Wrap { trim: false });
        // .scroll(x, y) offsets

        par.render(area, surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        if viewport != self.viewport {
            // Clamped before the cast: a size past u16::MAX must not wrap around.
            let width = self.contents.width().min(viewport.0 as usize) as u16;
            let height = self.contents.height().min(viewport.1 as usize) as u16;
            self.size = (width, height);
            self.viewport = viewport;
        }
        Some(self.size)
    }
}

/// The largest height [`required_size`] returns: a huge document (or a very long line)
/// must not overflow the height, nor the sums of callers adding a border, padding or a
/// header to it.
pub const MAX_REQUIRED_HEIGHT: u16 = u16::MAX - 256;

/// The size `text` takes wrapped at `max_text_width` columns. The height saturates at
/// [`MAX_REQUIRED_HEIGHT`].
pub fn required_size(text: &tui::text::Text, max_text_width: u16) -> (u16, u16) {
    let max_text_width = max_text_width as usize;
    let mut text_width = 0;
    let mut height: usize = 0;
    for content in &text.lines {
        height += 1;
        let content_width = content.width();
        if content_width > max_text_width {
            text_width = max_text_width;
            height += content_width.checked_div(max_text_width).unwrap_or(0);
        } else if content_width > text_width {
            text_width = content_width;
        }
    }
    (
        text_width as u16,
        height.min(MAX_REQUIRED_HEIGHT as usize) as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_texts_saturate_instead_of_overflowing() {
        let many_lines = tui::text::Text::from("line\n".repeat(70_000));
        assert_eq!(required_size(&many_lines, 80), (4, MAX_REQUIRED_HEIGHT));

        let long_line = tui::text::Text::from("x".repeat(70_000));
        assert_eq!(
            required_size(&long_line, 80),
            (80, 1 + (70_000 / 80) as u16)
        );
        // without wrapping the width is what there is room for, not a wrapped-around u16
        let mut text = Text::from(tui::text::Text::from("x".repeat(65_600)));
        assert_eq!(text.required_size((120, 10)), Some((120, 1)));
    }
}
