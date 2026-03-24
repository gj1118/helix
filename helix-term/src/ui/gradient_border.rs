use helix_view::{
    editor::{GradientBorderConfig, GradientDirection},
    graphics::{Color, Rect, Style},
    theme::{Modifier, Theme},
};
use tui::buffer::Buffer as Surface;

type Rgb = (u8, u8, u8);

/// A utility for rendering gradient borders around UI components
pub struct GradientBorder {
    config: GradientBorderConfig,
    animation_frame: u32,
    // Cached parsed colors to avoid repeated hex parsing
    start_rgb: Rgb,
    end_rgb: Rgb,
    middle_rgb: Option<Rgb>,
}

impl GradientBorder {
    pub fn new(config: GradientBorderConfig) -> Self {
        let (start_rgb, end_rgb, middle_rgb) = Self::compute_cached_colors(&config);
        Self {
            config,
            animation_frame: 0,
            start_rgb,
            end_rgb,
            middle_rgb,
        }
    }

    /// Update animation frame for animated gradients
    pub fn tick(&mut self) {
        if self.config.animation_speed > 0 {
            self.animation_frame = self.animation_frame.wrapping_add(1);
        }
    }

    /// Disable gradient animation (set speed to 0)
    pub fn disable_animation(&mut self) {
        self.config.animation_speed = 0;
    }

    /// Parse hex color string to RGB
    fn parse_hex_color(hex: &str) -> Option<Rgb> {
        if hex.len() != 7 || !hex.starts_with('#') {
            return None;
        }

        let r = u8::from_str_radix(&hex[1..3], 16).ok()?;
        let g = u8::from_str_radix(&hex[3..5], 16).ok()?;
        let b = u8::from_str_radix(&hex[5..7], 16).ok()?;

        Some((r, g, b))
    }

    /// Compute cached RGB values from config (with sensible fallbacks)
    fn compute_cached_colors(config: &GradientBorderConfig) -> (Rgb, Rgb, Option<Rgb>) {
        let start_rgb = Self::parse_hex_color(&config.start_color).unwrap_or((138, 43, 226));
        let end_rgb = Self::parse_hex_color(&config.end_color).unwrap_or((0, 191, 255));
        let middle_rgb = if config.middle_color.is_empty() {
            None
        } else {
            Self::parse_hex_color(&config.middle_color)
        };
        (start_rgb, end_rgb, middle_rgb)
    }

    /// Interpolate between two colors
    fn interpolate_color(start: Rgb, end: Rgb, ratio: f32) -> Color {
        let ratio = ratio.clamp(0.0, 1.0);
        let r = (start.0 as f32 + (end.0 as f32 - start.0 as f32) * ratio) as u8;
        let g = (start.1 as f32 + (end.1 as f32 - start.1 as f32) * ratio) as u8;
        let b = (start.2 as f32 + (end.2 as f32 - start.2 as f32) * ratio) as u8;
        Color::Rgb(r, g, b)
    }

    /// Interpolate between three colors for middle color support
    fn interpolate_three_colors(start: Rgb, middle: Rgb, end: Rgb, ratio: f32) -> Color {
        let ratio = ratio.clamp(0.0, 1.0);
        if ratio < 0.5 {
            Self::interpolate_color(start, middle, ratio * 2.0)
        } else {
            Self::interpolate_color(middle, end, (ratio - 0.5) * 2.0)
        }
    }

    /// Calculate gradient color at a specific position
    fn get_gradient_color(&self, x: u16, y: u16, area: Rect) -> Color {
        let start_color = self.start_rgb;
        let end_color = self.end_rgb;

        // Apply animation offset if enabled
        let animation_offset = if self.config.animation_speed > 0 {
            (self.animation_frame as f32 * self.config.animation_speed as f32 * 0.01) % 1.0
        } else {
            0.0
        };

        let ratio = match self.config.direction {
            GradientDirection::Horizontal => {
                let base_ratio = (x - area.x) as f32 / area.width.max(1) as f32;
                (base_ratio + animation_offset) % 1.0
            }
            GradientDirection::Vertical => {
                let base_ratio = (y - area.y) as f32 / area.height.max(1) as f32;
                (base_ratio + animation_offset) % 1.0
            }
            GradientDirection::Diagonal => {
                let base_ratio =
                    ((x - area.x) + (y - area.y)) as f32 / (area.width + area.height).max(1) as f32;
                (base_ratio + animation_offset) % 1.0
            }
            GradientDirection::Radial => {
                let center_x = area.x + area.width / 2;
                let center_y = area.y + area.height / 2;
                let distance = ((x as f32 - center_x as f32).powi(2)
                    + (y as f32 - center_y as f32).powi(2))
                .sqrt();
                let max_distance = (area.width.max(area.height) / 2) as f32;
                let base_ratio = (distance / max_distance.max(1.0)).min(1.0);
                (base_ratio + animation_offset) % 1.0
            }
        };

        // Check if we have a middle color for 3-color gradients
        if let Some(middle_color) = self.middle_rgb {
            return Self::interpolate_three_colors(start_color, middle_color, end_color, ratio);
        }

        Self::interpolate_color(start_color, end_color, ratio)
    }

    /// Get the appropriate border characters based on thickness and rounded corners setting
    fn get_border_chars(thickness: u8, rounded: bool) -> Vec<&'static str> {
        match (thickness, rounded) {
            // Thickness 1 - thin borders
            (1, false) => vec!["─", "│", "┌", "┐", "└", "┘"], // thin square
            (1, true) => vec!["─", "│", "╭", "╮", "╰", "╯"],  // thin rounded

            // Thickness 2 - thick borders
            (2, false) => vec!["━", "┃", "┏", "┓", "┗", "┛"], // thick square
            (2, true) => vec!["━", "┃", "┏", "┓", "┗", "┛"],  // thick (no rounded equivalent)

            // Thickness 3 - double borders
            (3, false) => vec!["═", "║", "╔", "╗", "╚", "╝"], // double square
            (3, true) => vec!["═", "║", "╔", "╗", "╚", "╝"],  // double (no rounded equivalent)

            // Thickness 4 - block characters
            (4, _) => vec!["▄", "█", "█", "█", "█", "█"], // block (rounded doesn't apply)

            // Thickness 5 - full block characters
            (5, _) => vec!["▀", "█", "█", "█", "█", "█"], // full block (rounded doesn't apply)

            // Fallback to thin
            _ => vec!["─", "│", "┌", "┐", "└", "┘"],
        }
    }

    /// Render the gradient border around the given area
    pub fn render(&mut self, area: Rect, surface: &mut Surface, _theme: &Theme, rounded: bool) {
        if !self.config.enable || area.width < 2 || area.height < 2 {
            return;
        }

        let border_chars = Self::get_border_chars(self.config.thickness, rounded);
        let [horizontal, vertical, top_left, top_right, bottom_left, bottom_right] = [
            border_chars[0],
            border_chars[1],
            border_chars[2],
            border_chars[3],
            border_chars[4],
            border_chars[5],
        ];

        // Render top border
        for x in area.left()..area.right() {
            let color = self.get_gradient_color(x, area.top(), area);
            let style = Style::default().fg(color);
            let symbol = if x == area.left() {
                top_left
            } else if x == area.right() - 1 {
                top_right
            } else {
                horizontal
            };

            if let Some(cell) = surface.get_mut(x, area.top()) {
                cell.set_symbol(symbol).set_style(style);
            }
        }

        // Render bottom border
        let bottom_y = area.bottom() - 1;
        for x in area.left()..area.right() {
            let color = self.get_gradient_color(x, bottom_y, area);
            let style = Style::default().fg(color);
            let symbol = if x == area.left() {
                bottom_left
            } else if x == area.right() - 1 {
                bottom_right
            } else {
                horizontal
            };

            if let Some(cell) = surface.get_mut(x, bottom_y) {
                cell.set_symbol(symbol).set_style(style);
            }
        }

        // Render left and right borders (skip corners)
        for y in (area.top() + 1)..(area.bottom() - 1) {
            // Left border
            let color = self.get_gradient_color(area.left(), y, area);
            let style = Style::default().fg(color);
            if let Some(cell) = surface.get_mut(area.left(), y) {
                cell.set_symbol(vertical).set_style(style);
            }

            // Right border
            let right_x = area.right() - 1;
            let color = self.get_gradient_color(right_x, y, area);
            let style = Style::default().fg(color);
            if let Some(cell) = surface.get_mut(right_x, y) {
                cell.set_symbol(vertical).set_style(style);
            }
        }

        // Update animation frame
        self.tick();
    }

    /// Render gradient border with title (for pickers with titles)
    pub fn render_with_title(
        &mut self,
        area: Rect,
        surface: &mut Surface,
        theme: &Theme,
        title: Option<&str>,
        rounded: bool,
    ) {
        // If there's a title, we need to render the border but leave space for the title
        // First render the border normally
        self.render(area, surface, theme, rounded);

        // If there's a title, render it in the top border with padding
        if let Some(title) = title {
            if !title.is_empty() && area.width > title.len() as u16 + 6 {
                // Format title with spaces for padding: " Title "
                let padded_title = format!(" {} ", title);

                // Position title near the left side (after corner + some space)
                let title_start = area.x + 2;

                // Render each character of the padded title
                for (i, ch) in padded_title.chars().enumerate() {
                    let x = title_start + i as u16;
                    if x < area.right() - 1 {
                        let title_color = self.get_gradient_color(x, area.y, area);
                        let title_style = Style::default()
                            .fg(title_color)
                            .add_modifier(Modifier::BOLD);

                        if let Some(cell) = surface.get_mut(x, area.top()) {
                            // Reset the cell first to clear any previous content
                            cell.reset();
                            // Use a string slice to properly set the character
                            let ch_str: String = ch.to_string();
                            cell.set_symbol(&ch_str).set_style(title_style);
                        }
                    }
                }
            }
        }
    }

    /// Create a gradient border with default theme-based colors
    pub fn from_theme(_theme: &Theme, config: &GradientBorderConfig) -> Self {
        let mut border_config = config.clone();

        // Use theme colors as fallbacks if hex colors are invalid
        if Self::parse_hex_color(&border_config.start_color).is_none() {
            border_config.start_color = "#8A2BE2".to_string();
        }
        if Self::parse_hex_color(&border_config.end_color).is_none() {
            border_config.end_color = "#00BFFF".to_string();
        }

        Self::new(border_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> GradientBorderConfig {
        GradientBorderConfig {
            enable: true,
            thickness: 1,
            direction: GradientDirection::Horizontal,
            start_color: "#FF0000".to_string(),
            end_color: "#0000FF".to_string(),
            middle_color: "".to_string(),
            animation_speed: 0,
        }
    }

    // ===========================================
    // Hex Color Parsing Tests
    // ===========================================

    #[test]
    fn test_parse_hex_color_valid() {
        assert_eq!(
            GradientBorder::parse_hex_color("#FF0000"),
            Some((255, 0, 0))
        );
        assert_eq!(
            GradientBorder::parse_hex_color("#00FF00"),
            Some((0, 255, 0))
        );
        assert_eq!(
            GradientBorder::parse_hex_color("#0000FF"),
            Some((0, 0, 255))
        );
        assert_eq!(
            GradientBorder::parse_hex_color("#FFFFFF"),
            Some((255, 255, 255))
        );
        assert_eq!(GradientBorder::parse_hex_color("#000000"), Some((0, 0, 0)));
        assert_eq!(
            GradientBorder::parse_hex_color("#8A2BE2"),
            Some((138, 43, 226))
        );
    }

    #[test]
    fn test_parse_hex_color_lowercase() {
        assert_eq!(
            GradientBorder::parse_hex_color("#ff0000"),
            Some((255, 0, 0))
        );
        assert_eq!(
            GradientBorder::parse_hex_color("#abcdef"),
            Some((171, 205, 239))
        );
    }

    #[test]
    fn test_parse_hex_color_mixed_case() {
        assert_eq!(
            GradientBorder::parse_hex_color("#FfAaBb"),
            Some((255, 170, 187))
        );
    }

    #[test]
    fn test_parse_hex_color_invalid_no_hash() {
        assert_eq!(GradientBorder::parse_hex_color("FF0000"), None);
    }

    #[test]
    fn test_parse_hex_color_invalid_wrong_length() {
        assert_eq!(GradientBorder::parse_hex_color("#FF00"), None);
        assert_eq!(GradientBorder::parse_hex_color("#FF00000"), None);
        assert_eq!(GradientBorder::parse_hex_color("#F"), None);
        assert_eq!(GradientBorder::parse_hex_color("#"), None);
    }

    #[test]
    fn test_parse_hex_color_invalid_characters() {
        assert_eq!(GradientBorder::parse_hex_color("#GGGGGG"), None);
        assert_eq!(GradientBorder::parse_hex_color("#12345G"), None);
        assert_eq!(GradientBorder::parse_hex_color("#-12345"), None);
    }

    #[test]
    fn test_parse_hex_color_empty_string() {
        assert_eq!(GradientBorder::parse_hex_color(""), None);
    }

    // ===========================================
    // Color Interpolation Tests
    // ===========================================

    #[test]
    fn test_interpolate_color_start() {
        let color = GradientBorder::interpolate_color((255, 0, 0), (0, 0, 255), 0.0);
        assert_eq!(color, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn test_interpolate_color_end() {
        let color = GradientBorder::interpolate_color((255, 0, 0), (0, 0, 255), 1.0);
        assert_eq!(color, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn test_interpolate_color_middle() {
        let color = GradientBorder::interpolate_color((255, 0, 0), (0, 0, 255), 0.5);
        // Should be roughly (127, 0, 127) - purple
        assert_eq!(color, Color::Rgb(127, 0, 127));
    }

    #[test]
    fn test_interpolate_color_quarter() {
        let color = GradientBorder::interpolate_color((0, 0, 0), (100, 100, 100), 0.25);
        assert_eq!(color, Color::Rgb(25, 25, 25));
    }

    #[test]
    fn test_interpolate_color_clamps_below_zero() {
        let color = GradientBorder::interpolate_color((255, 0, 0), (0, 0, 255), -0.5);
        // Should clamp to 0.0, giving start color
        assert_eq!(color, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn test_interpolate_color_clamps_above_one() {
        let color = GradientBorder::interpolate_color((255, 0, 0), (0, 0, 255), 1.5);
        // Should clamp to 1.0, giving end color
        assert_eq!(color, Color::Rgb(0, 0, 255));
    }

    // ===========================================
    // Three-Color Interpolation Tests
    // ===========================================

    #[test]
    fn test_interpolate_three_colors_start() {
        let color = GradientBorder::interpolate_three_colors(
            (255, 0, 0), // red
            (0, 255, 0), // green
            (0, 0, 255), // blue
            0.0,
        );
        assert_eq!(color, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn test_interpolate_three_colors_middle() {
        let color = GradientBorder::interpolate_three_colors(
            (255, 0, 0), // red
            (0, 255, 0), // green
            (0, 0, 255), // blue
            0.5,
        );
        // At 0.5, should be exactly middle color
        assert_eq!(color, Color::Rgb(0, 255, 0));
    }

    #[test]
    fn test_interpolate_three_colors_end() {
        let color = GradientBorder::interpolate_three_colors(
            (255, 0, 0), // red
            (0, 255, 0), // green
            (0, 0, 255), // blue
            1.0,
        );
        assert_eq!(color, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn test_interpolate_three_colors_first_quarter() {
        let color = GradientBorder::interpolate_three_colors(
            (255, 0, 0), // red
            (0, 255, 0), // green
            (0, 0, 255), // blue
            0.25,
        );
        // At 0.25, halfway between red and green
        assert_eq!(color, Color::Rgb(127, 127, 0));
    }

    #[test]
    fn test_interpolate_three_colors_third_quarter() {
        let color = GradientBorder::interpolate_three_colors(
            (255, 0, 0), // red
            (0, 255, 0), // green
            (0, 0, 255), // blue
            0.75,
        );
        // At 0.75, halfway between green and blue
        assert_eq!(color, Color::Rgb(0, 127, 127));
    }

    // ===========================================
    // Border Characters Tests
    // ===========================================

    #[test]
    fn test_border_chars_thickness_1_square() {
        let chars = GradientBorder::get_border_chars(1, false);
        assert_eq!(chars, vec!["─", "│", "┌", "┐", "└", "┘"]);
    }

    #[test]
    fn test_border_chars_thickness_1_rounded() {
        let chars = GradientBorder::get_border_chars(1, true);
        assert_eq!(chars, vec!["─", "│", "╭", "╮", "╰", "╯"]);
    }

    #[test]
    fn test_border_chars_thickness_2() {
        let chars = GradientBorder::get_border_chars(2, false);
        assert_eq!(chars, vec!["━", "┃", "┏", "┓", "┗", "┛"]);
    }

    #[test]
    fn test_border_chars_thickness_3_double() {
        let chars = GradientBorder::get_border_chars(3, false);
        assert_eq!(chars, vec!["═", "║", "╔", "╗", "╚", "╝"]);
    }

    #[test]
    fn test_border_chars_thickness_4_block() {
        let chars = GradientBorder::get_border_chars(4, false);
        assert_eq!(chars, vec!["▄", "█", "█", "█", "█", "█"]);
    }

    #[test]
    fn test_border_chars_thickness_5_full_block() {
        let chars = GradientBorder::get_border_chars(5, false);
        assert_eq!(chars, vec!["▀", "█", "█", "█", "█", "█"]);
    }

    #[test]
    fn test_border_chars_invalid_thickness_fallback() {
        // Thickness 0 or > 5 should fallback to thin
        let chars = GradientBorder::get_border_chars(0, false);
        assert_eq!(chars, vec!["─", "│", "┌", "┐", "└", "┘"]);

        let chars = GradientBorder::get_border_chars(10, false);
        assert_eq!(chars, vec!["─", "│", "┌", "┐", "└", "┘"]);
    }

    // ===========================================
    // GradientBorder Construction Tests
    // ===========================================

    #[test]
    fn test_gradient_border_new() {
        let config = default_config();
        let border = GradientBorder::new(config.clone());

        assert_eq!(border.animation_frame, 0);
        assert_eq!(border.start_rgb, (255, 0, 0));
        assert_eq!(border.end_rgb, (0, 0, 255));
        assert!(border.middle_rgb.is_none());
    }

    #[test]
    fn test_gradient_border_with_middle_color() {
        let mut config = default_config();
        config.middle_color = "#00FF00".to_string();
        let border = GradientBorder::new(config);

        assert_eq!(border.middle_rgb, Some((0, 255, 0)));
    }

    #[test]
    fn test_gradient_border_with_invalid_colors_uses_fallback() {
        let config = GradientBorderConfig {
            enable: true,
            thickness: 1,
            direction: GradientDirection::Horizontal,
            start_color: "invalid".to_string(),
            end_color: "also_invalid".to_string(),
            middle_color: "".to_string(),
            animation_speed: 0,
        };
        let border = GradientBorder::new(config);

        // Should use fallback colors (138, 43, 226) and (0, 191, 255)
        assert_eq!(border.start_rgb, (138, 43, 226));
        assert_eq!(border.end_rgb, (0, 191, 255));
    }

    // ===========================================
    // Animation Tests
    // ===========================================

    #[test]
    fn test_tick_increments_frame_when_animated() {
        let mut config = default_config();
        config.animation_speed = 5;
        let mut border = GradientBorder::new(config);

        assert_eq!(border.animation_frame, 0);
        border.tick();
        assert_eq!(border.animation_frame, 1);
        border.tick();
        assert_eq!(border.animation_frame, 2);
    }

    #[test]
    fn test_tick_does_not_increment_when_not_animated() {
        let mut config = default_config();
        config.animation_speed = 0;
        let mut border = GradientBorder::new(config);

        assert_eq!(border.animation_frame, 0);
        border.tick();
        assert_eq!(border.animation_frame, 0);
    }

    #[test]
    fn test_disable_animation() {
        let mut config = default_config();
        config.animation_speed = 5;
        let mut border = GradientBorder::new(config);

        border.tick();
        assert_eq!(border.animation_frame, 1);

        border.disable_animation();
        border.tick();
        // Should not increment after disabling
        assert_eq!(border.animation_frame, 1);
    }

    #[test]
    fn test_animation_frame_wraps() {
        let mut config = default_config();
        config.animation_speed = 5;
        let mut border = GradientBorder::new(config);

        // Set to max u32 - 1
        border.animation_frame = u32::MAX;
        border.tick();
        // Should wrap to 0
        assert_eq!(border.animation_frame, 0);
    }

    // ===========================================
    // Gradient Direction Color Tests
    // ===========================================

    #[test]
    fn test_horizontal_gradient_colors() {
        let mut config = default_config();
        config.direction = GradientDirection::Horizontal;
        let border = GradientBorder::new(config);

        let area = Rect::new(0, 0, 10, 5);

        // Left edge should be start color (red)
        let left_color = border.get_gradient_color(0, 0, area);
        assert_eq!(left_color, Color::Rgb(255, 0, 0));

        // Right edge should be end color (blue)
        // Note: ratio = 9/10 = 0.9, so not quite pure blue
        let right_color = border.get_gradient_color(9, 0, area);
        // At 90% interpolation: r = 255 * 0.1 = 25, b = 255 * 0.9 = 229
        assert_eq!(right_color, Color::Rgb(25, 0, 229));
    }

    #[test]
    fn test_vertical_gradient_colors() {
        let mut config = default_config();
        config.direction = GradientDirection::Vertical;
        let border = GradientBorder::new(config);

        let area = Rect::new(0, 0, 10, 10);

        // Top edge should be start color
        let top_color = border.get_gradient_color(5, 0, area);
        assert_eq!(top_color, Color::Rgb(255, 0, 0));

        // Bottom edge color at y=9 (ratio = 0.9)
        let bottom_color = border.get_gradient_color(5, 9, area);
        assert_eq!(bottom_color, Color::Rgb(25, 0, 229));
    }

    #[test]
    fn test_diagonal_gradient_colors() {
        let mut config = default_config();
        config.direction = GradientDirection::Diagonal;
        let border = GradientBorder::new(config);

        let area = Rect::new(0, 0, 10, 10);

        // Top-left corner should be start color
        let tl_color = border.get_gradient_color(0, 0, area);
        assert_eq!(tl_color, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn test_radial_gradient_colors() {
        let mut config = default_config();
        config.direction = GradientDirection::Radial;
        let border = GradientBorder::new(config);

        let area = Rect::new(0, 0, 10, 10);

        // Center should be start color
        let center_color = border.get_gradient_color(5, 5, area);
        assert_eq!(center_color, Color::Rgb(255, 0, 0));
    }

    // ===========================================
    // Computed Cached Colors Tests
    // ===========================================

    #[test]
    fn test_compute_cached_colors_valid() {
        let config = GradientBorderConfig {
            enable: true,
            thickness: 1,
            direction: GradientDirection::Horizontal,
            start_color: "#112233".to_string(),
            end_color: "#445566".to_string(),
            middle_color: "#778899".to_string(),
            animation_speed: 0,
        };

        let (start, end, middle) = GradientBorder::compute_cached_colors(&config);
        assert_eq!(start, (0x11, 0x22, 0x33));
        assert_eq!(end, (0x44, 0x55, 0x66));
        assert_eq!(middle, Some((0x77, 0x88, 0x99)));
    }

    #[test]
    fn test_compute_cached_colors_empty_middle() {
        let config = default_config();
        let (_, _, middle) = GradientBorder::compute_cached_colors(&config);
        assert!(middle.is_none());
    }

    #[test]
    fn test_compute_cached_colors_invalid_middle() {
        let mut config = default_config();
        config.middle_color = "not_a_color".to_string();
        let (_, _, middle) = GradientBorder::compute_cached_colors(&config);
        assert!(middle.is_none());
    }
}
