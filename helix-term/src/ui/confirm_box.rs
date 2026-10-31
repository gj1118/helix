//! A small modal yes/no dialog.
//!
//! Used by the explorer to confirm destructive actions (delete, overwrite on
//! paste). The box takes every key and mouse event while it is open, so
//! nothing underneath can be triggered by accident. It defaults to the safe
//! answer: plain `Enter` cancels unless the confirm button was selected.

use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::{
    graphics::Rect,
    input::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind},
};
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::gradient_border::PanelBorder;

type OnConfirm = Box<dyn FnOnce(&mut Compositor, &mut Context)>;

/// Widest the message text may get; longer lines wrap.
const MAX_TEXT_WIDTH: usize = 60;

pub struct ConfirmBox {
    title: String,
    message: String,
    confirm_label: String,
    /// Whether the confirm button (rather than cancel) is highlighted.
    confirm_selected: bool,
    on_confirm: Option<OnConfirm>,
    // Button hit areas of the last render, for the mouse.
    confirm_button: Rect,
    cancel_button: Rect,
    /// Follows `[editor.gradient-borders]`, otherwise the common border color.
    border: PanelBorder,
}

impl ConfirmBox {
    /// `on_confirm` runs after the box has been closed, only when the user
    /// confirms.
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: impl Into<String>,
        on_confirm: impl FnOnce(&mut Compositor, &mut Context) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm_label: confirm_label.into(),
            confirm_selected: false,
            on_confirm: Some(Box::new(on_confirm)),
            confirm_button: Rect::default(),
            cancel_button: Rect::default(),
            border: PanelBorder::default(),
        }
    }

    fn confirm_text(&self) -> String {
        format!(" {} (y) ", self.confirm_label)
    }

    fn cancel_text() -> &'static str {
        " Cancel (n) "
    }

    fn close(&mut self) -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor, _cx| {
            compositor.pop();
        })))
    }

    fn confirm(&mut self) -> EventResult {
        let on_confirm = self.on_confirm.take();
        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.pop();
            if let Some(on_confirm) = on_confirm {
                on_confirm(compositor, cx);
            }
        })))
    }

    fn on_key(&mut self, key: KeyEvent) -> EventResult {
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
            return self.close();
        }
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return EventResult::Consumed(None);
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') | KeyCode::Esc => {
                self.close()
            }
            KeyCode::Enter => {
                if self.confirm_selected {
                    self.confirm()
                } else {
                    self.close()
                }
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                self.confirm_selected = !self.confirm_selected;
                EventResult::Consumed(None)
            }
            _ => EventResult::Consumed(None),
        }
    }
}

/// Greedy word wrap to `width` columns; blank lines in `text` are kept.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}

impl Component for ConfirmBox {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.on_key(*key),
            Event::Mouse(mouse) => {
                if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                    if contains(self.confirm_button, mouse.column, mouse.row) {
                        return self.confirm();
                    }
                    if contains(self.cancel_button, mouse.column, mouse.row) {
                        return self.close();
                    }
                }
                // Modal: nothing reaches the layers underneath.
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        let border_style = crate::ui::border_style(theme);
        let text_style = theme.get("ui.text");
        let selected_style = theme.get("ui.menu.selected");

        let confirm_text = self.confirm_text();
        let cancel_text = Self::cancel_text();
        let buttons_width = confirm_text.width() + 2 + cancel_text.width();

        let max_text = MAX_TEXT_WIDTH
            .min(area.width.saturating_sub(8) as usize)
            .max(10);
        let lines = wrap(&self.message, max_text);
        let inner_width = lines
            .iter()
            .map(|line| line.width())
            .chain([buttons_width, self.title.width()])
            .max()
            .unwrap_or(0);

        // border (1) + padding (1) on each side.
        let width = ((inner_width + 4) as u16).min(area.width);
        // border, message, blank line, buttons, border.
        let height = ((lines.len() + 4) as u16).min(area.height);
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 3,
            width,
            height,
        );

        surface.clear_with(rect, background);
        self.border.render(rect, surface, cx.editor);
        let inner = rect.inner(helix_view::graphics::Margin::all(1));
        if inner.width < 3 || inner.height == 0 {
            return;
        }
        // The title sits in the top border, where the border drew its line.
        let title_width = inner.width.saturating_sub(2) as usize;
        surface.set_stringn(
            inner.x + 1,
            rect.y,
            self.title.as_str(),
            title_width,
            border_style,
        );

        let text_x = inner.x + 1;
        let text_width = inner.width.saturating_sub(2) as usize;
        for (i, line) in lines.iter().enumerate() {
            let y = inner.y + i as u16;
            // Keep the last row for the buttons.
            if y + 1 >= inner.bottom() {
                break;
            }
            surface.set_stringn(text_x, y, line, text_width, text_style);
        }

        // Buttons, centered on the last inner row.
        let y = inner.bottom().saturating_sub(1);
        let buttons_x = inner.x + (inner.width.saturating_sub(buttons_width as u16)) / 2;
        self.confirm_button = Rect::new(buttons_x, y, confirm_text.width() as u16, 1);
        self.cancel_button = Rect::new(
            buttons_x + confirm_text.width() as u16 + 2,
            y,
            cancel_text.width() as u16,
            1,
        );
        let (confirm_style, cancel_style) = if self.confirm_selected {
            (selected_style, text_style)
        } else {
            (text_style, selected_style)
        };
        surface.set_string(self.confirm_button.x, y, &confirm_text, confirm_style);
        surface.set_string(self.cancel_button.x, y, cancel_text, cancel_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words_and_keeps_blank_lines() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn wrap_leaves_overlong_words_whole() {
        assert_eq!(
            wrap("supercalifragilistic x", 5),
            vec!["supercalifragilistic", "x"]
        );
    }
}
