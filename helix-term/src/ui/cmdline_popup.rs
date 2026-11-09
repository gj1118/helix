use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::completers::CompletionResult;
use crate::ui::gradient_border::GradientBorder;
use crate::ui::prompt::{Prompt, PromptEvent};
use helix_core::{unicode::width::UnicodeWidthStr, Position};
use helix_view::{
    editor::CmdlineStyle,
    graphics::{CursorKind, Rect},
    Editor,
};
use std::borrow::Cow;
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// Most completions the list below the input shows at once.
const MAX_COMPLETIONS: usize = 10;

pub struct CmdlinePopup {
    prompt: Prompt,
    style: CmdlineStyle,
    // Popup-specific properties
    popup_area: Rect,
    min_width: u16,
    max_width: u16,
    padding: u16,
    // Gradient border for cmdline popup
    gradient_border: Option<GradientBorder>,
}

impl CmdlinePopup {
    pub fn new(
        prompt_text: Cow<'static, str>,
        history_register: Option<char>,
        completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
        callback_fn: impl FnMut(&mut Context, &str, PromptEvent) + 'static,
        style: CmdlineStyle,
    ) -> Self {
        Self {
            prompt: Prompt::new(prompt_text, history_register, completion_fn, callback_fn),
            style,
            popup_area: Rect::default(),
            min_width: 40,
            max_width: 80,
            padding: 2,
            gradient_border: None,
        }
    }

    /// The prompt the popup wraps, e.g. to deliver completions computed in the background.
    pub fn prompt_mut(&mut self) -> &mut Prompt {
        &mut self.prompt
    }

    pub fn with_line(mut self, line: String, editor: &Editor) -> Self {
        self.prompt = self.prompt.with_line(line, editor);
        self
    }

    pub fn with_language(
        mut self,
        language: &'static str,
        loader: std::sync::Arc<arc_swap::ArcSwap<helix_core::syntax::Loader>>,
    ) -> Self {
        self.prompt = self.prompt.with_language(language, loader);
        self
    }

    /// Calculate optimal popup dimensions and position, inside `viewport`.
    fn calculate_popup_area(&self, viewport: Rect) -> Rect {
        let content_width = self.prompt.line().width().max(self.min_width as usize);
        // In usize: a very long line must not overflow (or wrap around to a narrow popup).
        let width = content_width
            .saturating_add(self.padding as usize * 2)
            .min(self.max_width as usize)
            .min(viewport.width.saturating_sub(4) as usize) as u16;

        // Base height for single line + borders, unless the screen is shorter.
        let height = 3.min(viewport.height);

        let x = viewport.x + (viewport.width.saturating_sub(width)) / 2;
        let y = viewport.y + (viewport.height.saturating_sub(height)) / 3; // Position in upper third

        Rect::new(x, y, width, height)
    }

    /// Get command type icon based on the input
    fn get_command_icon<'a>(&self, config: &'a helix_view::editor::CmdlineIcons) -> &'a str {
        let line = self.prompt.line();
        if line.starts_with("search:") || line.starts_with("/") || line.starts_with("?") {
            &config.search
        } else if line.starts_with(":") {
            &config.command
        } else if line.starts_with('!') {
            &config.shell
        } else {
            // Check if this is a regex prompt by looking at the prompt text
            match self.prompt.prompt() {
                s if s.starts_with("search:") || s == "Search" => &config.search,
                "Cmdline" => &config.command,
                _ => &config.general,
            }
        }
    }

    /// Render popup-style cmdline
    fn render_popup(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let popup_area = self.calculate_popup_area(area);
        self.popup_area = popup_area;

        let theme = &cx.editor.theme;
        let config = &cx.editor.config().gradient_borders;

        // Only inside the border (see `clear_bordered`).
        crate::ui::clear_bordered(
            surface,
            popup_area,
            theme.get("ui.popup"),
            !crate::ui::border_cells_take_background(&cx.editor.config()),
        );

        // Use gradient border if enabled, otherwise use default border
        let inner_area = if config.enable && self.style == CmdlineStyle::Popup {
            // Initialize gradient border if needed
            if self.gradient_border.is_none() {
                self.gradient_border = Some(GradientBorder::from_theme(theme, config));
            }

            // Render gradient border with title
            if let Some(ref mut gradient_border) = self.gradient_border {
                let rounded = cx.editor.config().border_rounded();
                if !cx.editor.config().borders_hidden() {
                    gradient_border.render_with_title(
                        popup_area,
                        surface,
                        theme,
                        Some(self.prompt.prompt()),
                        rounded,
                    );
                }
            }

            // Inner area inside the gradient border
            {
                // The gradient border is one cell wide whatever its thickness.
                crate::ui::gradient_border::GradientBorder::inner_area(popup_area)
            }
        } else {
            // Use traditional border
            let border_style = crate::ui::border_style(theme);

            let border_type = crate::ui::border_type(&cx.editor.config());
            let block = Block::default()
                .borders(tui::widgets::Borders::ALL)
                .border_type(border_type)
                .border_style(border_style);
            // The background is painted inside the border above (see `clear_bordered`).
            // `ui.popup.title` when the theme sets it; otherwise the border's style.
            let block = match theme.try_get("ui.popup.title") {
                Some(style) => block.title(tui::text::Span::styled(self.prompt.prompt(), style)),
                None => block.title(self.prompt.prompt()),
            };

            let inner_area = block.inner(popup_area);
            block.render(popup_area, surface);
            inner_area
        };

        // Render command icon (if enabled) but not the prompt text since it's now on the border
        let config = cx.editor.config();
        let icon = if config.cmdline.show_icons {
            self.get_command_icon(&config.cmdline.icons)
        } else {
            ""
        };
        // Render icon without trailing space to avoid extra padding before input
        let prefix_text = if icon.is_empty() {
            "".to_string()
        } else {
            icon.to_string()
        };

        if !prefix_text.is_empty() {
            let prompt_color = theme.get("ui.text.focus");
            // Make the icon more prominent with bold styling
            let icon_style = prompt_color.add_modifier(helix_view::theme::Modifier::BOLD);
            surface.set_string(inner_area.x, inner_area.y, &prefix_text, icon_style);
        }

        // Calculate input area
        let input_area = Rect::new(
            inner_area.x + prefix_text.width() as u16,
            inner_area.y,
            inner_area.width.saturating_sub(prefix_text.width() as u16),
            1,
        );

        // Render input text with syntax highlighting if available (unless the screen is too
        // short to show it inside the border).
        if inner_area.height > 0 && input_area.width > 0 {
            self.render_input_text(input_area, surface, cx);
        }

        // Render completion popup if needed
        if !self.prompt.completions().is_empty() {
            self.render_completion_popup(popup_area, area, surface, cx);
        }
    }

    /// Render the input text with horizontal scrolling support
    fn render_input_text(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let theme = &cx.editor.theme;
        let text_style = theme.get("ui.text");
        let line_width = area.width as usize;

        // Update scroll anchor based on cursor position and available width
        self.prompt.update_scroll_anchor(line_width);

        let anchor = self.prompt.anchor();
        let line = self.prompt.line();
        let visible_text = &line[anchor..];

        // Use set_string_anchored to handle truncation indicators
        surface.set_string_anchored(
            area.x,
            area.y,
            self.prompt.truncate_start(),
            self.prompt.truncate_end(),
            visible_text,
            line_width,
            |_| text_style,
        );
    }

    /// The area of the completion list: one row below the input popup (`base_area`), as
    /// tall as the items need (at most `MAX_COMPLETIONS`) and the screen (`viewport`)
    /// allows; empty when not even one item fits.
    fn completion_area(base_area: Rect, viewport: Rect, items: usize) -> Rect {
        let y = base_area.bottom().saturating_add(1);
        let available = viewport.bottom().saturating_sub(y);
        // Fixed height based on visible items + borders
        let height = (items.min(MAX_COMPLETIONS) as u16 + 2).min(available);
        if height < 3 {
            return Rect::new(base_area.x, y, 0, 0);
        }
        Rect::new(base_area.x, y, base_area.width, height)
    }

    /// Render completion popup
    fn render_completion_popup(
        &mut self,
        base_area: Rect,
        viewport: Rect,
        surface: &mut Surface,
        cx: &Context,
    ) {
        let theme = &cx.editor.theme;
        // Match global autocomplete/picker colors
        let completion_bg = theme.get("ui.menu");
        let selected_row_bg = theme.get("ui.menu.selected");

        // Position completion popup below the main popup, inside the screen: on a short
        // terminal fewer items are shown (a frame drawn past the bottom of the screen
        // would write outside of it).
        let total_items = self.prompt.completions().len();
        let comp_area = Self::completion_area(base_area, viewport, total_items);
        if comp_area.width < 2 {
            return;
        }
        // as many items as there are rows inside the border
        let max_display_items = (comp_area.height - 2) as usize;

        // Clear and render completion background (match global autocomplete), only
        // inside the border (see `clear_bordered`).
        crate::ui::clear_bordered(
            surface,
            comp_area,
            completion_bg,
            !crate::ui::border_cells_take_background(&cx.editor.config()),
        );

        let config = &cx.editor.config().gradient_borders;

        // Use gradient border for completion if enabled
        let inner_area = if config.enable && self.style == CmdlineStyle::Popup {
            // Reuse the gradient border for completion popup
            if let Some(ref mut gradient_border) = self.gradient_border {
                let rounded = cx.editor.config().border_rounded();
                if !cx.editor.config().borders_hidden() {
                    gradient_border.render(comp_area, surface, &cx.editor.theme, rounded);
                }
            }

            // Inner area inside the gradient border
            {
                // The gradient border is one cell wide whatever its thickness.
                crate::ui::gradient_border::GradientBorder::inner_area(comp_area)
            }
        } else {
            // Use traditional border, matching popup card styling
            let border_type = crate::ui::border_type(&cx.editor.config());
            let block = Block::default()
                .borders(tui::widgets::Borders::ALL)
                .border_type(border_type)
                .border_style(crate::ui::border_style(theme));

            let inner_area = block.inner(comp_area);
            block.render(comp_area, surface);
            inner_area
        };

        // Render completion items with scrolling support
        let config = cx.editor.config();
        let picker_symbol = config.picker_symbol.as_str();
        let symbol_width = picker_symbol.width();

        let completions = self.prompt.completions();
        let selected_index = self.prompt.selection().unwrap_or(0);

        // Calculate scroll offset to keep selected item visible within the fixed window
        let scroll_offset = if selected_index >= max_display_items {
            selected_index.saturating_sub(max_display_items - 1)
        } else {
            0
        };

        // Render visible completion items
        for (display_idx, (completion_idx, (_range, completion))) in completions
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(max_display_items)
            .enumerate()
        {
            let y = inner_area.y + display_idx as u16;
            let is_selected = self.prompt.selection() == Some(completion_idx);
            let item_style = if is_selected {
                // Fill the whole selected row across the popup width (the row is blank:
                // the list was cleared above).
                let row = Rect::new(inner_area.x, y, inner_area.width, 1);
                surface.set_style(row.intersection(surface.area), selected_row_bg);
                // Use the theme's selected style for text (fg+bg from ui.menu.selected)
                selected_row_bg
            } else {
                completion_bg.patch(completion.style)
            };

            let prefix = if is_selected {
                picker_symbol.to_string()
            } else {
                " ".repeat(symbol_width)
            };
            let text = format!("{}{}", prefix, completion.content);
            surface.set_stringn(
                inner_area.x,
                y,
                &text,
                inner_area.width as usize,
                item_style,
            );
        }

        // Add scroll indicators if there are more items
        if total_items > max_display_items {
            let scroll_indicator_style = theme.get("ui.text.inactive");

            // Show up arrow if we can scroll up
            if scroll_offset > 0 {
                surface.set_string(
                    inner_area.x + inner_area.width.saturating_sub(1),
                    inner_area.y,
                    "↑",
                    scroll_indicator_style,
                );
            }

            // Show down arrow if we can scroll down
            if scroll_offset + max_display_items < total_items {
                surface.set_string(
                    inner_area.x + inner_area.width.saturating_sub(1),
                    inner_area.y + inner_area.height.saturating_sub(1),
                    "↓",
                    scroll_indicator_style,
                );
            }
        }
    }

    /// Render traditional bottom cmdline
    fn render_bottom(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // Delegate to the original prompt rendering
        self.prompt.render_prompt(area, surface, cx);
    }
}

impl Component for CmdlinePopup {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        // Delegate event handling to the underlying prompt
        self.prompt.handle_event(event, cx)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        match self.style {
            CmdlineStyle::Popup => self.render_popup(area, surface, cx),
            CmdlineStyle::Bottom => self.render_bottom(area, surface, cx),
        }
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        match self.style {
            CmdlineStyle::Popup => {
                // Calculate cursor position for popup style
                let config = editor.config();
                let icon = if config.cmdline.show_icons {
                    self.get_command_icon(&config.cmdline.icons)
                } else {
                    ""
                };
                let prefix_width = if icon.is_empty() { 0 } else { icon.width() };

                // Compute inner area similar to render
                let inner_area = if editor.config().gradient_borders.enable {
                    // The gradient border is one cell wide whatever its thickness.
                    crate::ui::gradient_border::GradientBorder::inner_area(self.popup_area)
                } else {
                    Block::default()
                        .borders(tui::widgets::Borders::ALL)
                        .inner(self.popup_area)
                };

                // Build the same input area used in render_popup
                let input_area = Rect::new(
                    inner_area.x + prefix_width as u16,
                    inner_area.y,
                    inner_area.width.saturating_sub(prefix_width as u16),
                    1,
                );

                // Compute cursor position accounting for horizontal scroll anchor
                let byte_pos = self.prompt.position();
                let anchor = self.prompt.anchor();
                let line = self.prompt.line();

                // Calculate cursor position relative to the visible portion (after anchor)
                // Also account for truncation indicator if text is scrolled
                let truncate_start = self.prompt.truncate_start();
                let visible_cursor_offset = if byte_pos >= anchor {
                    line[anchor..byte_pos].width()
                } else {
                    0
                };

                // Add 1 for the truncation indicator "…" if we're scrolled
                let indicator_offset = if truncate_start { 1 } else { 0 };
                let cursor_offset = (visible_cursor_offset + indicator_offset) as u16;
                let clamped_offset = cursor_offset.min(input_area.width.saturating_sub(1));
                let cursor_x = input_area.x as usize + clamped_offset as usize;
                let cursor_y = input_area.y as usize;

                (
                    Some(Position::new(cursor_y, cursor_x)),
                    editor
                        .config()
                        .cursor_shape
                        .from_mode(helix_view::document::Mode::Insert),
                )
            }
            CmdlineStyle::Bottom => {
                // Delegate to original prompt cursor calculation
                self.prompt.cursor(area, editor)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===========================================
    // CmdlinePopup Construction Tests
    // ===========================================

    #[test]
    fn test_cmdline_popup_default_properties() {
        let popup = CmdlinePopup::new(
            "Test:".into(),
            None,
            crate::ui::completers::none,
            |_ctx, _input, _event| {},
            CmdlineStyle::Popup,
        );

        assert_eq!(popup.style, CmdlineStyle::Popup);
        assert_eq!(popup.min_width, 40);
        assert_eq!(popup.max_width, 80);
        assert_eq!(popup.padding, 2);
        assert!(popup.gradient_border.is_none());
        assert_eq!(popup.popup_area, Rect::default());
    }

    #[test]
    fn test_cmdline_popup_bottom_style() {
        let popup = CmdlinePopup::new(
            ":".into(),
            None,
            crate::ui::completers::none,
            |_ctx, _input, _event| {},
            CmdlineStyle::Bottom,
        );

        assert_eq!(popup.style, CmdlineStyle::Bottom);
    }

    // ===========================================
    // Popup Area Calculation Tests
    // ===========================================

    #[test]
    fn test_calculate_popup_area_centered() {
        let popup = CmdlinePopup::new(
            ":".into(),
            None,
            crate::ui::completers::none,
            |_ctx, _input, _event| {},
            CmdlineStyle::Popup,
        );

        let viewport = Rect::new(0, 0, 100, 50);
        let area = popup.calculate_popup_area(viewport);

        // Width should be between min_width (40) and max_width (80)
        assert!(area.width >= popup.min_width);
        assert!(area.width <= popup.max_width);

        // Height should be 3 (base height for single line + borders)
        assert_eq!(area.height, 3);

        // Should be horizontally centered
        let expected_x = viewport.x + (viewport.width.saturating_sub(area.width)) / 2;
        assert_eq!(area.x, expected_x);

        // Should be positioned in upper third
        let expected_y = viewport.y + (viewport.height.saturating_sub(area.height)) / 3;
        assert_eq!(area.y, expected_y);
    }

    #[test]
    fn test_calculate_popup_area_small_viewport() {
        let popup = CmdlinePopup::new(
            ":".into(),
            None,
            crate::ui::completers::none,
            |_ctx, _input, _event| {},
            CmdlineStyle::Popup,
        );

        // Viewport smaller than max_width
        let viewport = Rect::new(0, 0, 50, 20);
        let area = popup.calculate_popup_area(viewport);

        // Width should be constrained by viewport
        assert!(area.width <= viewport.width.saturating_sub(4));
    }

    #[test]
    fn popup_and_completions_stay_on_screen() {
        let mut popup = CmdlinePopup::new(
            ":".into(),
            None,
            crate::ui::completers::none,
            |_ctx, _input, _event| {},
            CmdlineStyle::Popup,
        );
        for (width, height) in [
            (0, 0),
            (1, 1),
            (3, 2),
            (10, 3),
            (80, 5),
            (80, 12),
            (200, 60),
        ] {
            let viewport = Rect::new(0, 0, width, height);
            let area = popup.calculate_popup_area(viewport);
            assert_eq!(viewport.intersection(area), area, "{viewport:?}");
            for items in [1, 10, 500] {
                let list = CmdlinePopup::completion_area(area, viewport, items);
                if list.height > 0 {
                    assert_eq!(viewport.intersection(list), list, "{viewport:?}");
                    assert!(list.height <= items as u16 + 2);
                    assert!(list.height <= MAX_COMPLETIONS as u16 + 2);
                }
            }
        }
        // a tall screen shows ten completions, a short one what fits
        let area = popup.calculate_popup_area(Rect::new(0, 0, 80, 40));
        let list = CmdlinePopup::completion_area(area, Rect::new(0, 0, 80, 40), 500);
        assert_eq!(list.height, 12);
        let short = Rect::new(0, 0, 80, 12);
        let area = popup.calculate_popup_area(short);
        let list = CmdlinePopup::completion_area(area, short, 500);
        assert_eq!(list.bottom(), short.bottom());

        // a very long line doesn't overflow the width
        let viewport = Rect::new(0, 0, 100, 30);
        popup.prompt.set_line_unchecked("x".repeat(70_000));
        let area = popup.calculate_popup_area(viewport);
        assert_eq!(area.width, popup.max_width);
    }

    // ===========================================
    // CmdlineStyle Tests
    // ===========================================

    #[test]
    fn test_cmdline_style_popup_variant() {
        let style = CmdlineStyle::Popup;
        assert_eq!(style, CmdlineStyle::Popup);
    }

    #[test]
    fn test_cmdline_style_bottom_variant() {
        let style = CmdlineStyle::Bottom;
        assert_eq!(style, CmdlineStyle::Bottom);
    }

    #[test]
    fn test_cmdline_style_equality() {
        assert_eq!(CmdlineStyle::Popup, CmdlineStyle::Popup);
        assert_eq!(CmdlineStyle::Bottom, CmdlineStyle::Bottom);
        assert_ne!(CmdlineStyle::Popup, CmdlineStyle::Bottom);
    }
}
