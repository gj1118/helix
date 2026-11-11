use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::completers::CompletionResult;
use crate::ui::gradient_border::GradientBorder;
use crate::ui::prompt::{Prompt, PromptEvent};
use helix_core::{unicode::width::UnicodeWidthStr, Position};
use helix_view::{
    editor::{CmdlineConfig, CmdlineStyle, GradientBorderConfig},
    graphics::{CursorKind, Rect, Style},
    Editor,
};
use std::borrow::Cow;
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// Most completions the list below the input shows at once.
const MAX_COMPLETIONS: usize = 10;

/// Columns between the input and the popup's border, on each side.
const PADDING: u16 = 2;

pub struct CmdlinePopup {
    prompt: Prompt,
    style: CmdlineStyle,
    /// Where the popup was last drawn.
    popup_area: Rect,
    /// The gradient border, with the configuration it was made from (kept for its animation).
    gradient_border: Option<(GradientBorderConfig, GradientBorder)>,
    /// The highlighted styles of the input (one per byte), for the line and theme they were
    /// computed for: highlighting parses the line, which a frame drawn for every keystroke
    /// (or a cursor blink) need not do again.
    highlights: Option<(String, String, Vec<Style>)>,
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
            gradient_border: None,
            highlights: None,
        }
    }

    /// The prompt the popup wraps, e.g. to deliver completions computed in the background.
    pub fn prompt_mut(&mut self) -> &mut Prompt {
        &mut self.prompt
    }

    #[cfg(test)]
    fn with_line_unchecked(mut self, line: &str) -> Self {
        self.prompt.set_line_unchecked(line.to_string());
        self
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

    /// The help shown below the popup for what is typed (e.g. the documentation of a command),
    /// like `Prompt::doc_fn` above the bottom command line.
    pub fn with_doc_fn(mut self, doc_fn: crate::ui::prompt::DocFn) -> Self {
        self.prompt.doc_fn = doc_fn;
        self
    }

    /// The popup's area inside `viewport`: as wide as the input needs, between the
    /// `min-popup-width` and `max-popup-width` of `[editor.cmdline]`, centered, in the upper
    /// third.
    fn calculate_popup_area(&self, viewport: Rect, config: &CmdlineConfig) -> Rect {
        let min_width = config.min_popup_width;
        // a maximum below the minimum is the minimum
        let max_width = config.max_popup_width.max(min_width);
        let content_width = self
            .prompt
            .line()
            .width()
            .saturating_add(PADDING as usize * 2)
            .max(min_width as usize);
        // In usize: a very long line must not overflow (or wrap around to a narrow popup).
        let width = content_width
            .min(max_width as usize)
            .min(viewport.width.saturating_sub(4) as usize) as u16;

        // Base height for single line + borders, unless the screen is shorter.
        let height = 3.min(viewport.height);

        let x = viewport.x + (viewport.width.saturating_sub(width)) / 2;
        let y = viewport.y + (viewport.height.saturating_sub(height)) / 3; // Position in upper third

        Rect::new(x, y, width, height)
    }

    /// The icon of the kind of prompt: from what it is (its history register and label), not
    /// from what is typed in it.
    fn get_command_icon<'a>(&self, icons: &'a helix_view::editor::CmdlineIcons) -> &'a str {
        match (self.prompt.history_register(), self.prompt.prompt()) {
            (Some(':'), _) | (_, "Cmdline") => &icons.command,
            (Some('|'), _) => &icons.shell,
            (Some('/'), _) | (_, "Search") => &icons.search,
            (_, prompt) if prompt.starts_with("search") => &icons.search,
            _ => &icons.general,
        }
    }

    /// The gradient border for `config`, made again when the configuration changed (a
    /// `:config-reload` while the popup is open).
    fn gradient_border(
        &mut self,
        theme: &helix_view::Theme,
        config: &GradientBorderConfig,
    ) -> &mut GradientBorder {
        if self
            .gradient_border
            .as_ref()
            .is_none_or(|(made_for, _)| made_for != config)
        {
            self.gradient_border =
                Some((config.clone(), GradientBorder::from_theme(theme, config)));
        }
        &mut self.gradient_border.as_mut().unwrap().1
    }

    /// Render popup-style cmdline
    fn render_popup(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let config = cx.editor.config();
        let popup_area = self.calculate_popup_area(area, &config.cmdline);
        self.popup_area = popup_area;

        let theme = &cx.editor.theme;

        // Only inside the border (see `clear_bordered`).
        crate::ui::clear_bordered(
            surface,
            popup_area,
            theme.get("ui.popup"),
            !crate::ui::border_cells_take_background(&config),
        );

        // Use gradient border if enabled, otherwise use default border
        let inner_area = if config.gradient_borders.enable {
            let rounded = config.border_rounded();
            let hidden = config.borders_hidden();
            let title = self.prompt.prompt().to_string();
            let gradient_border = self.gradient_border(theme, &config.gradient_borders);
            if !hidden {
                gradient_border.render_with_title(
                    popup_area,
                    surface,
                    theme,
                    Some(&title),
                    rounded,
                );
            }
            // The gradient border is one cell wide whatever its thickness.
            GradientBorder::inner_area(popup_area)
        } else {
            let block = Block::default()
                .borders(tui::widgets::Borders::ALL)
                .border_type(crate::ui::border_type(&config))
                .border_style(crate::ui::border_style(theme));
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

        // The icon of the kind of prompt, right before the input. The prompt's label is the
        // border's title.
        let icon = self.icon(&config);
        let icon_width = Self::icon_width(icon, inner_area);
        if inner_area.height > 0 && icon_width > 0 {
            let icon_style = theme
                .get("ui.text.focus")
                .add_modifier(helix_view::theme::Modifier::BOLD);
            surface.set_stringn(
                inner_area.x,
                inner_area.y,
                icon,
                icon_width as usize,
                icon_style,
            );
        }

        // The input (unless the screen is too short or narrow to show it inside the border).
        if let Some(input_area) = self.input_area(area, &config) {
            self.render_input_text(input_area, surface, cx);
        }

        // Below the popup: the completions, then the help for what is typed.
        let mut below = popup_area;
        if !self.prompt.completions().is_empty() {
            let list = self.render_completion_popup(popup_area, area, surface, cx);
            if list.height > 0 {
                below = list;
            }
        }
        self.render_doc(below, area, surface, cx);
    }

    /// Render the input text with horizontal scrolling support, highlighted when the prompt
    /// has a language (a regex, a shell command).
    fn render_input_text(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let theme = &cx.editor.theme;
        let line_width = Self::line_width(area);

        // Update scroll anchor based on cursor and available width
        self.prompt.update_scroll_anchor(line_width);

        if self.prompt.line().is_empty() {
            // The most recently entered value as a suggestion, as with the bottom command line.
            if let Some(suggestion) = self.prompt.first_history_completion(cx.editor) {
                surface.set_stringn(
                    area.x,
                    area.y,
                    &suggestion,
                    line_width,
                    theme.get("ui.text.inactive"),
                );
            }
            return;
        }

        let text_style = theme.get("ui.text");
        let line = self.prompt.line();
        let fresh = self
            .highlights
            .as_ref()
            .is_some_and(|(highlighted, theme_name, _)| {
                highlighted == line && theme_name == theme.name()
            });
        if !fresh {
            self.highlights = self
                .prompt
                .highlighted_line_styles(cx.editor)
                .map(|styles| (line.clone(), theme.name().to_string(), styles));
        }
        let styles = self.highlights.as_ref().map(|(_, _, styles)| styles);

        let anchor = self.prompt.anchor();
        surface.set_string_anchored(
            area.x,
            area.y,
            self.prompt.truncate_start(),
            self.prompt.truncate_end(),
            &line[anchor..],
            line_width,
            |offset| {
                styles
                    .and_then(|styles| styles.get(anchor + offset))
                    .map_or(text_style, |style| text_style.patch(*style))
            },
        );
    }

    /// The icon shown before the input (`show-icons`), or none.
    fn icon<'a>(&self, config: &'a helix_view::editor::Config) -> &'a str {
        if config.cmdline.show_icons {
            self.get_command_icon(&config.cmdline.icons)
        } else {
            ""
        }
    }

    fn icon_width(icon: &str, inner_area: Rect) -> u16 {
        (icon.width().min(u16::MAX as usize) as u16).min(inner_area.width)
    }

    /// Where the input goes in `viewport`: inside the popup's border, after the icon. `None`
    /// when the screen is too short or narrow for it. Asked for the cursor before the popup
    /// is drawn (by the compositor), so it does not depend on drawing.
    fn input_area(&self, viewport: Rect, config: &helix_view::editor::Config) -> Option<Rect> {
        let popup_area = self.calculate_popup_area(viewport, &config.cmdline);
        let inner_area = if config.gradient_borders.enable {
            // The gradient border is one cell wide whatever its thickness.
            GradientBorder::inner_area(popup_area)
        } else {
            Block::default()
                .borders(tui::widgets::Borders::ALL)
                .inner(popup_area)
        };
        let icon_width = Self::icon_width(self.icon(config), inner_area);
        let input_area = inner_area.clip_left(icon_width).with_height(1);
        (inner_area.height > 0 && input_area.width > 0).then_some(input_area)
    }

    /// The columns of the input `area` the text takes: the last one is kept for the cursor
    /// after the end of the line.
    fn line_width(area: Rect) -> usize {
        area.width.saturating_sub(1).max(1) as usize
    }

    /// The help for what is typed (`with_doc_fn`), in a box below `above` (the popup or its
    /// completions), as wide as the popup and as tall as the help needs and the screen
    /// allows.
    fn render_doc(&mut self, above: Rect, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let Some(doc) = (self.prompt.doc_fn)(self.prompt.line()) else {
            return;
        };
        let mut text = crate::ui::Text::new(doc.into_owned());
        let width = self.popup_area.width;
        // border and a column of margin on each side
        let text_width = width.saturating_sub(4);
        let y = above.bottom();
        let available = viewport.bottom().saturating_sub(y);
        if text_width == 0 || available < 3 {
            return;
        }
        let (_, text_height) = crate::ui::text::required_size(&text.contents, text_width);
        let area = Rect::new(
            self.popup_area.x,
            y,
            width,
            text_height.saturating_add(2).min(available),
        );

        let config = cx.editor.config();
        let help = cx.editor.theme.get("ui.help");
        crate::ui::clear_bordered(
            surface,
            area,
            help,
            !crate::ui::border_cells_take_background(&config),
        );
        let mut border_style = help;
        border_style.bg = None;
        let block = Block::bordered()
            .border_style(border_style)
            .border_type(crate::ui::border_type(&config));
        let inner = block
            .inner(area)
            .inner(helix_view::graphics::Margin::horizontal(1));
        block.render(area, surface);
        text.render(inner, surface, cx);
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

    /// Render the completion list below the popup (`base_area`); its area (empty when it did
    /// not fit).
    fn render_completion_popup(
        &mut self,
        base_area: Rect,
        viewport: Rect,
        surface: &mut Surface,
        cx: &Context,
    ) -> Rect {
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
            return Rect::default();
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

        let editor_config = cx.editor.config();

        // Use gradient border for completion if enabled
        let inner_area = if editor_config.gradient_borders.enable {
            let rounded = editor_config.border_rounded();
            if !editor_config.borders_hidden() {
                self.gradient_border(theme, &editor_config.gradient_borders)
                    .render(comp_area, surface, theme, rounded);
            }
            // The gradient border is one cell wide whatever its thickness.
            GradientBorder::inner_area(comp_area)
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
        comp_area
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
        // the plugins hear that the command line opened before its first key
        self.prompt.report_open();
        match self.style {
            CmdlineStyle::Popup => self.render_popup(area, surface, cx),
            CmdlineStyle::Bottom => self.render_bottom(area, surface, cx),
        }
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        match self.style {
            CmdlineStyle::Popup => {
                // In the input, after the scroll anchor: none when the screen has no room for
                // the input.
                let Some(input_area) = self.input_area(area, &editor.config()) else {
                    return (None, CursorKind::Hidden);
                };
                let offset = self
                    .prompt
                    .cursor_offset(Self::line_width(input_area))
                    .min(input_area.width.saturating_sub(1) as usize);
                (
                    Some(Position::new(
                        input_area.y as usize,
                        input_area.x as usize + offset,
                    )),
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
        let config = CmdlineConfig::default();
        let area = popup.calculate_popup_area(viewport, &config);

        // Width should be between min_width (40) and max_width (80)
        assert!(area.width >= config.min_popup_width);
        assert!(area.width <= config.max_popup_width);

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
        let area = popup.calculate_popup_area(viewport, &CmdlineConfig::default());

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
            let area = popup.calculate_popup_area(viewport, &CmdlineConfig::default());
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
        let area = popup.calculate_popup_area(Rect::new(0, 0, 80, 40), &CmdlineConfig::default());
        let list = CmdlinePopup::completion_area(area, Rect::new(0, 0, 80, 40), 500);
        assert_eq!(list.height, 12);
        let short = Rect::new(0, 0, 80, 12);
        let area = popup.calculate_popup_area(short, &CmdlineConfig::default());
        let list = CmdlinePopup::completion_area(area, short, 500);
        assert_eq!(list.bottom(), short.bottom());

        // a very long line doesn't overflow the width
        let viewport = Rect::new(0, 0, 100, 30);
        popup.prompt.set_line_unchecked("x".repeat(70_000));
        let area = popup.calculate_popup_area(viewport, &CmdlineConfig::default());
        assert_eq!(area.width, CmdlineConfig::default().max_popup_width);
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

    #[test]
    fn the_popup_width_follows_the_configuration() {
        let popup = |line: &str| {
            CmdlinePopup::new(
                ":".into(),
                None,
                crate::ui::completers::none,
                |_ctx, _input, _event| {},
                CmdlineStyle::Popup,
            )
            .with_line_unchecked(line)
        };
        let viewport = Rect::new(0, 0, 200, 50);
        let config = |min_popup_width: u16, max_popup_width: u16| CmdlineConfig {
            min_popup_width,
            max_popup_width,
            ..Default::default()
        };

        // `min-popup-width` for a short line, growing with the line up to `max-popup-width`
        let width: u16 = popup("w")
            .calculate_popup_area(viewport, &config(60, 120))
            .width;
        assert_eq!(width, 60);
        let width: u16 = popup(&"x".repeat(90))
            .calculate_popup_area(viewport, &config(60, 120))
            .width;
        assert_eq!(width, 90 + 2 * PADDING);
        let width: u16 = popup(&"x".repeat(500))
            .calculate_popup_area(viewport, &config(60, 120))
            .width;
        assert_eq!(width, 120);
        // a maximum below the minimum is the minimum, and the screen is the limit
        let width: u16 = popup("w")
            .calculate_popup_area(viewport, &config(50, 10))
            .width;
        assert_eq!(width, 50);
        let width: u16 = popup("w")
            .calculate_popup_area(Rect::new(0, 0, 30, 10), &config(60, 120))
            .width;
        assert_eq!(width, 26);
    }

    #[test]
    fn the_icon_is_the_kind_of_prompt_not_what_is_typed() {
        let icons = helix_view::editor::CmdlineIcons::default();
        let icon = |label: &'static str, register: Option<char>, line: &str| -> String {
            CmdlinePopup::new(
                label.into(),
                register,
                crate::ui::completers::none,
                |_ctx, _input, _event| {},
                CmdlineStyle::Popup,
            )
            .with_line_unchecked(line)
            .get_command_icon(&icons)
            .to_string()
        };
        assert_eq!(icon("Cmdline", Some(':'), "write"), icons.command);
        // typing `/` or `!` in the command line does not make it a search or a shell
        assert_eq!(icon("Cmdline", Some(':'), "/x"), icons.command);
        assert_eq!(icon("Cmdline", Some(':'), "!ls"), icons.command);
        assert_eq!(icon("Search", Some('/'), ""), icons.search);
        assert_eq!(icon("pipe:", Some('|'), "sort"), icons.shell);
        assert_eq!(icon("insert-output:", Some('|'), ""), icons.shell);
        assert_eq!(icon("rename-to:", None, "?x"), icons.general);
    }
}
