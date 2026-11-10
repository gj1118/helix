use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::gradient_border::GradientBorder;
use helix_core::unicode::segmentation::UnicodeSegmentation;
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::theme::Modifier;
use helix_view::{
    editor::{
        GradientBorderConfig, Notification, NotificationConfig, NotificationPosition,
        NotificationStyle, Severity,
    },
    graphics::{Color, Rect, Style},
    Editor,
};
use std::collections::HashSet;
use tokio::time::sleep as tokio_sleep;
use tui::buffer::Buffer as Surface;

/// The notifications of the editor in noice.nvim style: cards stacked in a corner (or at the
/// center) of the screen, newest last. The editor keeps the notifications; this only lays
/// them out and draws them, every frame.
#[derive(Default)]
pub struct NotificationPopup {
    /// The notifications whose dismiss timer is armed (see [`schedule_dismiss`]).
    timers: HashSet<usize>,
    /// The gradient border, with the configuration it was made from.
    gradient_border: Option<(GradientBorderConfig, GradientBorder)>,
}

/// A notification laid out: where its card goes and the lines shown in it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Card {
    id: usize,
    area: Rect,
    /// The width of the border (each side): 0 without one.
    border: u16,
    /// The padding inside the border (each side), when there is room for it.
    padding: u16,
    /// The icon (or emoji) and a space before the first line, when there is room for it.
    prefix: Option<String>,
    /// The message wrapped to the card; the last line ends in `…` when the message is longer
    /// than the card can show.
    lines: Vec<String>,
}

/// The sizes of a card's frame, from the configuration.
#[derive(Debug, Clone, Copy)]
struct Frame {
    border: u16,
    padding: u16,
}

impl Frame {
    fn new(config: &helix_view::editor::Config) -> Self {
        let notifications = &config.notifications;
        // The gradient border is always one cell wide (its thickness only picks glyphs); the
        // simple border draws `border.width` nested frames.
        let border = if !notifications.border.enable {
            0
        } else if BorderLook::new(config, notifications.border.radius)
            .gradient
            .is_some()
        {
            1
        } else {
            notifications.border.width.max(1) as u16
        };
        Self {
            border,
            padding: notifications.padding,
        }
    }
}

/// Smallest card drawn.
const MIN_WIDTH: u16 = 4;
const MIN_HEIGHT: u16 = 3;
/// Rows between stacked cards.
const SPACING: u16 = 1;

impl NotificationPopup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms a dismiss timer for each new notification with a timeout, and forgets those of
    /// the notifications gone.
    fn arm_timers(&mut self, active: &[&Notification]) {
        self.timers
            .retain(|id| active.iter().any(|notification| notification.id == *id));
        for notification in active {
            if let Some(timeout) = notification.timeout {
                if self.timers.insert(notification.id) {
                    schedule_dismiss(
                        notification.id,
                        remaining_time(notification.timestamp, timeout),
                    );
                }
            }
        }
    }

    /// The cards of `notifications` (oldest first) in `viewport`. When they do not all fit,
    /// the oldest are left out: the newest notification is always shown (if anything fits).
    fn layout(
        notifications: &[&Notification],
        viewport: Rect,
        config: &NotificationConfig,
        frame: Frame,
    ) -> Vec<Card> {
        let max_width = config.max_width.min(viewport.width.saturating_sub(4));
        // Rows usable below/above the 1-row margin from the viewport edge.
        let max_height = config
            .max_height
            .max(MIN_HEIGHT)
            .min(viewport.height.saturating_sub(1));
        if max_width < MIN_WIDTH || max_height < MIN_HEIGHT {
            // No room (tiny terminal, narrow area beside a pinned panel, or a tiny
            // `max-width`).
            return Vec::new();
        }

        // Newest first, while they fit.
        let mut cards = Vec::new();
        let mut used = 0u16;
        for notification in notifications.iter().rev() {
            let (width, height, card) =
                Self::card(notification, max_width, max_height, config, frame);
            let needed = used.saturating_add(height).saturating_add(SPACING);
            if needed > viewport.height {
                break;
            }
            used = needed;
            cards.push((width, height, card));
        }
        cards.reverse();

        // Stacked from the edge, oldest first: downwards for top positions, upwards for
        // bottom positions.
        let mut y_offset = 0u16;
        cards
            .into_iter()
            .map(|(width, height, mut card)| {
                let left = viewport.x + 2;
                let center = viewport.x + viewport.width.saturating_sub(width) / 2;
                let right = viewport.x + viewport.width.saturating_sub(width + 2);
                let top = viewport.y + y_offset + 1;
                let bottom = viewport.y + viewport.height.saturating_sub(height + y_offset + 1);
                let (x, y) = match config.position {
                    NotificationPosition::TopLeft => (left, top),
                    NotificationPosition::TopCenter => (center, top),
                    NotificationPosition::TopRight => (right, top),
                    NotificationPosition::BottomLeft => (left, bottom),
                    NotificationPosition::BottomCenter => (center, bottom),
                    NotificationPosition::BottomRight => (right, bottom),
                };
                card.area = Rect::new(x, y, width, height);
                y_offset = y_offset.saturating_add(height).saturating_add(SPACING);
                card
            })
            .collect()
    }

    /// The card of `notification` (its area still to be placed), and its size: as wide as
    /// its lines need, wrapped to `max_width` and cut to `max_height`.
    fn card(
        notification: &Notification,
        max_width: u16,
        max_height: u16,
        config: &NotificationConfig,
        frame: Frame,
    ) -> (u16, u16, Card) {
        let border = frame.border;
        // The padding only when it leaves room inside the border.
        let inside_width = max_width.saturating_sub(border * 2);
        let inside_height = max_height.saturating_sub(border * 2);
        let padding = if inside_width > frame.padding * 2 && inside_height > frame.padding * 2 {
            frame.padding
        } else {
            0
        };
        let text_width = inside_width.saturating_sub(padding * 2) as usize;
        let text_rows = inside_height.saturating_sub(padding * 2).max(1) as usize;

        // The icon before the first line, when the line keeps a column for the text.
        let prefix = severity_prefix(config, notification.severity)
            .map(|icon| format!("{icon} "))
            .filter(|prefix| prefix.width() < text_width);
        let prefix_width = prefix.as_deref().map_or(0, UnicodeWidthStr::width);

        let mut lines = wrap(
            &notification.message,
            text_width.saturating_sub(prefix_width).max(1),
            text_width.max(1),
        );
        if lines.len() > text_rows {
            lines.truncate(text_rows);
            let last_width = if text_rows == 1 {
                text_width.saturating_sub(prefix_width)
            } else {
                text_width
            };
            let last = lines.last_mut().unwrap();
            *last = with_ellipsis(last, last_width);
        }

        let content_width = lines
            .iter()
            .enumerate()
            .map(|(i, line)| line.width() + if i == 0 { prefix_width } else { 0 })
            .max()
            .unwrap_or(0)
            .max(1) as u16;
        let frame_size = (border + padding) * 2;
        let width = content_width
            .saturating_add(frame_size)
            .clamp(MIN_WIDTH, max_width);
        let height = (lines.len() as u16)
            .saturating_add(frame_size)
            .clamp(MIN_HEIGHT, max_height);
        let card = Card {
            id: notification.id,
            area: Rect::default(),
            border,
            padding,
            prefix,
            lines,
        };
        (width, height, card)
    }

    /// The style of the text of a notification of `severity`.
    fn severity_style(severity: Severity, theme: &helix_view::Theme) -> Style {
        match severity {
            Severity::Error => theme.get("error"),
            Severity::Warning => theme.get("warning"),
            Severity::Info => theme.get("info"),
            Severity::Hint => theme.get("hint"),
        }
    }

    // Render a simple (non-gradient) border without painting the background.
    fn render_simple_border(
        area: Rect,
        surface: &mut Surface,
        style: Style,
        symbols: &tui::symbols::line::Set,
        width: u16,
    ) {
        let (h, v, tl, tr, bl, br) = (
            symbols.horizontal,
            symbols.vertical,
            symbols.top_left,
            symbols.top_right,
            symbols.bottom_left,
            symbols.bottom_right,
        );

        for s in 0..width.max(1) {
            let x0 = area.x.saturating_add(s);
            let x1 = area.right().saturating_sub(1 + s);
            let y0 = area.y.saturating_add(s);
            let y1 = area.bottom().saturating_sub(1 + s);

            if x0 >= x1 || y0 >= y1 {
                break;
            }

            // Top and bottom lines
            for x in x0..=x1 {
                let ch_top = if x == x0 {
                    tl
                } else if x == x1 {
                    tr
                } else {
                    h
                };
                let ch_bot = if x == x0 {
                    bl
                } else if x == x1 {
                    br
                } else {
                    symbols.bottom
                };
                if let Some(cell) = surface.get_mut(x, y0) {
                    cell.set_symbol(ch_top).set_style(style);
                }
                if let Some(cell) = surface.get_mut(x, y1) {
                    cell.set_symbol(ch_bot).set_style(style);
                }
            }
            // Left and right lines
            for y in (y0 + 1)..y1 {
                if let Some(cell) = surface.get_mut(x0, y) {
                    cell.set_symbol(v).set_style(style);
                }
                if let Some(cell) = surface.get_mut(x1, y) {
                    cell.set_symbol(symbols.right).set_style(style);
                }
            }
        }
    }

    /// The drop shadow of `card`: the cells right of and below it (by the configured offset),
    /// inside `viewport`, darkened. What they show stays readable: a shadow, not a hole.
    fn render_shadow(
        card: &Card,
        viewport: Rect,
        surface: &mut Surface,
        config: &NotificationConfig,
    ) {
        let (dx, dy) = (config.shadow.offset_x, config.shadow.offset_y);
        let area = card.area;
        let strips = [
            // right of the card
            Rect::new(area.right(), area.y.saturating_add(dy), dx, area.height),
            // below it
            Rect::new(
                area.x.saturating_add(dx),
                area.bottom(),
                area.width.saturating_sub(dx),
                dy,
            ),
        ];
        let shadow = Style::default()
            .bg(Color::Rgb(0, 0, 0))
            .add_modifier(Modifier::DIM);
        for strip in strips {
            let strip = strip.intersection(viewport).intersection(surface.area);
            if strip.width > 0 && strip.height > 0 {
                surface.set_style(strip, shadow);
            }
        }
    }

    fn render_card(
        &mut self,
        card: &Card,
        notification: &Notification,
        surface: &mut Surface,
        editor: &Editor,
    ) {
        let editor_config = editor.config();
        let config = &editor_config.notifications;
        let theme = &editor.theme;
        if card.area.width < MIN_WIDTH || card.area.height < MIN_HEIGHT {
            return;
        }

        let text_style = Self::severity_style(notification.severity, theme);
        let background_style = theme.get("ui.popup");

        // Per-notification corner radius override
        let corner_radius = notification.corner_radius.unwrap_or(config.border.radius);
        let look = BorderLook::new(&editor_config, corner_radius);
        let border_type = look.border_type;
        // Whether the border cells take the card's background, as for every popup (see
        // `ui::border_cells_take_background`): without a border, without border glyphs,
        // and for borders along the outer edge of their cells. Otherwise they keep the
        // background behind the card. The card's background is painted before the border.
        let border_cells_take_background = !config.border.enable
            || if let Some(gradient_config) = look.gradient.as_ref() {
                border_type == tui::widgets::BorderType::Hidden
                    || GradientBorder::hugs_the_edge(gradient_config.thickness, look.rounded)
            } else {
                matches!(
                    border_type,
                    tui::widgets::BorderType::Hidden
                        | tui::widgets::BorderType::Edge
                        | tui::widgets::BorderType::QuadrantOutside
                )
            };
        crate::ui::clear_bordered(
            surface,
            card.area,
            background_style,
            !border_cells_take_background,
        );

        if config.border.enable {
            if let Some(wanted) = look.gradient {
                // The gradient border, made again when its configuration changes (with the
                // animation off).
                if self
                    .gradient_border
                    .as_ref()
                    .is_none_or(|(made_for, _)| *made_for != wanted)
                {
                    let mut border = GradientBorder::from_theme(theme, &wanted);
                    border.disable_animation();
                    self.gradient_border = Some((wanted, border));
                }
                if let Some((_, gradient_border)) = self.gradient_border.as_mut() {
                    if border_type != tui::widgets::BorderType::Hidden {
                        gradient_border.render(card.area, surface, theme, look.rounded);
                    }
                }
            } else {
                Self::render_simple_border(
                    card.area,
                    surface,
                    crate::ui::border_style(theme),
                    &look.symbols,
                    card.border,
                );
            }
        }

        // The lines, inside the border and the padding, as laid out.
        let inset = card.border + card.padding;
        let content = Rect::new(
            card.area.x + inset,
            card.area.y + inset,
            card.area.width.saturating_sub(inset * 2),
            card.area.height.saturating_sub(inset * 2),
        );
        if content.width == 0 || content.height == 0 {
            return;
        }
        for (i, line) in card.lines.iter().enumerate().take(content.height as usize) {
            let y = content.y + i as u16;
            let mut x = content.x;
            if let (0, Some(prefix)) = (i, &card.prefix) {
                let (end, _) =
                    surface.set_stringn(x, y, prefix, content.width as usize, text_style);
                x = end;
            }
            let available = content.right().saturating_sub(x) as usize;
            surface.set_stringn(x, y, line, available, text_style);
        }
    }
}

/// The icon (or emoji, which wins) shown before a notification of `severity`, if any.
fn severity_prefix(config: &NotificationConfig, severity: Severity) -> Option<&str> {
    // hints use the info icon
    let pick = |error, warning, info| match severity {
        Severity::Error => error,
        Severity::Warning => warning,
        Severity::Info | Severity::Hint => info,
    };
    let prefix: &str = if config.show_emojis {
        pick(
            &config.emojis.error,
            &config.emojis.warning,
            &config.emojis.info,
        )
    } else if config.show_icons {
        pick(
            &config.icons.error,
            &config.icons.warning,
            &config.icons.info,
        )
    } else {
        return None;
    };
    (!prefix.is_empty()).then_some(prefix)
}

/// `text` wrapped at word boundaries: the first line to `first_width` columns, the others to
/// `width`. A word longer than a line is broken (nothing is dropped); tabs are spaces, and
/// each line of `text` starts a new line.
fn wrap(text: &str, first_width: usize, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let line_width = |lines: &Vec<String>| if lines.is_empty() { first_width } else { width };
    for paragraph in text.replace('\t', "    ").lines() {
        let mut current = String::new();
        let mut current_width = 0;
        for word in paragraph.split_whitespace() {
            let word_width = word.width();
            let gap = usize::from(!current.is_empty());
            if current_width + gap + word_width <= line_width(&lines) {
                if gap == 1 {
                    current.push(' ');
                }
                current.push_str(word);
                current_width += gap + word_width;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            // the word alone, broken over lines when longer than one
            for grapheme in word.graphemes(true) {
                let grapheme_width = grapheme.width();
                if current_width + grapheme_width > line_width(&lines) && !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push_str(grapheme);
                current_width += grapheme_width;
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// `line` ending in `…`, within `width` columns: what was cut is more than the card shows.
fn with_ellipsis(line: &str, width: usize) -> String {
    let mut fitted = String::new();
    let mut fitted_width = 0;
    for grapheme in line.graphemes(true) {
        let grapheme_width = grapheme.width();
        if fitted_width + grapheme_width + 1 > width {
            break;
        }
        fitted.push_str(grapheme);
        fitted_width += grapheme_width;
    }
    fitted.push('…');
    fitted
}

impl Component for NotificationPopup {
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        // Notifications don't handle events directly
        EventResult::Ignored(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let editor = &*cx.editor;
        let editor_config = editor.config();
        let config = &editor_config.notifications;

        // Only shown when the notification style is Popup
        if !config.enable || config.style != NotificationStyle::Popup {
            self.timers.clear();
            return;
        }

        let active = editor.get_active_notifications();
        self.arm_timers(&active);
        if active.is_empty() {
            return;
        }

        let cards = Self::layout(&active, area, config, Frame::new(&editor_config));
        // The shadows first: a card stacked close to another covers its shadow.
        if config.shadow.enable {
            for card in &cards {
                Self::render_shadow(card, area, surface, config);
            }
        }
        for card in &cards {
            if let Some(notification) = active.iter().find(|n| n.id == card.id) {
                self.render_card(card, notification, surface, editor);
            }
        }
    }
}

/// How a notification's border is drawn, from `[editor.notifications.border]` and the
/// editor's `border-style` and `[editor.gradient-borders]`.
struct BorderLook {
    /// The kind of glyphs (for the background of the border cells; `Hidden` draws none).
    border_type: tui::widgets::BorderType,
    /// The glyphs of a border without a gradient.
    symbols: tui::symbols::line::Set,
    /// Whether the corners are rounded (what a gradient border follows).
    rounded: bool,
    /// The gradient, when the border has one (with the animation off).
    gradient: Option<helix_view::editor::GradientBorderConfig>,
}

impl BorderLook {
    fn new(editor_config: &helix_view::editor::Config, corner_radius: u8) -> Self {
        use helix_view::editor::{BorderStyle, NotificationBorderStyle};
        use tui::widgets::BorderType;

        let border = &editor_config.notifications.border;
        let (border_type, rounded) = match border.style {
            NotificationBorderStyle::Solid => {
                // a corner radius asks for rounded corners on an otherwise plain border
                let mut border_type = crate::ui::border_type(editor_config);
                if corner_radius > 0 && border_type == BorderType::Edge {
                    border_type = BorderType::Rounded;
                }
                (
                    border_type,
                    editor_config.border_rounded() || corner_radius > 0,
                )
            }
            NotificationBorderStyle::Dashed | NotificationBorderStyle::Dotted => {
                if corner_radius > 0 {
                    (BorderType::Rounded, true)
                } else {
                    (BorderType::Plain, false)
                }
            }
            style => {
                let border_type = match style.border_style() {
                    Some(BorderStyle::None) => BorderType::Hidden,
                    Some(BorderStyle::Plain) => BorderType::Edge,
                    Some(BorderStyle::Rounded) => BorderType::Rounded,
                    Some(BorderStyle::Double) => BorderType::Double,
                    Some(BorderStyle::Thick) => BorderType::Thick,
                    Some(BorderStyle::QuadrantInside) => BorderType::QuadrantInside,
                    Some(BorderStyle::QuadrantOutside) => BorderType::QuadrantOutside,
                    None => BorderType::Plain,
                };
                (border_type, border_type == BorderType::Rounded)
            }
        };

        let mut symbols = BorderType::line_symbols(border_type);
        let dashes = match border.style {
            NotificationBorderStyle::Dashed => Some(("┄", "┆")),
            NotificationBorderStyle::Dotted => Some(("┈", "┊")),
            _ => None,
        };
        if let Some((horizontal, vertical)) = dashes {
            symbols.horizontal = horizontal;
            symbols.bottom = horizontal;
            symbols.vertical = vertical;
            symbols.right = vertical;
        }

        // `border.gradient` draws a gradient between its own colors, with the thickness and
        // direction of `[editor.gradient-borders]`; without it, notifications follow
        // `[editor.gradient-borders]` like every popup.
        let gradient = if border.gradient {
            Some(helix_view::editor::GradientBorderConfig {
                enable: true,
                start_color: border.gradient_start.clone(),
                end_color: border.gradient_end.clone(),
                middle_color: String::new(),
                animation_speed: 0,
                ..editor_config.gradient_borders.clone()
            })
        } else if editor_config.gradient_borders.enable {
            Some(helix_view::editor::GradientBorderConfig {
                animation_speed: 0,
                ..editor_config.gradient_borders.clone()
            })
        } else {
            None
        };

        Self {
            border_type,
            symbols,
            rounded,
            gradient,
        }
    }
}

/// What is left of `timeout` since `started`.
fn remaining_time(
    started: tokio::time::Instant,
    timeout: std::time::Duration,
) -> std::time::Duration {
    timeout.saturating_sub(started.elapsed())
}

/// Dismisses notification `id` once its timeout has passed. A repeated message
/// refreshes the notification's timestamp, so when the timer fires early the
/// notification is still active and the timer is armed again for what is left;
/// otherwise the popup would stay until some unrelated redraw.
fn schedule_dismiss(id: usize, after: std::time::Duration) {
    tokio::spawn(async move {
        tokio_sleep(after).await;
        // Post back to the UI thread to mutate Editor safely
        crate::job::dispatch(move |editor: &mut helix_view::Editor, _compositor| {
            editor.dismiss_notification_if_expired(id);
            let still_active = editor
                .get_active_notifications()
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| {
                    n.timeout
                        .map(|timeout| remaining_time(n.timestamp, timeout))
                });
            if let Some(remaining) = still_active {
                schedule_dismiss(id, remaining);
            }
            helix_event::request_redraw();
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_config(toml: &str) -> helix_view::editor::Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn border_style_picks_the_notification_glyphs() {
        use tui::widgets::BorderType;

        // "solid" (the default) follows `border-style`, a radius rounding a plain border
        let look = BorderLook::new(&editor_config(""), 2);
        assert_eq!(look.border_type, BorderType::Rounded);
        assert!(look.rounded);
        let look = BorderLook::new(&editor_config(""), 0);
        assert_eq!(look.border_type, BorderType::Edge);
        assert!(!look.rounded);
        let look = BorderLook::new(&editor_config("border-style = \"double\""), 2);
        assert_eq!(look.border_type, BorderType::Double);

        // a `border-style` value for the notifications only, whatever the radius
        for (name, border_type) in [
            ("none", BorderType::Hidden),
            ("plain", BorderType::Edge),
            ("rounded", BorderType::Rounded),
            ("double", BorderType::Double),
            ("thick", BorderType::Thick),
            ("quadrant-inside", BorderType::QuadrantInside),
            ("quadrant-outside", BorderType::QuadrantOutside),
        ] {
            let config = editor_config(&format!(
                "border-style = \"thick\"\nnotifications.border.style = \"{name}\""
            ));
            let look = BorderLook::new(&config, 2);
            assert_eq!(look.border_type, border_type, "{name}");
            assert_eq!(look.rounded, name == "rounded", "{name}");
            assert_eq!(
                look.symbols.horizontal,
                BorderType::line_symbols(border_type).horizontal
            );
        }

        // dashed and dotted lines, with the corners the radius asks for
        let dashed = editor_config("notifications.border.style = \"dashed\"");
        let look = BorderLook::new(&dashed, 0);
        assert_eq!((look.symbols.horizontal, look.symbols.vertical), ("┄", "┆"));
        assert_eq!((look.symbols.bottom, look.symbols.right), ("┄", "┆"));
        assert_eq!(look.symbols.top_left, "┌");
        assert_eq!(BorderLook::new(&dashed, 1).symbols.top_left, "╭");
        let dotted = editor_config("notifications.border.style = \"dotted\"");
        let look = BorderLook::new(&dotted, 0);
        assert_eq!((look.symbols.horizontal, look.symbols.vertical), ("┈", "┊"));
    }

    #[test]
    fn border_gradient_uses_the_notification_colors() {
        // no gradient by default
        assert!(BorderLook::new(&editor_config(""), 2).gradient.is_none());

        // `border.gradient` draws one between its colors, with the editor's thickness
        let config = editor_config(
            r##"
            gradient-borders = { thickness = 3, start-color = "#000000", end-color = "#000000", middle-color = "#ffffff", animation-speed = 5 }
            notifications.border = { gradient = true, gradient-start = "#ff0000", gradient-end = "#0000ff" }
            "##,
        );
        let gradient = BorderLook::new(&config, 2).gradient.unwrap();
        assert!(gradient.enable);
        assert_eq!(gradient.start_color, "#ff0000");
        assert_eq!(gradient.end_color, "#0000ff");
        assert_eq!(gradient.middle_color, "");
        assert_eq!(gradient.thickness, 3);
        assert_eq!(gradient.animation_speed, 0);

        // without it, the editor's gradient borders are used, as for every popup
        let config = editor_config(
            r##"gradient-borders = { enable = true, start-color = "#010203", animation-speed = 5 }"##,
        );
        let gradient = BorderLook::new(&config, 2).gradient.unwrap();
        assert_eq!(gradient.start_color, "#010203");
        assert_eq!(gradient.animation_speed, 0);
    }

    fn notification(id: usize, message: &str, severity: Severity) -> Notification {
        let mut notification = Notification::new(message.to_string(), severity);
        notification.id = id;
        notification
    }

    fn frame(border: u16, padding: u16) -> Frame {
        Frame { border, padding }
    }

    #[test]
    fn wrapping_keeps_every_word_and_room_for_the_icon() {
        let cases: [(&str, usize, usize, &[&str]); 7] = [
            ("Hello", 20, 20, &["Hello"]),
            ("Hello", 5, 5, &["Hello"]),
            ("Hello World", 6, 6, &["Hello", "World"]),
            // the first line leaves room for the icon
            ("one two three four", 7, 10, &["one two", "three four"]),
            // a word longer than a line is broken, not cut short
            ("superlongword", 5, 5, &["super", "longw", "ord"]),
            ("", 20, 20, &[""]),
            // lines (an empty one too) stay lines; tabs are spaces
            ("a\n\nb\tc", 20, 20, &["a", "", "b c"]),
        ];
        for (text, first, rest, expected) in cases {
            let lines: Vec<String> = wrap(text, first, rest);
            assert_eq!(lines, expected, "{text:?}");
            for (i, line) in lines.iter().enumerate() {
                let limit = if i == 0 { first } else { rest };
                let width: usize = line.width();
                assert!(width <= limit, "{text:?}: {line:?}");
            }
        }
        // nothing is lost
        let text = "a fairly long notification message with averyveryverylongword in it";
        let lines = wrap(text, 8, 12);
        let joined: String = lines.concat().replace(' ', "");
        assert_eq!(joined, text.replace(' ', ""));
    }

    #[test]
    fn a_long_message_shows_its_first_line_whole_and_ends_in_an_ellipsis() {
        let config = NotificationConfig {
            max_width: 30,
            max_height: 6,
            ..Default::default()
        };
        let message = "the quick brown fox jumps over the lazy dog again and again and again";
        let n = notification(1, message, Severity::Error);
        let (width, height, card) = NotificationPopup::card(&n, 30, 6, &config, frame(1, 1));
        assert_eq!(card.prefix.as_deref(), Some("❌ "));
        // as wide as its widest line (and the frame), as tall as `max-height`
        let widest = (card.lines[0].width() + 3).max(card.lines[1].width());
        assert_eq!((width, height), (widest as u16 + 4, 6));
        // 30 - 2 * (border + padding) = 26 columns; the first line after the icon (3)
        assert_eq!(card.lines.len(), 2);
        assert_eq!(card.lines[0], "the quick brown fox");
        assert!(card.lines[0].width() + 3 <= 26);
        assert!(card.lines[1].ends_with('…'), "{:?}", card.lines);
        assert!(card.lines[1].width() <= 26);

        // a short one: as wide as it needs
        let n = notification(2, "saved", Severity::Info);
        let (width, height, card) = NotificationPopup::card(&n, 30, 6, &config, frame(1, 1));
        assert_eq!(card.lines, ["saved"]);
        assert_eq!(width, ("💡 saved".width() + 4) as u16);
        assert_eq!(height, 5);
        // without icons and emojis, none
        let plain = NotificationConfig {
            show_emojis: false,
            show_icons: false,
            ..config.clone()
        };
        let (_, _, card) = NotificationPopup::card(&n, 30, 6, &plain, frame(1, 1));
        assert_eq!(card.prefix, None);
    }

    #[test]
    fn the_newest_notifications_are_shown_when_not_all_fit() {
        let config = NotificationConfig::default();
        let notifications: Vec<Notification> = (1..=6)
            .map(|id| notification(id, &format!("message {id}"), Severity::Info))
            .collect();
        let refs: Vec<&Notification> = notifications.iter().collect();
        // cards of 5 rows and a row between them: 3 fit in 20 rows
        let cards = NotificationPopup::layout(&refs, Rect::new(0, 0, 80, 20), &config, frame(1, 1));
        let ids: Vec<usize> = cards.iter().map(|card| card.id).collect();
        assert_eq!(ids, [4, 5, 6]);
        // stacked downwards from the top right, oldest first, without overlapping
        let ys: Vec<u16> = cards.iter().map(|card| card.area.y).collect();
        assert_eq!(ys, [1, 7, 13]);
        assert!(cards.iter().all(|card| card.area.right() == 78));
    }

    #[test]
    fn test_layout_never_panics_and_stays_in_viewport() {
        let positions = [
            NotificationPosition::TopLeft,
            NotificationPosition::TopCenter,
            NotificationPosition::TopRight,
            NotificationPosition::BottomLeft,
            NotificationPosition::BottomCenter,
            NotificationPosition::BottomRight,
        ];
        let notifications: Vec<Notification> = (0..3)
            .map(|id| notification(id, "a fairly long notification message", Severity::Info))
            .collect();
        let refs: Vec<&Notification> = notifications.iter().collect();
        for max_width in [0, 1, 2, 3, 4, 60] {
            for position in positions {
                let config = NotificationConfig {
                    max_width,
                    position,
                    ..Default::default()
                };
                for (border, padding) in [(0, 0), (1, 1), (3, 2)] {
                    for width in 0..12 {
                        for height in 0..8 {
                            let viewport = Rect::new(3, 2, width, height);
                            let cards = NotificationPopup::layout(
                                &refs,
                                viewport,
                                &config,
                                frame(border, padding),
                            );
                            for card in cards {
                                assert_eq!(viewport.intersection(card.area), card.area);
                                assert!(card.area.width <= max_width);
                                let inside = card
                                    .area
                                    .height
                                    .saturating_sub((card.border + card.padding) * 2);
                                assert!(card.lines.len() <= (inside as usize).max(1));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_shadow_darkens_beside_the_card_and_keeps_what_is_there() {
        let mut surface = Surface::filled(Rect::new(0, 0, 20, 10), &{
            let mut cell = tui::buffer::Cell::default();
            cell.set_symbol("x");
            cell
        });
        let card = Card {
            id: 1,
            area: Rect::new(2, 2, 6, 4),
            border: 1,
            padding: 0,
            prefix: None,
            lines: Vec::new(),
        };
        let config = NotificationConfig::default();
        NotificationPopup::render_shadow(&card, Rect::new(0, 0, 20, 10), &mut surface, &config);
        let shadowed = |x: u16, y: u16| surface[(x, y)].bg == Color::Rgb(0, 0, 0);
        // right of the card (from a row down) and below it (from a column in)
        assert!(shadowed(8, 3) && shadowed(8, 6) && shadowed(3, 6));
        assert!(!shadowed(8, 2) && !shadowed(2, 6));
        // never on the card itself, and the text under the shadow stays
        assert!(!shadowed(7, 5) && !shadowed(2, 2));
        assert_eq!(surface[(8, 3)].symbol.as_str(), "x");
        // nothing outside the viewport
        let mut surface = Surface::empty(Rect::new(0, 0, 20, 10));
        NotificationPopup::render_shadow(&card, Rect::new(0, 0, 8, 6), &mut surface, &config);
        assert!(!surface
            .content
            .iter()
            .any(|cell| cell.bg == Color::Rgb(0, 0, 0)));
    }

    #[test]
    fn severity_prefix_picks_emojis_then_icons() {
        let mut config = NotificationConfig::default();
        let prefix: Option<&str> = severity_prefix(&config, Severity::Error);
        assert_eq!(prefix, Some(config.emojis.error.as_str()));
        config.show_emojis = false;
        assert_eq!(
            severity_prefix(&config, Severity::Hint),
            Some(config.icons.info.as_str())
        );
        config.show_icons = false;
        assert_eq!(severity_prefix(&config, Severity::Warning), None);
    }

    #[test]
    fn test_notification_is_expired_no_timeout() {
        let notification = Notification::new("Test", Severity::Info);
        // No timeout means never expires
        assert!(!notification.is_expired());
    }
}
