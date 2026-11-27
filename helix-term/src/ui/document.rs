use std::{borrow::Cow, cmp::min};

use helix_core::doc_formatter::{DocumentFormatter, FormattedGrapheme, GraphemeSource, TextFormat};
use helix_core::graphemes::{render_grapheme, Grapheme};
use helix_core::str_utils::char_to_byte_idx;
use helix_core::syntax::{self, HighlightEvent, Highlighter, OverlayHighlights};
use helix_core::text_annotations::TextAnnotations;
use helix_core::{visual_offset_from_block, Position, RopeSlice};
use helix_stdx::rope::RopeSliceExt;
use helix_view::editor::{WhitespaceConfig, WhitespaceRenderValue};
use helix_view::graphics::Rect;
use helix_view::theme::Style;
use helix_view::view::ViewPosition;
use helix_view::{Document, Theme};
use tui::buffer::Buffer as Surface;

use crate::ui::text_decorations::DecorationManager;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct LinePos {
    /// Indicates whether the given visual line
    /// is the first visual line of the given document line
    pub first_visual_line: bool,
    /// The line index of the document line that contains the given visual line
    pub doc_line: usize,
    /// Vertical offset from the top of the inner view area
    pub visual_line: u16,
    /// The given visual line is the last visual line of the document line
    pub is_last_visual_line: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn render_document(
    surface: &mut Surface,
    viewport: Rect,
    doc: &Document,
    offset: ViewPosition,
    doc_annotations: &TextAnnotations,
    syntax_highlighter: Option<SyntaxHighlights<'_>>,
    overlay_highlights: Vec<syntax::OverlayHighlights>,
    theme: &Theme,
    decorations: DecorationManager,
) {
    let mut renderer = TextRenderer::new(
        surface,
        doc,
        theme,
        Position::new(offset.vertical_offset, offset.horizontal_offset),
        viewport,
    );
    render_text(
        &mut renderer,
        doc.text().slice(..),
        offset.anchor,
        &doc.text_format(viewport.width, Some(theme)),
        doc_annotations,
        syntax_highlighter,
        overlay_highlights,
        theme,
        decorations,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text(
    renderer: &mut TextRenderer,
    text: RopeSlice<'_>,
    anchor: usize,
    text_fmt: &TextFormat,
    text_annotations: &TextAnnotations,
    syntax_highlighter: Option<SyntaxHighlights<'_>>,
    overlay_highlights: Vec<syntax::OverlayHighlights>,
    theme: &Theme,
    mut decorations: DecorationManager,
) {
    let row_off = visual_offset_from_block(text, anchor, anchor, text_fmt, text_annotations)
        .0
        .row;

    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, text_annotations, anchor);
    let mut syntax_highlighter =
        SyntaxHighlighter::new(syntax_highlighter, text, theme, renderer.text_style);
    let mut overlay_highlighter = OverlayHighlighter::new(overlay_highlights, theme);

    let mut last_line_pos = LinePos {
        first_visual_line: false,
        doc_line: usize::MAX,
        visual_line: u16::MAX,
        is_last_visual_line: true,
    };
    let mut last_line_end = 0;
    let mut is_in_indent_area = true;
    let mut last_line_indent_level = 0;
    let mut reached_view_top = false;

    // the columns drawn (without soft wrap, lines go past them on both sides)
    let first_col = renderer.offset.col;
    let end_col = first_col.saturating_add(renderer.viewport.width as usize);

    loop {
        // Without soft wrap, the plain text left of the view and right of it (a long line) is
        // moved past without being drawn grapheme by grapheme: to the first column drawn, or to
        // the end of the line. It stops before what decorations wait for.
        if reached_view_top && !text_fmt.soft_wrap {
            let col = formatter.next_visual_pos().col;
            let col_limit = if col < first_col {
                first_col
            } else if col >= end_col {
                usize::MAX
            } else {
                0
            };
            if col_limit > col {
                formatter.skip_plain_text(decorations.next_anchor(), col_limit);
            }
        }
        let Some(mut grapheme) = formatter.next() else {
            break;
        };

        // skip any graphemes on visual lines before the block start
        if grapheme.visual_pos.row < row_off {
            continue;
        }
        grapheme.visual_pos.row -= row_off;
        if !reached_view_top {
            decorations.prepare_for_rendering(grapheme.char_idx);
            reached_view_top = true;
        }

        // if the end of the viewport is reached stop rendering
        if grapheme.visual_pos.row as u16 >= renderer.viewport.height + renderer.offset.row as u16 {
            break;
        }

        // apply decorations before rendering a new line
        if grapheme.visual_pos.row as u16 != last_line_pos.visual_line {
            if last_line_pos.doc_line == grapheme.line_idx {
                last_line_pos.is_last_visual_line = false;
            }

            // we initiate doc_line with usize::MAX because no file
            // can reach that size (memory allocations are limited to isize::MAX)
            // initially there is no "previous" line (so doc_line is set to usize::MAX)
            // in that case we don't need to draw indent guides/virtual text
            if last_line_pos.doc_line != usize::MAX {
                // draw indent guides for the last line
                renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
                is_in_indent_area = true;
                decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
            }
            last_line_pos = LinePos {
                first_visual_line: grapheme.line_idx != last_line_pos.doc_line,
                doc_line: grapheme.line_idx,
                visual_line: grapheme.visual_pos.row as u16,
                is_last_visual_line: true,
            };
            decorations.decorate_line(renderer, last_line_pos);
        }

        // acquire the correct grapheme style
        while grapheme.char_idx >= syntax_highlighter.pos {
            syntax_highlighter.advance();
        }
        while grapheme.char_idx >= overlay_highlighter.pos {
            overlay_highlighter.advance();
        }

        let grapheme_style = if let GraphemeSource::VirtualText { highlight } = grapheme.source {
            let mut style = renderer.text_style;
            if let Some(highlight) = highlight {
                style = style.patch(theme.highlight(highlight));
            }
            GraphemeStyle {
                syntax_style: style,
                overlay_style: Style::default(),
            }
        } else {
            GraphemeStyle {
                syntax_style: syntax_highlighter.style,
                overlay_style: overlay_highlighter.style,
            }
        };
        decorations.decorate_grapheme(renderer, &grapheme);

        let virt = grapheme.is_virtual();
        let grapheme_width = renderer.draw_grapheme(
            &grapheme,
            grapheme_style,
            virt,
            &mut last_line_indent_level,
            &mut is_in_indent_area,
            grapheme.visual_pos,
        );
        last_line_end = grapheme.visual_pos.col + grapheme_width;
    }

    // (no line was drawn in a view without rows: there is none to hang virtual lines from)
    if last_line_pos.doc_line != usize::MAX {
        renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
        decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
    }
}

#[derive(Debug)]
pub struct TextRenderer<'a> {
    surface: &'a mut Surface,
    pub text_style: Style,
    pub whitespace_style: Style,
    pub indent_guide_char: String,
    pub indent_guide_style: Style,
    pub newline: String,
    pub nbsp: String,
    pub nnbsp: String,
    pub space: String,
    pub tab: String,
    pub virtual_tab: String,
    pub indent_width: u16,
    pub starting_indent: usize,
    pub draw_indent_guides: bool,
    pub viewport: Rect,
    pub offset: Position,
}

pub struct GraphemeStyle {
    syntax_style: Style,
    overlay_style: Style,
}

impl<'a> TextRenderer<'a> {
    pub fn new(
        surface: &'a mut Surface,
        doc: &Document,
        theme: &Theme,
        offset: Position,
        viewport: Rect,
    ) -> TextRenderer<'a> {
        let editor_config = doc.config.load();
        let WhitespaceConfig {
            render: ws_render,
            characters: ws_chars,
        } = &editor_config.whitespace;

        let tab_width = doc.tab_width();
        let tab = if ws_render.tab() == WhitespaceRenderValue::All {
            std::iter::once(ws_chars.tab)
                .chain(std::iter::repeat_n(ws_chars.tabpad, tab_width - 1))
                .collect()
        } else {
            " ".repeat(tab_width)
        };
        let virtual_tab = " ".repeat(tab_width);
        let newline = if ws_render.newline() == WhitespaceRenderValue::All {
            ws_chars.newline.into()
        } else {
            " ".to_owned()
        };

        let space = if ws_render.space() == WhitespaceRenderValue::All {
            ws_chars.space.into()
        } else {
            " ".to_owned()
        };
        let nbsp = if ws_render.nbsp() == WhitespaceRenderValue::All {
            ws_chars.nbsp.into()
        } else {
            " ".to_owned()
        };
        let nnbsp = if ws_render.nnbsp() == WhitespaceRenderValue::All {
            ws_chars.nnbsp.into()
        } else {
            " ".to_owned()
        };

        let text_style = theme.get("ui.text");

        let indent_width = doc.indent_style.indent_width(tab_width) as u16;

        TextRenderer {
            surface,
            indent_guide_char: editor_config.indent_guides.character.into(),
            newline,
            nbsp,
            nnbsp,
            space,
            tab,
            virtual_tab,
            whitespace_style: theme.get("ui.virtual.whitespace"),
            indent_width,
            starting_indent: offset.col / indent_width as usize
                + !offset.col.is_multiple_of(indent_width as usize) as usize
                + editor_config.indent_guides.skip_levels as usize,
            indent_guide_style: text_style.patch(
                theme
                    .try_get("ui.virtual.indent-guide")
                    .unwrap_or_else(|| theme.get("ui.virtual.whitespace")),
            ),
            text_style,
            draw_indent_guides: editor_config.indent_guides.render,
            viewport,
            offset,
        }
    }
    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_decoration_grapheme(
        &mut self,
        grapheme: Grapheme,
        style: Style,
        mut row: u16,
        col: u16,
    ) -> bool {
        if (row as usize) < self.offset.row
            || row >= self.viewport.height
            || col >= self.viewport.width
        {
            return false;
        }
        row -= self.offset.row as u16;

        let grapheme = match grapheme {
            Grapheme::Tab { width } => {
                let grapheme_tab_width = char_to_byte_idx(&self.virtual_tab, width);
                &self.virtual_tab[..grapheme_tab_width]
            }
            Grapheme::Other { ref g } if g == "\u{00A0}" => " ",
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => " ",
        };

        self.surface.set_string(
            self.viewport.x + col,
            self.viewport.y + row,
            grapheme,
            style,
        );
        true
    }

    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_grapheme(
        &mut self,
        grapheme: &FormattedGrapheme,
        grapheme_style: GraphemeStyle,
        is_virtual: bool,
        last_indent_level: &mut usize,
        is_in_indent_area: &mut bool,
        mut position: Position,
    ) -> usize {
        if position.row < self.offset.row {
            return 0;
        }
        position.row -= self.offset.row;
        let cut_off_start = self.offset.col.saturating_sub(position.col);
        let is_whitespace = grapheme.is_whitespace();

        // TODO is it correct to apply the whitespace style to all unicode white spaces?
        let mut style = grapheme_style.syntax_style;
        if is_whitespace {
            style = style.patch(self.whitespace_style);
        }
        style = style.patch(grapheme_style.overlay_style);

        let width = grapheme.width();
        let mut is_tab = false;
        let space = if is_virtual { " " } else { &self.space };
        let nbsp = if is_virtual { " " } else { &self.nbsp };
        let nnbsp = if is_virtual { " " } else { &self.nnbsp };
        let tab = if is_virtual {
            &self.virtual_tab
        } else {
            &self.tab
        };
        let grapheme = match grapheme.raw {
            Grapheme::Tab { width } => {
                is_tab = true;
                let grapheme_tab_width = char_to_byte_idx(tab, width);
                Cow::Borrowed(&tab[..grapheme_tab_width])
            }
            // TODO special rendering for other whitespaces?
            Grapheme::Other { ref g } if g == " " && !grapheme.source.is_eof() => {
                Cow::Borrowed(space)
            }
            Grapheme::Other { ref g } if g == "\u{00A0}" => Cow::Borrowed(nbsp),
            Grapheme::Other { ref g } if g == "\u{202F}" => Cow::Borrowed(nnbsp),
            Grapheme::Other { ref g } => render_grapheme(g),
            Grapheme::Newline => Cow::Borrowed(self.newline.as_str()),
        };

        let in_bounds = self.column_in_bounds(position.col, width);

        if in_bounds {
            let x = self.viewport.x + (position.col - self.offset.col) as u16;
            let y = self.viewport.y + position.row as u16;
            if is_tab {
                // A tab expands to `width` single-column cells; writing them
                // individually keeps background styles (selection, cursorline)
                // across the whole tab and avoids the redraw diff clipping
                // `render-whitespace` pads. A single `set_grapheme` would pack
                // them into one wide cell and leave the rest unstyled.
                self.surface.set_tab(x, y, &grapheme, style);
            } else {
                self.surface.set_grapheme(x, y, &grapheme, width, style);
            }
        } else if cut_off_start != 0 && cut_off_start < width {
            // partially on screen
            let rect = Rect::new(
                self.viewport.x,
                self.viewport.y + position.row as u16,
                (width - cut_off_start) as u16,
                1,
            );
            self.surface.set_style(rect, style);
        }
        if *is_in_indent_area && !is_whitespace {
            *last_indent_level = position.col;
            *is_in_indent_area = false;
        }

        width
    }

    pub fn column_in_bounds(&self, colum: usize, width: usize) -> bool {
        self.offset.col <= colum && colum + width <= self.offset.col + self.viewport.width as usize
    }

    /// Overlay indentation guides ontop of a rendered line
    /// The indentation level is computed in `draw_lines`.
    /// Therefore this function must always be called afterwards.
    pub fn draw_indent_guides(&mut self, indent_level: usize, mut row: u16) {
        if !self.draw_indent_guides || self.offset.row > row as usize {
            return;
        }
        row -= self.offset.row as u16;

        // Don't draw indent guides outside of view
        let end_indent = min(
            indent_level,
            // Add indent_width - 1 to round up, since the first visible
            // indent might be a bit after offset.col
            self.offset.col + self.viewport.width as usize + (self.indent_width as usize - 1),
        ) / self.indent_width as usize;

        for i in self.starting_indent..end_indent {
            let x = (self.viewport.x as usize + (i * self.indent_width as usize) - self.offset.col)
                as u16;
            let y = self.viewport.y + row;
            debug_assert!(self.surface.in_bounds(x, y));
            self.surface
                .set_string(x, y, &self.indent_guide_char, self.indent_guide_style);
        }
    }

    /// The surface row of row `y` of the text area, or `None` if it is scrolled out of view.
    /// The row of the screen of the text's row `y` (`None` above the view).
    pub(crate) fn surface_row(&self, y: u16) -> Option<u16> {
        let row = (y as usize).checked_sub(self.offset.row)?;
        Some(row as u16 + self.viewport.y)
    }

    pub fn set_string(&mut self, x: u16, y: u16, string: &str, style: Style) {
        if let Some(y) = self.surface_row(y) {
            self.surface.set_string(x, y, string, style)
        }
    }

    pub fn set_stringn(&mut self, x: u16, y: u16, string: &str, width: usize, style: Style) {
        if let Some(y) = self.surface_row(y) {
            self.surface.set_stringn(x, y, string, width, style);
        }
    }

    /// Sets the style of an area **within the text viewport* this accounts
    /// both for the renderers vertical offset and its viewport
    pub fn set_style(&mut self, mut area: Rect, style: Style) {
        let top = area.y.max(self.offset.row as u16);
        area.height = area.bottom().saturating_sub(top);
        let Some(y) = self.surface_row(top) else {
            return;
        };
        area.y = y;
        self.surface.set_style(area, style);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_string_truncated(
        &mut self,
        x: u16,
        y: u16,
        string: &str,
        width: usize,
        style: impl Fn(usize) -> Style, // Map a grapheme's string offset to a style
        ellipsis: bool,
        truncate_start: bool,
    ) -> (u16, u16) {
        let Some(surface_y) = self.surface_row(y) else {
            return (x, y);
        };
        self.surface.set_string_truncated(
            x,
            surface_y,
            string,
            width,
            style,
            ellipsis,
            truncate_start,
        )
    }
}

/// The syntax highlights of the text drawn: from the highlighter, or as recorded from one
/// before (see [`RecordedHighlights`]).
pub enum SyntaxHighlights<'h> {
    Live(Highlighter<'h>),
    Recorded {
        highlights: std::sync::Arc<RecordedHighlights>,
        /// The next event.
        next: usize,
    },
}

impl<'h> From<Highlighter<'h>> for SyntaxHighlights<'h> {
    fn from(highlighter: Highlighter<'h>) -> Self {
        Self::Live(highlighter)
    }
}

impl From<std::sync::Arc<RecordedHighlights>> for SyntaxHighlights<'_> {
    fn from(highlights: std::sync::Arc<RecordedHighlights>) -> Self {
        Self::Recorded {
            highlights,
            next: 0,
        }
    }
}

impl SyntaxHighlights<'_> {
    fn next_event_offset(&self) -> u32 {
        match self {
            Self::Live(highlighter) => highlighter.next_event_offset(),
            Self::Recorded { highlights, next } => highlights
                .events
                .get(*next)
                .map_or(u32::MAX, |event| event.offset),
        }
    }

    /// The next event and the highlights it sets, as the highlighter's `advance`.
    fn advance(
        &mut self,
        mut each: impl FnMut(HighlightEvent, &mut dyn Iterator<Item = syntax::Highlight>),
    ) {
        match self {
            Self::Live(highlighter) => {
                let (event, mut highlights) = highlighter.advance();
                each(event, &mut highlights);
            }
            Self::Recorded { highlights, next } => {
                let Some(event) = highlights.events.get(*next) else {
                    return;
                };
                *next += 1;
                let list = &highlights.highlights[event.highlights.clone()];
                each(event.event, &mut list.iter().copied());
            }
        }
    }
}

/// The events of a highlighter over a range of the text, all of them: drawn again and again
/// while the text, its syntax tree and the range stay the same (a cursor moving within the
/// lines in view), without running the queries again.
#[derive(Debug, Default)]
pub struct RecordedHighlights {
    events: Vec<RecordedEvent>,
    /// The highlights of every event, one after the other.
    highlights: Vec<syntax::Highlight>,
}

#[derive(Debug)]
struct RecordedEvent {
    /// Where it happens (a byte of the text).
    offset: u32,
    event: HighlightEvent,
    /// Its highlights in [`RecordedHighlights::highlights`].
    highlights: std::ops::Range<usize>,
}

impl RecordedHighlights {
    /// Most events kept: a range that never ends is not recorded forever.
    const MAX_EVENTS: usize = 1 << 20;

    /// Runs `highlighter` to its end. `None` when it has too many events (drawn live then).
    pub fn record(mut highlighter: Highlighter<'_>) -> Option<Self> {
        let mut recorded = Self::default();
        loop {
            let offset = highlighter.next_event_offset();
            if offset == u32::MAX {
                return Some(recorded);
            }
            if recorded.events.len() >= Self::MAX_EVENTS {
                return None;
            }
            let (event, highlights) = highlighter.advance();
            let start = recorded.highlights.len();
            recorded.highlights.extend(highlights);
            recorded.events.push(RecordedEvent {
                offset,
                event,
                highlights: start..recorded.highlights.len(),
            });
        }
    }
}

struct SyntaxHighlighter<'h, 'r, 't> {
    inner: Option<SyntaxHighlights<'h>>,
    text: RopeSlice<'r>,
    /// The character index of the next highlight event, or `usize::MAX` if the highlighter is
    /// finished.
    pos: usize,
    theme: &'t Theme,
    text_style: Style,
    style: Style,
}

impl<'h, 'r, 't> SyntaxHighlighter<'h, 'r, 't> {
    fn new(
        inner: Option<SyntaxHighlights<'h>>,
        text: RopeSlice<'r>,
        theme: &'t Theme,
        text_style: Style,
    ) -> Self {
        let mut highlighter = Self {
            inner,
            text,
            pos: 0,
            theme,
            style: text_style,
            text_style,
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self
            .inner
            .as_ref()
            .and_then(|highlighter| {
                let next_byte_idx = highlighter.next_event_offset();
                (next_byte_idx != u32::MAX).then(|| {
                    // Move the byte index to the nearest character boundary (rounding up) and
                    // convert it to a character index.
                    self.text
                        .byte_to_char(self.text.ceil_char_boundary(next_byte_idx as usize))
                })
            })
            .unwrap_or(usize::MAX);
    }

    fn advance(&mut self) {
        let Some(highlighter) = self.inner.as_mut() else {
            return;
        };

        let (text_style, style, theme) = (self.text_style, &mut self.style, self.theme);
        highlighter.advance(|event, highlights| {
            let base = match event {
                HighlightEvent::Refresh => text_style,
                HighlightEvent::Push => *style,
            };
            *style = highlights.fold(base, |acc, highlight| acc.patch(theme.highlight(highlight)));
        });
        self.update_pos();
    }
}

struct OverlayHighlighter<'t> {
    inner: syntax::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: Vec<OverlayHighlights>, theme: &'t Theme) -> Self {
        let inner = syntax::OverlayHighlighter::new(overlays);
        let mut highlighter = Self {
            inner,
            pos: 0,
            theme,
            style: Style::default(),
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self.inner.next_event_offset();
    }

    fn advance(&mut self) {
        let (event, highlights) = self.inner.advance();
        let base = match event {
            HighlightEvent::Refresh => Style::default(),
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            acc.patch(self.theme.highlight(highlight))
        });
        self.update_pos();
    }
}
