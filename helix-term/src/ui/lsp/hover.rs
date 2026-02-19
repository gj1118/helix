use std::sync::Arc;

use arc_swap::ArcSwap;
use helix_core::syntax;
use helix_lsp::lsp;
use helix_plugin::types::{RenderObject, RenderStyle};
use helix_view::graphics::{Color, Margin, Modifier, Rect, Style}; // Added Modifier, Color
use helix_view::input::Event;
use tui::buffer::Buffer;
use tui::text::Text;
use tui::widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap}; // Added Text

use crate::compositor::{Component, Context, EventResult};

use crate::alt;
use crate::ui::Markdown;

enum HoverContent {
    Markdown(Option<Markdown>, Markdown),
    Custom(RenderObject),
}

pub struct Hover {
    active_index: usize,
    contents: Vec<HoverContent>,
    config_loader: Arc<ArcSwap<syntax::Loader>>,
}

impl Hover {
    pub const ID: &'static str = "hover";

    pub fn new(
        hovers: Vec<(String, lsp::Hover)>,
        config_loader: Arc<ArcSwap<syntax::Loader>>,
        plugin_manager: Option<std::sync::Arc<helix_plugin::PluginManager>>,
    ) -> Self {
        let n_hovers = hovers.len();
        let contents = hovers
            .into_iter()
            .enumerate()
            .map(|(idx, (server_name, hover))| {
                let hover_text = hover_contents_to_string(hover.contents);

                // 1. Try custom rendering first (only if plugins are enabled)
                if let Some(pm) = &plugin_manager {
                    if pm.is_enabled() {
                        if let Some(render_obj) = pm.render_hover(hover_text.clone()) {
                            return HoverContent::Custom(render_obj);
                        }
                    }
                }

                // 2. Fallback to Markdown
                let header = (n_hovers > 1)
                    .then(|| format!("**[{}/{}] {}**\n", idx + 1, n_hovers, server_name))
                    .map(|h| Markdown::new(h, Arc::clone(&config_loader)));

                // 3. Apply text transformation if available (legacy support)
                let body_text = if let Some(pm) = &plugin_manager {
                    if pm.is_enabled() {
                        pm.transform_hover_text(hover_text)
                    } else {
                        hover_text
                    }
                } else {
                    hover_text
                };

                let body = Markdown::new(body_text, Arc::clone(&config_loader));

                HoverContent::Markdown(header, body)
            })
            .collect();

        Self {
            active_index: usize::default(),
            contents,
            config_loader,
        }
    }

    fn content(&self) -> &HoverContent {
        &self.contents[self.active_index]
    }

    pub fn content_string(&self) -> String {
        match self.content() {
            HoverContent::Markdown(header, body) => {
                let mut s = String::new();
                if let Some(h) = header {
                    s.push_str(&h.contents);
                    s.push('\n');
                }
                s.push_str(&body.contents);
                s
            }
            HoverContent::Custom(obj) => {
                // Helper to recursively extract text
                object_to_string(obj)
            }
        }
    }

    fn set_index(&mut self, index: usize) {
        assert!((0..self.contents.len()).contains(&index));
        self.active_index = index;
    }
}

const PADDING_HORIZONTAL: u16 = 2;
const PADDING_TOP: u16 = 1;
const PADDING_BOTTOM: u16 = 1;
const HEADER_HEIGHT: u16 = 1;

impl Component for Hover {
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        match self.content() {
            HoverContent::Markdown(header, body) => {
                let margin = Margin::all(1);
                let area = area.inner(margin);

                // ... Existing Markdown rendering logic ...
                if let Some(header) = header {
                    let header = header.parse(Some(&cx.editor.theme));
                    let header = Paragraph::new(&header);
                    header.render(area.with_height(HEADER_HEIGHT), surface);
                }

                let start_y = if header.is_some() {
                    HEADER_HEIGHT + 1
                } else {
                    0
                };
                let contents = body.parse(Some(&cx.editor.theme));
                let contents_area = area.clip_top(start_y);
                let contents_para = Paragraph::new(&contents)
                    .wrap(Wrap { trim: false })
                    .scroll((cx.scroll.unwrap_or_default() as u16, 0));
                contents_para.render(contents_area, surface);
            }
            HoverContent::Custom(obj) => {
                render_object(obj, area, surface, cx, &self.config_loader);
            }
        }
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let max_text_width = viewport.0.saturating_sub(PADDING_HORIZONTAL).clamp(10, 120);

        match self.content() {
            HoverContent::Markdown(header, body) => {
                // ... Existing calculation ...
                let header_width = header
                    .as_ref()
                    .map(|h| {
                        let h = h.parse(None);
                        crate::ui::text::required_size(&h, max_text_width).0
                    })
                    .unwrap_or_default();

                let b = body.parse(None);
                let (w, h) = crate::ui::text::required_size(&b, max_text_width);

                let width = PADDING_HORIZONTAL + header_width.max(w);
                let height = PADDING_TOP
                    + if header.is_some() {
                        HEADER_HEIGHT + 1
                    } else {
                        0
                    }
                    + h
                    + PADDING_BOTTOM;
                Some((width, height))
            }
            HoverContent::Custom(obj) => {
                let (w, h) = measure_object(obj, max_text_width, &self.config_loader);
                Some((w, h))
            }
        }
    }

    fn handle_event(&mut self, event: &Event, _ctx: &mut Context) -> EventResult {
        let Event::Key(event) = event else {
            return EventResult::Ignored(None);
        };

        match event {
            alt!('p') => {
                let index = self
                    .active_index
                    .checked_sub(1)
                    .unwrap_or(self.contents.len() - 1);
                self.set_index(index);
                EventResult::Consumed(None)
            }
            alt!('n') => {
                self.set_index((self.active_index + 1) % self.contents.len());
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }
}

// Custom Rendering Helpers

fn parse_color(theme: &helix_view::Theme, color_spec: &str) -> Option<Color> {
    if color_spec.starts_with('#') {
        if color_spec.len() == 7 {
            let r = u8::from_str_radix(&color_spec[1..3], 16).ok()?;
            let g = u8::from_str_radix(&color_spec[3..5], 16).ok()?;
            let b = u8::from_str_radix(&color_spec[5..7], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    } else {
        let style = theme.get(color_spec);
        return style.fg;
    }
    None
}

fn interpolate_color(c1: Color, c2: Color, t: f32) -> Color {
    match (c1, c2) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let r = (r1 as f32 + (r2 as f32 - r1 as f32) * t) as u8;
            let g = (g1 as f32 + (g2 as f32 - g1 as f32) * t) as u8;
            let b = (b1 as f32 + (b2 as f32 - b1 as f32) * t) as u8;
            Color::Rgb(r, g, b)
        }
        _ => c1,
    }
}

fn get_gradient_color(colors: &[Color], t: f32) -> Color {
    if colors.is_empty() {
        return Color::Reset;
    }
    if colors.len() == 1 {
        return colors[0];
    }

    let scaled_t = t * (colors.len() - 1) as f32;
    let idx = scaled_t.floor() as usize;
    let params = scaled_t - idx as f32;

    let c1 = colors[idx.min(colors.len() - 1)];
    let c2 = colors[(idx + 1).min(colors.len() - 1)];

    interpolate_color(c1, c2, params)
}

fn draw_gradient_border(
    area: Rect,
    surface: &mut Buffer,
    gradient: &helix_plugin::types::RenderGradient,
    theme: &helix_view::Theme,
) -> Rect {
    // Return inner area
    let colors: Vec<Color> = gradient
        .colors
        .iter()
        .filter_map(|c| parse_color(theme, c))
        .collect();

    if colors.is_empty() {
        // Fallback to default border if no valid colors
        let b = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        let inner = b.inner(area);
        b.render(area, surface);
        return inner;
    }

    if colors.len() == 1 {
        let style = Style::default().fg(colors[0]);
        let b = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(style);
        let inner = b.inner(area);
        b.render(area, surface);
        return inner;
    }

    // Manual Gradient Drawing
    // Extract bounds to avoid borrow conflict with surface.get_mut()
    let buf_left = surface.area.left();
    let buf_right = surface.area.right();
    let buf_top = surface.area.top();
    let buf_bottom = surface.area.bottom();

    // Top
    for x in area.left()..area.right() {
        if x < buf_left || x >= buf_right || area.top() < buf_top || area.top() >= buf_bottom {
            continue;
        }

        let t = (x - area.left()) as f32 / (area.width.saturating_sub(1)).max(1) as f32;
        let color = get_gradient_color(&colors, t);
        let syn = if x == area.left() {
            "╭"
        } else if x == area.right() - 1 {
            "╮"
        } else {
            "─"
        };
        if let Some(cell) = surface.get_mut(x, area.top()) {
            cell.set_symbol(syn).set_fg(color);
        }
    }
    // Bottom
    let bottom_y = area.bottom().saturating_sub(1);
    for x in area.left()..area.right() {
        if x < buf_left || x >= buf_right || bottom_y < buf_top || bottom_y >= buf_bottom {
            continue;
        }

        let t = (x - area.left()) as f32 / (area.width.saturating_sub(1)).max(1) as f32;
        let color = get_gradient_color(&colors, t);
        let syn = if x == area.left() {
            "╰"
        } else if x == area.right() - 1 {
            "╯"
        } else {
            "─"
        };
        if let Some(cell) = surface.get_mut(x, bottom_y) {
            cell.set_symbol(syn).set_fg(color);
        }
    }
    // Left & Right (Vertical)
    for y in (area.top() + 1)..bottom_y {
        if y < buf_top || y >= buf_bottom {
            continue;
        }

        let c_left = get_gradient_color(&colors, 0.0);
        if area.left() >= buf_left && area.left() < buf_right {
            if let Some(cell) = surface.get_mut(area.left(), y) {
                cell.set_symbol("│").set_fg(c_left);
            }
        }

        let c_right = get_gradient_color(&colors, 1.0);
        let right_x = area.right().saturating_sub(1);
        if right_x >= buf_left && right_x < buf_right {
            if let Some(cell) = surface.get_mut(right_x, y) {
                cell.set_symbol("│").set_fg(c_right);
            }
        }
    }

    let b = Block::default().borders(Borders::ALL);
    b.inner(area)
}

fn object_to_string(obj: &RenderObject) -> String {
    match obj {
        RenderObject::Text { content, .. }
        | RenderObject::Markdown { content }
        | RenderObject::Code { content, .. } => content.clone(),
        RenderObject::Separator { .. } => "─".repeat(40),
        RenderObject::Block { children, .. } => children
            .iter()
            .map(object_to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn to_tui_style(style: &Option<RenderStyle>, theme: &helix_view::Theme) -> Style {
    let mut s = Style::default();
    if let Some(conf) = style {
        if let Some(fg) = &conf.fg {
            if let Ok(color) = theme.get(fg).fg.ok_or(()) {
                s = s.fg(color);
            }
            // Fallback: try parsing hex? For now assume theme keys
        }
        if let Some(bg) = &conf.bg {
            if let Ok(color) = theme.get(bg).bg.ok_or(()) {
                s = s.bg(color);
            }
        }
        if let Some(mods) = &conf.modifiers {
            for m in mods {
                match m.as_str() {
                    "bold" => s = s.add_modifier(Modifier::BOLD),
                    "italic" => s = s.add_modifier(Modifier::ITALIC),
                    _ => {}
                }
            }
        }
    }
    s
}

fn measure_object(
    obj: &RenderObject,
    max_width: u16,
    loader: &Arc<ArcSwap<syntax::Loader>>,
) -> (u16, u16) {
    match obj {
        RenderObject::Text { content, .. } => (content.len().min(max_width as usize) as u16, 1),
        RenderObject::Markdown { content } => {
            let md = Markdown::new(content.clone(), Arc::clone(loader));
            let parsed = md.parse(None);
            crate::ui::text::required_size(&parsed, max_width)
        }
        RenderObject::Code { content, .. } => {
            // simplified measurement
            let lines: Vec<&str> = content.lines().collect();
            let w = lines.iter().map(|l| l.len()).max().unwrap_or(0) as u16;
            (w.min(max_width), lines.len() as u16)
        }
        RenderObject::Separator { .. } => (0, 1),
        RenderObject::Block {
            children,
            direction,
            border,
            ..
        } => {
            let is_horizontal = direction.as_deref() == Some("horizontal");
            let mut width = 0;
            let mut height = 0;

            for child in children {
                let (cw, ch) = measure_object(child, max_width, loader);
                if is_horizontal {
                    width += cw;
                    height = height.max(ch);
                } else {
                    width = width.max(cw);
                    height += ch;
                }
            }

            // Add space for borders
            if border.is_some() {
                width += 2;
                height += 2;
            }

            (width, height)
        }
    }
}

fn render_object(
    obj: &RenderObject,
    area: Rect,
    surface: &mut Buffer,
    cx: &Context,
    loader: &Arc<ArcSwap<syntax::Loader>>,
) {
    if area.area() == 0 {
        return;
    }

    match obj {
        RenderObject::Text { content, style } => {
            let s = to_tui_style(style, &cx.editor.theme);
            surface.set_stringn(area.x, area.y, content, area.width as usize, s);
        }
        RenderObject::Markdown { content } => {
            let md = Markdown::new(content.clone(), Arc::clone(loader));
            let parsed = md.parse(Some(&cx.editor.theme));
            let para = Paragraph::new(&parsed)
                .wrap(Wrap { trim: false })
                .scroll((cx.scroll.unwrap_or(0) as u16, 0));
            para.render(area, surface);
        }
        RenderObject::Code { content, .. } => {
            // TODO: Syntax highlighting
            let text = Text::from(content.as_str());
            let para = Paragraph::new(&text);
            para.render(area, surface);
        }
        RenderObject::Separator { style } => {
            let s = to_tui_style(style, &cx.editor.theme);
            let buf_left = surface.area.left();
            let buf_right = surface.area.right();
            let buf_top = surface.area.top();
            let buf_bottom = surface.area.bottom();

            if area.y >= buf_top && area.y < buf_bottom {
                for x in area.left()..area.right() {
                    if x >= buf_left && x < buf_right {
                        if let Some(cell) = surface.get_mut(x, area.y) {
                            cell.set_symbol("─").set_style(s);
                        }
                    }
                }
            }
        }
        RenderObject::Block {
            children,
            direction,
            style,
            border,
            title: _title,
            ..
        } => {
            let mut inner_area = area;

            // Render Border
            if let Some(border_type) = border {
                let check_border = match border_type.as_str() {
                    "rounded" | "all" => Borders::ALL,
                    "top" => Borders::TOP,
                    "bottom" => Borders::BOTTOM,
                    "left" => Borders::LEFT,
                    "right" => Borders::RIGHT,
                    _ => Borders::empty(),
                };

                if check_border != Borders::empty() {
                    let has_gradient = style.as_ref().and_then(|s| s.gradient.as_ref());

                    if let Some(gradient) = has_gradient {
                        inner_area =
                            draw_gradient_border(area, surface, gradient, &cx.editor.theme);
                    } else {
                        let border_render_type = if check_border == Borders::ALL {
                            BorderType::Rounded
                        } else {
                            BorderType::Plain
                        };
                        let b = Block::default()
                            .borders(check_border)
                            .border_type(border_render_type)
                            .style(to_tui_style(style, &cx.editor.theme));

                        inner_area = b.inner(area);
                        b.render(area, surface);
                    }
                }
            }

            let is_horizontal = direction.as_deref() == Some("horizontal");
            let mut current_x = inner_area.x;
            let mut current_y = inner_area.y;

            for child in children {
                let (req_w, req_h) = measure_object(child, inner_area.width, loader);

                let child_area = if is_horizontal {
                    Rect::new(current_x, current_y, req_w, inner_area.height)
                } else {
                    // Constrain height to remaining available space in the inner area
                    let available_height = inner_area.bottom().saturating_sub(current_y);
                    let constrained_height = req_h.min(available_height);
                    Rect::new(current_x, current_y, inner_area.width, constrained_height)
                };

                render_object(child, child_area, surface, cx, loader);

                if is_horizontal {
                    current_x += req_w;
                } else {
                    current_y += req_h.min(inner_area.bottom().saturating_sub(current_y));
                }

                // prevent overflow
                if is_horizontal && current_x >= inner_area.right() {
                    break;
                }
                if !is_horizontal && current_y >= inner_area.bottom() {
                    break;
                }
            }
        }
    }
}

fn hover_contents_to_string(contents: lsp::HoverContents) -> String {
    fn marked_string_to_markdown(contents: lsp::MarkedString) -> String {
        match contents {
            lsp::MarkedString::String(contents) => contents,
            lsp::MarkedString::LanguageString(string) => {
                if string.language == "markdown" {
                    string.value
                } else {
                    format!("```{}\n{}\n```", string.language, string.value)
                }
            }
        }
    }
    match contents {
        lsp::HoverContents::Scalar(contents) => marked_string_to_markdown(contents),
        lsp::HoverContents::Array(contents) => contents
            .into_iter()
            .map(marked_string_to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp::HoverContents::Markup(contents) => contents.value,
    }
}
