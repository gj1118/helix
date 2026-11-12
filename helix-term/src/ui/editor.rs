use crate::{
    commands::{self, OnKeyCallback, OnKeyCallbackKind},
    compositor::{Component, Context, Event, EventResult},
    events::{OnModeSwitch, PostCommand},
    handlers::completion::CompletionItem,
    key,
    keymap::{KeymapResult, Keymaps},
    ui::{
        document::{render_document, LinePos, TextRenderer},
        statusline,
        text_decorations::{
            self, Decoration, DecorationManager, FoldDecoration, InlineDiagnostics,
            PluginDecoration,
        },
        Completion, NotificationPopup, ProgressSpinners,
    },
};

use helix_core::{
    diagnostic::NumberOrString,
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    movement::Direction,
    syntax::{self, OverlayHighlights},
    text_annotations::TextAnnotations,
    text_folding::RopeSliceFoldExt,
    unicode::width::UnicodeWidthStr,
    visual_offset_from_block, Change, Position, Range, Selection, Transaction,
};
use helix_loader::VERSION_AND_GIT_HASH;
use helix_lsp::lsp::SymbolKind;
use helix_view::{
    // annotations::diagnostics::DiagnosticFilter,
    document::{Mode, SCRATCH_BUFFER_NAME},
    editor::{CompleteAction, InlineBlameConfig, InlineBlameShow},
    graphics::{Color, CursorKind, Modifier, Rect, Style},
    icons::ICONS,
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    keyboard::{KeyCode, KeyModifiers},
    Document,
    DocumentId,
    Editor,
    Theme,
    View,
    ViewId,
};
use std::{
    mem::take,
    num::NonZeroUsize,
    ops,
    path::{PathBuf, MAIN_SEPARATOR, MAIN_SEPARATOR_STR},
    rc::Rc,
};

use tui::{
    buffer::{Buffer as Surface, Cell},
    text::{Span, Spans},
};

use super::text_decorations::blame::InlineBlame;

pub struct EditorView {
    pub keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    pseudo_pending: Vec<KeyEvent>,
    /// Keys of a diff view held back: they start the keys of a diff action (`]c`, `>>`...).
    diff_pending: Vec<KeyEvent>,
    pub(crate) last_insert: (commands::MappableCommand, Vec<InsertEvent>),
    pub(crate) completion: Option<Completion>,
    /// The view and document the completion was opened in: its preview and savepoint belong
    /// to them, even when another view or document gets the focus.
    completion_origin: Option<(ViewId, DocumentId)>,
    spinners: ProgressSpinners,
    bufferline_info: BufferLineInfo,
    /// Tracks if the terminal window is focused by reaction to terminal focus events
    terminal_focused: bool,
    /// Tracks if there are prompt layers active (updated by compositor)
    pub prompt_active: bool,
    /// The focused view's secondary cursors when the terminal draws them (kitty's multiple
    /// cursors protocol): their screen cells, with the cell as the views were drawn, to tell
    /// whether something was drawn over them afterwards.
    terminal_secondary_cursors: Vec<(u16, u16, Cell)>,
    notification_popup: NotificationPopup,
    /// Cache for bufferline display names to avoid O(n³) recomputation per frame
    bufferline_cache: BufferlineCache,
    /// The split divider being dragged with the mouse, if any.
    split_drag: Option<helix_view::tree::SplitDivider>,
}

/// Cached data for bufferline rendering.
/// Names (display name per document) are recomputed only when document set changes.
/// Texts (formatted strings including modified indicator) are recomputed when
/// document modified status or document set changes.
#[derive(Default)]
struct BufferlineCache {
    doc_ids: Vec<DocumentId>,
    doc_paths: Vec<String>,
    names: Vec<(DocumentId, String)>,
    modified: Vec<bool>,
    /// The language of each document: it picks the icon.
    languages: Vec<Option<String>>,
    /// The separator the positions were laid out with.
    separator: String,
    texts: Vec<String>,
    widths: Vec<usize>,
    positions: Vec<usize>,
    total_width: usize,
}

#[derive(Debug, Clone)]
pub enum InsertEvent {
    Key(KeyEvent),
    CompletionApply {
        trigger_offset: usize,
        changes: Vec<Change>,
    },
    TriggerCompletion,
    RequestCompletion,
}

/// The pointer shown over a split divider.
fn split_pointer(divider: helix_view::tree::SplitDivider) -> helix_view::editor::PointerShape {
    use helix_view::editor::PointerShape;
    if divider.vertical {
        PointerShape::ColResize
    } else {
        PointerShape::RowResize
    }
}

/// While the explorer tree has the keyboard, a movement command the keymap resolved to
/// (`j`, `gg`, `ge`, `C-d`, with any count) moves the tree's cursor instead of the buffer's.
/// Returns whether the command was handed to the tree.
fn route_motion_to_explorer(
    cxt: &mut commands::Context,
    command: &commands::MappableCommand,
) -> bool {
    let panel = &cxt.editor.explorer_panel;
    if !(panel.visible && panel.focused) || cxt.editor.mode != Mode::Normal {
        return false;
    }
    let commands::MappableCommand::Static { name, .. } = command else {
        return false;
    };
    let Some(motion) = super::explorer::Motion::from_command(name) else {
        return false;
    };
    let count = cxt.count.map(NonZeroUsize::get);
    cxt.callback.push(Box::new(move |compositor, _cx| {
        if let Some(explorer) =
            compositor.find_id::<super::explorer::Explorer>(super::explorer::EXPLORER_ID)
        {
            explorer.apply_motion(motion, count);
        }
    }));
    true
}

impl EditorView {
    pub fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            pseudo_pending: Vec::new(),
            diff_pending: Vec::new(),
            last_insert: (commands::MappableCommand::normal_mode, Vec::new()),
            completion: None,
            completion_origin: None,
            spinners: ProgressSpinners::default(),
            bufferline_info: BufferLineInfo::default(),
            terminal_focused: true,
            prompt_active: false,
            terminal_secondary_cursors: Vec::new(),
            notification_popup: NotificationPopup::new(),
            bufferline_cache: BufferlineCache::default(),
            split_drag: None,
        }
    }

    pub fn spinners_mut(&mut self) -> &mut ProgressSpinners {
        &mut self.spinners
    }

    /// The welcome screen, in the middle of the view, as large as the view allows: the mark
    /// beside the wordmark and the help; or a smaller mark beside the help; or the help alone;
    /// or nothing.
    pub fn render_welcome(theme: &Theme, view: &View, surface: &mut Surface, is_colorful: bool) {
        use crate::ui::logo;

        /// Columns between the mark and what is beside it.
        const GAP_LARGE: u16 = 4;
        const GAP_SMALL: u16 = 6;

        let command = |keys: &'static str, more: Option<&'static str>, label: &'static str| {
            let mut spans = vec![Span::styled(keys, theme.get("markup.raw"))];
            let keys_width = keys.len() + more.map_or(0, str::len);
            if let Some(more) = more {
                spans.push(Span::styled(more, theme.get("comment")));
            }
            spans.push(Span::raw(" ".repeat(20usize.saturating_sub(keys_width))));
            spans.push(Span::raw(label));
            Spans(spans)
        };
        let commands = [
            command(":tutor", Some("<enter>"), "learn helix"),
            command(":theme", Some("<space><tab>"), "choose a theme"),
            command("<space>e", None, "file explorer"),
            command("<space>?", None, "see all commands"),
            command(":quit", Some("<enter>"), "quit helix"),
        ];
        let version = Spans::from(Span::styled(VERSION_AND_GIT_HASH, theme.get("comment")));
        let title = Spans(vec![
            Span::raw("helix "),
            Span::styled(VERSION_AND_GIT_HASH, theme.get("comment")),
        ]);
        let tagline = Spans::from(Span::styled(
            "A post-modern modal text editor",
            theme.get("ui.text").add_modifier(Modifier::ITALIC),
        ));
        let docs = Spans(vec![
            Span::styled("docs: ", theme.get("ui.text")),
            Span::styled("docs.helix-editor.com", theme.get("markup.link.url")),
        ]);
        let width_of =
            |lines: &[&Spans]| lines.iter().map(|line| line.width()).max().unwrap_or(0) as u16;
        let commands_width = width_of(&commands.iter().collect::<Vec<_>>());

        // The help: `(line, centered)`, the commands aligned on the left in the middle.
        let mut help: Vec<(Spans, bool)> = vec![
            (title, true),
            (Spans::default(), false),
            (tagline.clone(), true),
            (Spans::default(), false),
        ];
        help.extend(commands.iter().cloned().map(|line| (line, false)));
        help.push((Spans::default(), false));
        help.push((docs.clone(), true));
        let help_width = width_of(&help.iter().map(|(line, _)| line).collect::<Vec<_>>());

        // Beside the large mark: the wordmark, the version, the tagline and the help, on the
        // left.
        let background = theme.get("ui.background").bg;
        let mut column: Vec<Spans> = logo::wordmark(is_colorful, background);
        column.push(version);
        column.push(tagline.clone());
        column.push(Spans::default());
        column.extend(commands.iter().cloned());
        column.push(Spans::default());
        column.push(docs);
        let column_width = width_of(&column.iter().collect::<Vec<_>>()).max(logo::wordmark_width());

        // Not over the statusline.
        let area = view.area.clip_bottom(1);
        let fits =
            |width: u16, height: usize| width <= area.width && height <= area.height as usize;
        let top = |height: usize| area.y + (area.height.saturating_sub(height as u16)) / 2;
        let left = |width: u16| area.x + (area.width.saturating_sub(width)) / 2;
        let draw = |surface: &mut Surface, x: u16, y: u16, line: &Spans| {
            surface.set_spans(x, y, line, line.width() as u16);
        };

        let large = logo::mark(&logo::MARK_LARGE, is_colorful);
        let large_width = logo::width(&logo::MARK_LARGE);
        let full_width = large_width + GAP_LARGE + column_width;
        if fits(full_width, large.len().max(column.len())) {
            let x = left(full_width);
            let y = top(large.len());
            for (row, line) in large.iter().enumerate() {
                draw(surface, x, y + row as u16, line);
            }
            let column_x = x + large_width + GAP_LARGE;
            let column_y = y + (large.len().saturating_sub(column.len()) / 2) as u16;
            for (row, line) in column.iter().enumerate() {
                draw(surface, column_x, column_y + row as u16, line);
            }
            return;
        }

        // The help, at `x`: its centered lines in the middle of its width, its commands as a
        // block in the middle.
        let draw_help = |surface: &mut Surface, x: u16, y: u16| {
            let commands_x = x + (help_width - commands_width) / 2;
            for (row, (line, centered)) in help.iter().enumerate() {
                let line_x = if *centered {
                    x + (help_width - line.width() as u16) / 2
                } else {
                    commands_x
                };
                draw(surface, line_x, y + row as u16, line);
            }
        };

        let small = logo::mark(&logo::MARK_SMALL, is_colorful);
        let small_width = logo::width(&logo::MARK_SMALL);
        let medium_width = small_width + GAP_SMALL + help_width;
        if fits(medium_width, small.len().max(help.len())) {
            let x = left(medium_width);
            let y = top(small.len().max(help.len()));
            for (row, line) in small.iter().enumerate() {
                draw(surface, x, y + row as u16, line);
            }
            draw_help(surface, x + small_width + GAP_SMALL, y);
            return;
        }

        if fits(help_width, help.len()) {
            draw_help(surface, left(help_width), top(help.len()));
        }
    }

    pub fn render_view(
        &self,
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        is_focused: bool,
    ) {
        let inner = view.inner_area(doc);
        let area = view.area;
        let theme = &editor.theme;
        let config = editor.config();
        let loader = editor.syn_loader.load();

        let view_offset = doc.view_offset(view.id);

        let text_annotations = view.text_annotations(doc, Some(theme));
        let mut decorations = DecorationManager::default();

        if config.breadcrumb.enable {
            Self::render_breadcrumb(editor, doc, view, area.with_height(1), surface);
        }

        if !(is_focused && self.terminal_focused) {
            surface.set_style(area, theme.get("ui.background.inactive"))
        }

        // A side of a diff: its changed lines and words (under the cursor line).
        if let Some(diff) = crate::ui::diff_view::DiffDecoration::new(editor, doc, view) {
            decorations.add_decoration(diff);
        }

        if is_focused && config.cursorline {
            decorations.add_decoration(Self::cursorline(doc, view, theme));
        }

        decorations.add_decoration(FoldDecoration::new(&text_annotations, theme));

        if is_focused && config.cursorcolumn {
            Self::highlight_cursorcolumn(doc, view, surface, theme, inner, &text_annotations);
        }

        // Set DAP highlights, if needed.
        if let Some(frame) = editor.current_stack_frame() {
            let dap_line = frame.line.saturating_sub(1);
            let style = theme.get("ui.highlight.frameline");
            let line_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                if pos.doc_line != dap_line {
                    return;
                }
                renderer.set_style(Rect::new(inner.x, pos.visual_line, inner.width, 1), style);
            };

            decorations.add_decoration(line_decoration);
        }

        let syntax_highlighter = Self::doc_syntax_highlighter(
            doc,
            &text_annotations,
            view_offset.anchor,
            inner.height,
            &loader,
        );
        let mut overlays = Vec::new();

        overlays.push(Self::overlay_syntax_highlights(
            doc,
            view_offset.anchor,
            inner.height,
            &text_annotations,
        ));

        if doc
            .language_config()
            .and_then(|config| config.rainbow_brackets)
            .unwrap_or(config.rainbow_brackets)
        {
            if let Some(overlay) = Self::doc_rainbow_highlights(
                doc,
                &text_annotations,
                view_offset.anchor,
                inner.height,
                theme,
                &loader,
            ) {
                overlays.push(overlay);
            }
        }

        if let Some(overlay) = Self::doc_document_link_highlights(doc, theme) {
            overlays.push(overlay);
        }

        Self::doc_diagnostics_highlights_into(doc, theme, &mut overlays);
        overlays.extend(Self::plugin_highlights(doc, theme));

        if is_focused {
            if config.lsp.auto_document_highlight {
                if let Some(overlay) = Self::doc_document_highlights(doc, view, theme) {
                    overlays.push(overlay);
                }
            }
            if let Some(tabstops) = Self::tabstop_highlights(doc, theme) {
                overlays.push(tabstops);
            }
            overlays.push(self.doc_selection_highlights(
                editor,
                doc,
                view,
                theme,
                self.terminal_focused,
            ));
            if let Some(overlay) = Self::highlight_focused_view_elements(view, doc, theme) {
                overlays.push(overlay);
            }
        }

        let gutter_overflow = view.gutter_offset(doc) == 0;
        if !gutter_overflow {
            Self::render_gutter(
                editor,
                doc,
                view,
                view.area,
                theme,
                is_focused & self.terminal_focused,
                &mut decorations,
            );
        }

        Self::render_inline_blame(&config.inline_blame, doc, view, &mut decorations, theme);
        if config.lsp.enable && config.lsp.display_code_lenses {
            Self::render_code_lenses(doc, view, &mut decorations, theme);
        }

        if config.welcome_screen && doc.version() == 0 && doc.is_welcome {
            Self::render_welcome(
                theme,
                view,
                surface,
                config.true_color || crate::true_color(),
            );
        }

        let primary_cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        if is_focused {
            decorations.add_decoration(text_decorations::Cursor {
                cache: &editor.cursor_cache,
                primary_cursor,
            });
        }
        if is_focused && editor.mode() == Mode::Insert {
            if let Some(completion) = doc.inline_completion(view.id) {
                let text = doc.text().slice(..);
                let line = text.char_to_line(completion.cursor);
                let line_end = helix_core::line_ending::line_end_char_index(&text, line);
                decorations.add_decoration(
                    text_decorations::inline_completion::InlineCompletionDecoration::new(
                        completion,
                        theme
                            .try_get_exact("ui.virtual.inline-completion")
                            .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
                        doc.text_format(inner.width, Some(theme)),
                        line_end,
                    ),
                );
            }
        }
        let width = view.inner_width(doc);
        let config = doc.config.load();
        let enable_cursor_line = view
            .diagnostics_handler
            .show_cursorline_diagnostics(doc, view.id);
        let inline_diagnostic_config = config.inline_diagnostics.prepare(width, enable_cursor_line);
        decorations.add_decoration(InlineDiagnostics::new(
            doc,
            theme,
            primary_cursor,
            inline_diagnostic_config,
            config.end_of_line_diagnostics,
        ));

        decorations.add_decoration(PluginDecoration::new(doc, theme, view.id));

        render_document(
            surface,
            inner,
            doc,
            view_offset,
            &text_annotations,
            syntax_highlighter,
            overlays,
            theme,
            decorations,
        );

        // Draw rulers after document. Skip cells that already have content.
        Self::render_rulers(editor, doc, view, inner, surface, theme);

        // if we're not at the edge of the screen, draw a right border
        if viewport.right() != view.area.right() {
            let x = area.right();
            let border_style = theme.get("ui.window");
            for y in area.top()..area.bottom() {
                surface[(x, y)]
                    .set_symbol(tui::symbols::line::VERTICAL)
                    //.set_symbol(" ")
                    .set_style(border_style);
            }
        }

        // With neither inline diagnostics on the cursor line nor end-of-line diagnostics, the
        // diagnostics under the cursor are shown in the top right corner of the view, as
        // upstream does. Not while a plugin shows a panel in this view (the
        // inline-diagnostic plugin turns the others off to draw its own): they would be shown
        // twice. Annotations don't count: they are lenses, hints, blame... as often as not.
        use helix_view::annotations::diagnostics::DiagnosticFilter;
        if config.inline_diagnostics.cursor_line == DiagnosticFilter::Disable
            && config.end_of_line_diagnostics == DiagnosticFilter::Disable
            && !doc.plugin_panels.contains_key(&view.id)
        {
            Self::render_diagnostics(doc, view, inner, surface, theme);
        }

        // A plugin's panel goes over everything drawn in the view (it takes no rows).
        Self::render_plugin_panel(editor, doc, view, inner, surface, theme);

        // Statusline on the last row of the view area.
        // The cmdline space reservation is handled at the top level in EditorView::render.
        let statusline_area = view.area.clip_top(view.area.height.saturating_sub(1));

        let mut context =
            statusline::RenderContext::new(editor, doc, view, is_focused, &self.spinners);

        statusline::render(&mut context, statusline_area, surface);
    }

    /// The panel a plugin attached to a line (`buffer:set_panel`), drawn over the text like
    /// tiny-inline-diagnostic.nvim: beside the end of its line, or under it when too few
    /// columns are left, wrapping over the lines below. It takes no rows, so no line of the
    /// document moves and scrolling is unaffected; rows past the bottom of the view are cut.
    fn render_plugin_panel(
        editor: &Editor,
        doc: &Document,
        view: &View,
        inner: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        use helix_view::annotations::panel::{layout, PanelPart};
        use helix_view::graphics::Color;

        let Some(panel) = doc.plugin_panels.get(&view.id) else {
            return;
        };
        if editor.mode() == Mode::Insert && !panel.show_in_insert {
            return;
        }
        let text = doc.text().slice(..);
        if panel.char_idx > text.len_chars() {
            return;
        }
        let line = text.char_to_line(panel.char_idx);
        let line_end = helix_core::line_ending::line_end_char_index(&text, line);
        // The last row of the line (soft wrap), and where its text ends.
        let Some(mut end) = view.screen_coords_at_pos(doc, text, line_end) else {
            return;
        };
        // After what plugins draw after the end of the line on its row (search lenses...),
        // laid out as they are drawn: the panel must not cover them.
        if let Some(annots) = doc.plugin_annotations.get(&view.id) {
            use helix_view::annotations::plugins::{eol_layout, plugin_annotations_on_line};
            let line_annots = plugin_annotations_on_line(annots, text, line);
            let scrolled = doc.view_offset(view.id).horizontal_offset;
            // the column after the line's end, where they start (as the renderer has it)
            let after_end = end.col + scrolled + 1;
            let eol_end = eol_layout(line_annots, after_end, inner.width, scrolled).eol_end;
            if eol_end > after_end {
                end.col = eol_end - 1 - scrolled;
            }
        }

        let hex = |color: &Option<String>| {
            color
                .as_deref()
                .and_then(|color| Color::from_hex(color).ok())
        };
        let mut body = theme.get("ui.popup");
        if let Some(fg) = hex(&panel.fg) {
            body.fg = Some(fg);
        }
        if let Some(bg) = hex(&panel.bg) {
            body.bg = Some(bg);
        }
        // Caps and arrows: in the panel's color, on whatever is under them.
        let edge = Style::default().fg(hex(&panel.arrow_fg).or(body.bg).unwrap_or(Color::Reset));
        let cap = Style::default().fg(body.bg.unwrap_or(Color::Reset));

        for row in layout(panel, Some(end.col), inner.width as usize) {
            let Some(y) = u16::try_from(end.row + row.row)
                .ok()
                .and_then(|row| inner.y.checked_add(row))
                .filter(|&y| y < inner.bottom())
            else {
                break;
            };
            for span in &row.spans {
                let (style, keep_bg) = match span.part {
                    PanelPart::Arrow => (edge, true),
                    PanelPart::Cap => (cap, true),
                    PanelPart::Icon(idx) => {
                        let mut style = body;
                        if let Some(fg) = hex(&panel.items[idx].icon_fg) {
                            style.fg = Some(fg);
                        }
                        (style, false)
                    }
                    PanelPart::Text => (body, false),
                };
                let Some(x) = u16::try_from(span.col)
                    .ok()
                    .and_then(|col| inner.x.checked_add(col))
                    .filter(|&x| x < inner.right())
                else {
                    continue;
                };
                let width = (inner.right() - x) as usize;
                // Whatever the text below had (its underlines, say) must not show through.
                let span_width = span.text.width().min(width) as u16;
                for cx in x..x + span_width {
                    let cell = &mut surface[(cx, y)];
                    let bg = cell.bg;
                    cell.reset();
                    if keep_bg {
                        cell.bg = bg;
                    }
                }
                surface.set_stringn(x, y, &span.text, width, style);
            }
        }
    }

    /// Labels of the code lenses on the visible lines, after the end of their line.
    fn render_code_lenses(
        doc: &Document,
        view: &View,
        decorations: &mut DecorationManager,
        theme: &Theme,
    ) {
        if doc.code_lenses.is_empty() {
            return;
        }
        let text = doc.text();
        let visible = view.line_range(doc);
        let mut titles: std::collections::HashMap<usize, Vec<&str>> =
            std::collections::HashMap::new();
        for lens in &doc.code_lenses {
            let line = text.char_to_line(lens.char_idx.min(text.len_chars()));
            if !visible.contains(&line) {
                continue;
            }
            // lenses without a command are waiting for `codeLens/resolve`
            if let Some(command) = &lens.lens.command {
                titles.entry(line).or_default().push(&command.title);
            }
        }
        let labels = titles
            .into_iter()
            .filter_map(|(line, titles)| {
                text_decorations::code_lenses::label(titles).map(|label| (line, label))
            })
            .collect::<std::collections::HashMap<_, _>>();
        if labels.is_empty() {
            return;
        }
        let style = theme
            .try_get("ui.virtual.code-lens")
            .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint"));
        decorations.add_decoration(text_decorations::code_lenses::CodeLensDecoration::new(
            labels, style,
        ));
    }

    fn render_inline_blame(
        inline_blame: &InlineBlameConfig,
        doc: &Document,
        view: &View,
        decorations: &mut DecorationManager,
        theme: &Theme,
    ) {
        const INLINE_BLAME_SCOPE: &str = "ui.virtual.inline-blame";
        let text = doc.text();
        match inline_blame.show {
            InlineBlameShow::Never => (),
            InlineBlameShow::CursorLine => {
                let cursor_line_idx = doc.cursor_line(view.id);

                // do not render inline blame for empty lines to reduce visual noise
                if text.line(cursor_line_idx) != doc.line_ending.as_str() {
                    if let Ok(line_blame) =
                        doc.line_blame(cursor_line_idx as u32, &inline_blame.format)
                    {
                        decorations.add_decoration(InlineBlame::new(
                            theme.get(INLINE_BLAME_SCOPE),
                            text_decorations::blame::LineBlame::OneLine((
                                cursor_line_idx,
                                line_blame,
                            )),
                        ));
                    };
                }
            }
            InlineBlameShow::AllLines => {
                // Only the lines on screen: sizing this by the whole file would allocate per
                // line of the document on every frame. Folded lines aren't on screen either:
                // the lines are gathered in runs of consecutive lines, one decoration each.
                let visible = view.line_range(doc);
                let folds = view.fold_annotations(doc);
                let mut runs: Vec<(usize, Vec<Option<String>>)> = Vec::new();
                let mut line_idx = visible.start;
                while line_idx < visible.end {
                    if let Some(fold) = folds
                        .superest_fold_containing(line_idx, |fold| fold.start.line..=fold.end.line)
                    {
                        line_idx = fold.end.line + 1;
                        continue;
                    }
                    // do not render inline blame for empty lines to reduce visual noise
                    let blame = (text.line(line_idx) != doc.line_ending.as_str())
                        .then(|| doc.line_blame(line_idx as u32, &inline_blame.format).ok())
                        .flatten();
                    match runs.last_mut() {
                        Some((first_line, lines)) if *first_line + lines.len() == line_idx => {
                            lines.push(blame)
                        }
                        _ => runs.push((line_idx, vec![blame])),
                    }
                    line_idx += 1;
                }

                for (first_line, lines) in runs {
                    decorations.add_decoration(InlineBlame::new(
                        theme.get(INLINE_BLAME_SCOPE),
                        text_decorations::blame::LineBlame::ManyLines { first_line, lines },
                    ));
                }
            }
        }
    }

    pub fn render_rulers(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        let editor_rulers = &editor.config().rulers;
        let ruler_char = &editor.config().ruler_char;
        // Base style from theme for rulers
        let base_style = theme.try_get("ui.virtual.ruler").unwrap_or_default();
        // Background style is used only for background-style rulers. If theme lacks a bg, reuse fg.
        let bg_style = if base_style.bg.is_none() {
            if let Some(fg) = base_style.fg {
                base_style.bg(fg)
            } else {
                // Fallback background to ensure visibility
                Style::default().bg(Color::Red)
            }
        } else {
            base_style
        };

        let rulers = doc
            .language_config()
            .and_then(|config| config.rulers.as_ref())
            .unwrap_or(editor_rulers);

        let view_offset = doc.view_offset(view.id);

        rulers
            .iter()
            // View might be horizontally scrolled, convert from absolute distance
            // from the 1st column to relative distance from left of viewport
            .filter_map(|ruler| ruler.checked_sub(1 + view_offset.horizontal_offset as u16))
            .filter(|ruler| ruler < &viewport.width)
            .map(|ruler| viewport.clip_left(ruler).with_width(1))
            .for_each(|area| {
                if ruler_char.is_empty() {
                    // Background-style ruler: only apply to cells without content
                    for y in area.top()..area.bottom() {
                        let cell = &surface[(area.x, y)];
                        // Skip cells that have non-whitespace content (like diagnostic bubbles)
                        if cell.symbol.as_str() == " " || cell.symbol.as_str().is_empty() {
                            surface[(area.x, y)].set_style(bg_style);
                        }
                    }
                } else {
                    // Foreground glyph ruler: only draw on empty/space cells
                    let mut glyph_style = base_style;
                    glyph_style.bg = None;
                    if glyph_style.fg.is_none() {
                        glyph_style = glyph_style.fg(Color::Gray);
                    }
                    for y in area.top()..area.bottom() {
                        let cell = &surface[(area.x, y)];
                        // Only draw ruler glyph on empty/space cells to avoid overwriting content
                        if cell.symbol.as_str() == " " || cell.symbol.as_str().is_empty() {
                            surface[(area.x, y)]
                                .set_symbol(ruler_char)
                                .set_style(glyph_style);
                        }
                    }
                }
            })
    }

    fn viewport_byte_range(
        text: helix_core::RopeSlice,
        annotations: &TextAnnotations,
        row: usize,
        height: u16,
    ) -> std::ops::Range<usize> {
        // Calculate viewport byte ranges:
        // Saturating subs to make it inclusive zero indexing.
        let last_line = text.len_lines().saturating_sub(1);
        let last_visible_line = text
            .nth_next_folded_line(&annotations.folds, row, (height as usize).saturating_sub(1))
            .min(last_line);
        let start = text.line_to_byte(row.min(last_line));
        let end = text.line_to_byte(last_visible_line + 1);

        start..end
    }

    /// Get the syntax highlighter for a document in a view represented by the first line
    /// and column (`offset`) and the last line. This is done instead of using a view
    /// directly to enable rendering syntax highlighted docs anywhere (eg. picker preview)
    pub fn doc_syntax_highlighter<'editor>(
        doc: &'editor Document,
        annotations: &TextAnnotations,
        anchor: usize,
        height: u16,
        loader: &'editor syntax::Loader,
    ) -> Option<syntax::Highlighter<'editor>> {
        let syntax = doc.syntax()?;
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));
        let range = Self::viewport_byte_range(text, annotations, row, height);
        let range = range.start as u32..range.end as u32;

        let highlighter = syntax.highlighter(text, loader, range);
        Some(highlighter)
    }

    pub fn overlay_syntax_highlights(
        doc: &Document,
        anchor: usize,
        height: u16,
        text_annotations: &TextAnnotations,
    ) -> OverlayHighlights {
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));

        let mut range = Self::viewport_byte_range(text, text_annotations, row, height);
        range = text.byte_to_char(range.start)..text.byte_to_char(range.end);

        text_annotations.collect_overlay_highlights(range)
    }

    pub fn doc_rainbow_highlights(
        doc: &Document,
        annotations: &TextAnnotations,
        anchor: usize,
        height: u16,
        theme: &Theme,
        loader: &syntax::Loader,
    ) -> Option<OverlayHighlights> {
        let syntax = doc.syntax()?;
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));
        let visible_range = Self::viewport_byte_range(text, annotations, row, height);
        let start = syntax::child_for_byte_range(
            &syntax.tree().root_node(),
            visible_range.start as u32..visible_range.end as u32,
        )
        .map_or(visible_range.start as u32, |node| node.start_byte());
        let range = start..visible_range.end as u32;

        Some(syntax.rainbow_highlights(text, theme.rainbow_length(), loader, range))
    }

    /// Get highlight spans for document diagnostics
    /// The ranges plugins highlight (`buffer:set_highlights`), one overlay per theme key (a
    /// key the theme does not have is not drawn), clamped to the text.
    fn plugin_highlights(
        doc: &Document,
        theme: &Theme,
    ) -> impl Iterator<Item = OverlayHighlights> + 'static {
        let len = doc.text().len_chars();
        let mut by_scope: Vec<(helix_core::syntax::Highlight, Vec<std::ops::Range<usize>>)> =
            Vec::new();
        for highlight in &doc.plugin_highlights {
            let Some(scope) = theme.find_highlight(&highlight.scope) else {
                continue;
            };
            let range = highlight.from.min(len)..highlight.to.min(len);
            if range.is_empty() {
                continue;
            }
            match by_scope.iter_mut().find(|(h, _)| *h == scope) {
                Some((_, ranges)) => ranges.push(range),
                None => by_scope.push((scope, vec![range])),
            }
        }
        by_scope.into_iter().map(|(highlight, mut ranges)| {
            // Ranges of one overlay must not overlap: merge them.
            ranges.sort_by_key(|r| (r.start, r.end));
            let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
            for range in ranges {
                match merged.last_mut() {
                    Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                    _ => merged.push(range),
                }
            }
            OverlayHighlights::Homogeneous {
                highlight,
                ranges: merged,
            }
        })
    }

    pub fn doc_diagnostics_highlights_into(
        doc: &Document,
        theme: &Theme,
        overlay_highlights: &mut Vec<OverlayHighlights>,
    ) {
        // Skip redundant work if no diagnostics.
        if doc.diagnostics().is_empty() {
            return;
        }

        use helix_core::diagnostic::{DiagnosticTag, Range, Severity};
        let get_scope_of = |scope| {
            theme
                .find_highlight_exact(scope)
                // get one of the themes below as fallback values
                .or_else(|| theme.find_highlight_exact("diagnostic"))
                .or_else(|| theme.find_highlight_exact("ui.cursor"))
                .or_else(|| theme.find_highlight_exact("ui.selection"))
                .expect(
                    "at least one of the following scopes must be defined in the theme: `diagnostic`, `ui.cursor`, or `ui.selection`",
                )
        };

        // Diagnostic tags
        let unnecessary = theme.find_highlight_exact("diagnostic.unnecessary");
        let deprecated = theme.find_highlight_exact("diagnostic.deprecated");

        let mut default_vec = Vec::new();
        let mut info_vec = Vec::new();
        let mut hint_vec = Vec::new();
        let mut warning_vec = Vec::new();
        let mut error_vec = Vec::new();
        let mut unnecessary_vec = Vec::new();
        let mut deprecated_vec = Vec::new();

        let push_diagnostic = |vec: &mut Vec<ops::Range<usize>>, range: Range| {
            // If any diagnostic overlaps ranges with the prior diagnostic,
            // merge the two together. Otherwise push a new span.
            match vec.last_mut() {
                Some(existing_range) if range.start <= existing_range.end => {
                    // This branch merges overlapping diagnostics, assuming that the current
                    // diagnostic starts on range.start or later. If this assertion fails,
                    // we will discard some part of `diagnostic`. This implies that
                    // `doc.diagnostics()` is not sorted by `diagnostic.range`.
                    debug_assert!(existing_range.start <= range.start);
                    existing_range.end = range.end.max(existing_range.end)
                }
                _ => vec.push(range.start..range.end),
            }
        };

        for diagnostic in doc.diagnostics() {
            // Separate diagnostics into different Vecs by severity.
            let vec = match diagnostic.severity {
                Some(Severity::Info) => &mut info_vec,
                Some(Severity::Hint) => &mut hint_vec,
                Some(Severity::Warning) => &mut warning_vec,
                Some(Severity::Error) => &mut error_vec,
                _ => &mut default_vec,
            };

            // If the diagnostic has tags and a non-warning/error severity, skip rendering
            // the diagnostic as info/hint/default and only render it as unnecessary/deprecated
            // instead. For warning/error diagnostics, render both the severity highlight and
            // the tag highlight.
            if diagnostic.tags.is_empty()
                || matches!(
                    diagnostic.severity,
                    Some(Severity::Warning | Severity::Error)
                )
            {
                push_diagnostic(vec, diagnostic.range);
            }

            for tag in &diagnostic.tags {
                match tag {
                    DiagnosticTag::Unnecessary => {
                        if unnecessary.is_some() {
                            push_diagnostic(&mut unnecessary_vec, diagnostic.range)
                        }
                    }
                    DiagnosticTag::Deprecated => {
                        if deprecated.is_some() {
                            push_diagnostic(&mut deprecated_vec, diagnostic.range)
                        }
                    }
                }
            }
        }

        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic"),
            ranges: default_vec,
        });
        if let Some(highlight) = unnecessary {
            overlay_highlights.push(OverlayHighlights::Homogeneous {
                highlight,
                ranges: unnecessary_vec,
            });
        }
        if let Some(highlight) = deprecated {
            overlay_highlights.push(OverlayHighlights::Homogeneous {
                highlight,
                ranges: deprecated_vec,
            });
        }
        overlay_highlights.extend([
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.info"),
                ranges: info_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.hint"),
                ranges: hint_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.warning"),
                ranges: warning_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.error"),
                ranges: error_vec,
            },
        ]);
    }

    pub fn doc_document_highlights(
        doc: &Document,
        view: &View,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        let ranges = doc.document_highlights(view.id)?;
        if ranges.is_empty() {
            return None;
        }

        let highlight = theme
            .find_highlight_exact("ui.highlight")
            .or_else(|| theme.find_highlight_exact("ui.selection"))
            .or_else(|| theme.find_highlight_exact("ui.cursor"))?;

        Some(OverlayHighlights::Homogeneous {
            highlight,
            ranges: ranges.to_vec(),
        })
    }

    pub fn doc_document_link_highlights(
        doc: &Document,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        let highlight = theme
            .find_highlight_exact("markup.link.url")
            .or_else(|| theme.find_highlight_exact("markup.link"))?;

        if doc.document_links.is_empty() {
            return None;
        }

        let mut ranges: Vec<ops::Range<usize>> = Vec::new();
        for link in &doc.document_links {
            if link.start >= link.end {
                continue;
            }

            match ranges.last_mut() {
                Some(existing_range) if link.start <= existing_range.end => {
                    existing_range.end = existing_range.end.max(link.end);
                }
                _ => ranges.push(link.start..link.end),
            }
        }

        if ranges.is_empty() {
            return None;
        }

        Some(OverlayHighlights::Homogeneous { highlight, ranges })
    }

    /// Get highlight spans for selections in a document view.
    fn doc_selection_highlights(
        &self,
        editor: &Editor,
        doc: &Document,
        view: &View,
        theme: &Theme,
        is_terminal_focused: bool,
    ) -> OverlayHighlights {
        let mode = editor.mode();
        let cursor_shape_config = &editor.config().cursor_shape;
        let terminal_draws_secondary_cursors = self.terminal_draws_secondary_cursors(editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);
        let primary_idx = selection.primary_index();

        let cursorkind = cursor_shape_config.from_mode(mode);
        let cursor_is_block = cursorkind == CursorKind::Block;

        let selection_scope = theme
            .find_highlight_exact("ui.selection")
            .expect("could not find `ui.selection` scope in the theme!");
        let primary_selection_scope = theme
            .find_highlight_exact("ui.selection.primary")
            .unwrap_or(selection_scope);

        let base_cursor_scope = theme
            .find_highlight_exact("ui.cursor")
            .unwrap_or(selection_scope);
        let base_primary_cursor_scope = theme
            .find_highlight("ui.cursor.primary")
            .unwrap_or(base_cursor_scope);

        let cursor_scope = match mode {
            Mode::Insert => theme.find_highlight_exact("ui.cursor.insert"),
            Mode::Select => theme.find_highlight_exact("ui.cursor.select"),
            Mode::Normal => theme.find_highlight_exact("ui.cursor.normal"),
        }
        .unwrap_or(base_cursor_scope);

        let primary_cursor_scope = match mode {
            Mode::Insert => theme.find_highlight_exact("ui.cursor.primary.insert"),
            Mode::Select => theme.find_highlight_exact("ui.cursor.primary.select"),
            Mode::Normal => theme.find_highlight_exact("ui.cursor.primary.normal"),
        }
        .unwrap_or(base_primary_cursor_scope);

        let mut spans = Vec::new();
        for (i, range) in selection.iter().enumerate() {
            let selection_is_primary = i == primary_idx;
            // Whether this cursor is drawn here rather than by the terminal: the primary one
            // when the terminal cursor is elsewhere, secondary ones unless the terminal draws
            // them (in the main cursor's shape, so their selections look like the primary's).
            let (draw_cursor, terminal_shaped) = if selection_is_primary {
                (!is_terminal_focused || self.prompt_active, true)
            } else {
                (
                    !terminal_draws_secondary_cursors,
                    terminal_draws_secondary_cursors,
                )
            };
            let (cursor_scope, selection_scope) = if selection_is_primary {
                (primary_cursor_scope, primary_selection_scope)
            } else {
                (cursor_scope, selection_scope)
            };

            // Special-case: cursor at end of the rope.
            if range.head == range.anchor && range.head == text.len_chars() {
                if draw_cursor {
                    // Primary cursor is drawn by the terminal when focused and no prompt is active
                    // Secondary cursors, unfocused primary cursor, and editor cursor when prompt is active are drawn manually
                    spans.push((cursor_scope, range.head..range.head + 1));
                }
                continue;
            }

            let range = range.min_width_1(text);
            if range.head > range.anchor {
                // Standard case.
                let cursor_start = prev_grapheme_boundary(text, range.head);
                // non block cursors look like they exclude the cursor
                let selection_end = if terminal_shaped && !cursor_is_block && mode != Mode::Insert {
                    range.head
                } else {
                    cursor_start
                };
                spans.push((selection_scope, range.anchor..selection_end));
                if draw_cursor {
                    spans.push((cursor_scope, cursor_start..range.head));
                }
            } else {
                // Reverse case.
                let cursor_end = next_grapheme_boundary(text, range.head);
                if draw_cursor {
                    spans.push((cursor_scope, range.head..cursor_end));
                }
                // non block cursors look like they exclude the cursor
                let selection_start = if terminal_shaped
                    && !cursor_is_block
                    && !(mode == Mode::Insert && cursor_end == range.anchor)
                {
                    range.head
                } else {
                    cursor_end
                };
                spans.push((selection_scope, selection_start..range.anchor));
            }
        }

        OverlayHighlights::Heterogenous { highlights: spans }
    }

    /// Whether the terminal draws the focused view's secondary cursors (kitty's multiple cursors
    /// protocol) rather than the editor. Like the primary cursor: while the terminal is focused
    /// and its cursor is in the editor (no prompt above it). Block cursors are always drawn by
    /// the editor, which colors them with the theme.
    fn terminal_draws_secondary_cursors(&self, editor: &Editor) -> bool {
        let kind = editor.config().cursor_shape.from_mode(editor.mode());
        editor.kitty_multi_cursor_support
            && self.terminal_focused
            && !self.prompt_active
            && !matches!(kind, CursorKind::Block | CursorKind::Hidden)
    }

    /// The screen cells of the focused view's secondary cursors the terminal draws, with the
    /// cells as drawn by the views.
    fn secondary_cursor_cells(&self, editor: &Editor, surface: &Surface) -> Vec<(u16, u16, Cell)> {
        if !self.terminal_draws_secondary_cursors(editor) {
            return Vec::new();
        }
        let (view, doc) = current_ref!(editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);
        let inner = view.inner_area(doc);
        selection
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != selection.primary_index())
            .filter_map(|(_, range)| {
                let pos = view.screen_coords_at_pos(doc, text, range.cursor(text))?;
                let x = inner.x.checked_add(u16::try_from(pos.col).ok()?)?;
                let y = inner.y.checked_add(u16::try_from(pos.row).ok()?)?;
                if x >= inner.right() || y >= inner.bottom() {
                    return None;
                }
                let cell = surface.get(x, y)?.clone();
                Some((x, y, cell))
            })
            .collect()
    }

    /// The secondary cursors the terminal should draw after the whole screen was drawn in
    /// `surface`: those whose cell is still as the views drew it, as anything drawn over them
    /// since (the completion menu, a popup, a picker, ...) would otherwise have a cursor on top.
    pub fn terminal_secondary_cursors(&self, surface: &Surface) -> Vec<(u16, u16)> {
        self.terminal_secondary_cursors
            .iter()
            .filter(|(x, y, cell)| surface.get(*x, *y) == Some(cell))
            .map(|&(x, y, _)| (x, y))
            .collect()
    }

    /// Render brace match, etc (meant for the focused view only)
    pub fn highlight_focused_view_elements(
        view: &View,
        doc: &Document,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        // Highlight matching braces
        let syntax = doc.syntax()?;
        let highlight = theme.find_highlight_exact("ui.cursor.match")?;
        let text = doc.text().slice(..);
        let pos = doc.selection(view.id).primary().cursor(text);
        let pos = helix_core::match_brackets::find_matching_bracket(syntax, text, pos)?;
        Some(OverlayHighlights::single(highlight, pos..pos + 1))
    }

    pub fn tabstop_highlights(doc: &Document, theme: &Theme) -> Option<OverlayHighlights> {
        let snippet = doc.active_snippet.as_ref()?;
        let highlight = theme.find_highlight_exact("tabstop")?;
        let mut ranges = Vec::new();
        for tabstop in snippet.tabstops() {
            ranges.extend(tabstop.ranges.iter().map(|range| range.start..range.end));
        }
        Some(OverlayHighlights::Homogeneous { highlight, ranges })
    }

    /// Render bufferline at the top
    pub fn render_bufferline(&mut self, editor: &Editor, viewport: Rect, surface: &mut Surface) {
        surface.clear_with(
            viewport,
            editor
                .theme
                .try_get("ui.bufferline.background")
                .unwrap_or_else(|| editor.theme.get("ui.statusline")),
        );

        let bufferline_active = editor
            .theme
            .try_get("ui.bufferline.active")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.active"));

        let bufferline_inactive = editor
            .theme
            .try_get("ui.bufferline")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.inactive"));

        let current_doc = view!(editor).doc;

        self.bufferline_info.clear();

        // Rebuild cache if document set changed
        self.refresh_bufferline_cache(editor);

        // Use cached values directly. All bufferline coordinates are display
        // columns relative to the start of the (virtual, unscrolled) line,
        // tracked as `usize` so thousands of buffers can't overflow.
        let total_width = self.bufferline_cache.total_width;
        let buffer_texts = &self.bufferline_cache.texts;
        let buffer_widths = &self.bufferline_cache.widths;
        let positions = &self.bufferline_cache.positions;

        let view_width = viewport.width as usize;

        // Determine scroll offset
        let scroll_offset = editor
            .documents()
            .position(|d| d.id() == current_doc)
            .and_then(|current_idx| positions.get(current_idx))
            .map_or(0, |&target_x| {
                if target_x >= view_width / 2 {
                    (target_x - view_width / 2).min(total_width.saturating_sub(view_width))
                } else {
                    0
                }
            });

        let config = editor.config();
        let sep = config.bufferline.separator.as_str();
        let sep_width = sep.width();

        // Second pass: render with the calculated offset
        for (idx, doc) in editor.documents().enumerate() {
            let buffer_x = positions[idx];
            let text = &buffer_texts[idx];

            // Render separator if not first document (only when fully visible
            // on the left edge, so it is never drawn over another label).
            if idx > 0 {
                let sep_start = buffer_x.saturating_sub(sep_width);
                if sep_start >= scroll_offset {
                    let sep_x = sep_start - scroll_offset;
                    if sep_x < view_width {
                        surface.set_stringn(
                            viewport.x + sep_x as u16,
                            viewport.y,
                            sep,
                            view_width - sep_x,
                            bufferline_inactive,
                        );
                    }
                }
            }

            // Stop at buffers that start past the visible area
            if buffer_x >= scroll_offset + view_width {
                break;
            }

            // Skip buffers that end before the visible area
            if buffer_x + buffer_widths[idx] <= scroll_offset {
                continue;
            }

            let style = if current_doc == doc.id() {
                bufferline_active
            } else {
                bufferline_inactive
            };

            // Clip text (by display columns) if it starts before the visible area
            let (visible_text, rel_x) = if buffer_x < scroll_offset {
                skip_display_columns(text, scroll_offset - buffer_x)
            } else {
                (
                    std::borrow::Cow::Borrowed(text.as_str()),
                    buffer_x - scroll_offset,
                )
            };
            if rel_x >= view_width {
                continue;
            }

            let start_x = viewport.x + rel_x as u16;
            let (end_x, _) = surface.set_stringn(
                start_x,
                viewport.y,
                &visible_text,
                view_width - rel_x,
                style,
            );

            // Track the columns actually drawn for mouse clicks
            self.bufferline_info
                .add_buffer_info(doc.id(), start_x..end_x.max(start_x));
        }
    }

    /// Whether the cached bufferline still matches the documents, compared in place: this runs
    /// every frame, so it must not allocate.
    fn bufferline_cache_is_current(&self, editor: &Editor, separator: &str) -> bool {
        let cache = &self.bufferline_cache;
        cache.separator == separator
            && cache.doc_ids.len() == editor.documents.len()
            && editor
                .documents()
                .zip(&cache.doc_ids)
                .zip(&cache.doc_paths)
                .zip(&cache.modified)
                .zip(&cache.languages)
                .all(|((((doc, id), path), modified), language)| {
                    doc.id() == *id
                        && doc.is_modified() == *modified
                        && doc.language_name() == language.as_deref()
                        && doc
                            .path()
                            .map_or(std::borrow::Cow::Borrowed(SCRATCH_BUFFER_NAME), |path| {
                                path.to_string_lossy()
                            })
                            == path.as_str()
                })
    }

    fn refresh_bufferline_cache(&mut self, editor: &Editor) {
        let config = editor.config();
        let separator = config.bufferline.separator.as_str();
        if self.bufferline_cache_is_current(editor, separator) {
            return;
        }

        let scratch = PathBuf::from(SCRATCH_BUFFER_NAME);

        let current_ids: Vec<DocumentId> = editor.documents().map(|d| d.id()).collect();
        let current_modified: Vec<bool> = editor.documents().map(|d| d.is_modified()).collect();
        let current_paths: Vec<String> = editor
            .documents()
            .map(|d| {
                // Lossy conversion so non-UTF-8 paths keep distinct names
                // instead of all collapsing to "".
                d.path().unwrap_or(&scratch).to_string_lossy().into_owned()
            })
            .collect();

        let languages: Vec<Option<String>> = editor
            .documents()
            .map(|d| d.language_name().map(String::from))
            .collect();

        let icons = ICONS.load();

        // Recompute names (O(n²)) when document set OR paths change
        let names: Vec<(DocumentId, String)> = if self.bufferline_cache.doc_ids == current_ids
            && self.bufferline_cache.doc_paths == current_paths
        {
            // Only modified status changed — reuse existing names
            self.bufferline_cache.names.clone()
        } else {
            // Doc set or paths changed — recompute names
            let components: Vec<Vec<String>> = current_paths
                .iter()
                .map(|p| p.split(MAIN_SEPARATOR).map(String::from).collect())
                .collect();
            editor
                .documents()
                .enumerate()
                .map(|(idx, doc)| {
                    let name = Self::compute_display_name(&components, idx);
                    (doc.id(), name)
                })
                .collect()
        };

        // Build texts and widths (always recompute when anything changed)
        let mut texts = Vec::new();
        let mut widths = Vec::new();
        let mut positions = Vec::new();
        let mut total_width = 0usize;
        // Widths are measured in display columns (not bytes): the default
        // separator "│" and file icons are multi-byte but one column wide.
        let sep_width = separator.width();

        for (idx, doc) in editor.documents().enumerate() {
            if idx > 0 {
                total_width = total_width.saturating_add(sep_width);
            }

            let fname = &names[idx].1;
            let text = if let Some(icon) = icons.mime().get(doc.path(), doc.language_name()) {
                format!(
                    " {}  {} {}",
                    icon.glyph(),
                    fname,
                    if doc.is_modified() { "[+] " } else { "" }
                )
            } else {
                format!(" {} {}", fname, if doc.is_modified() { "[+] " } else { "" })
            };

            positions.push(total_width);
            let text_width = text.width();
            texts.push(text);
            widths.push(text_width);
            total_width = total_width.saturating_add(text_width);
        }

        self.bufferline_cache = BufferlineCache {
            doc_ids: current_ids,
            doc_paths: current_paths,
            names,
            modified: current_modified,
            languages,
            separator: separator.to_owned(),
            texts,
            widths,
            positions,
            total_width,
        };
    }

    /// Compute a unique display name for the document at `doc_index` in the component list.
    /// This is the O(n²) core of `make_document_name` but called only when documents change.
    ///
    /// Uses the shortest path suffix that no other document shares. Documents whose full
    /// paths are identical (e.g. several scratch buffers) can't be told apart by any
    /// suffix, so they get an ordinal (`[scratch] (2)`) instead.
    fn compute_display_name(components: &[Vec<String>], doc_index: usize) -> String {
        let doc_components = &components[doc_index];
        let doc_components_len = doc_components.len();
        // Past the longest path every suffix is the whole path, so nothing more can change.
        let max_len = components.iter().map(Vec::len).max().unwrap_or(0).max(1);

        for k in 1..=max_len {
            let start = doc_components_len.saturating_sub(k);
            let curr_doc = &doc_components[start..];

            let clash = components
                .iter()
                .enumerate()
                .filter(|&(index, _)| index != doc_index)
                .any(|(_, parts)| parts[parts.len().saturating_sub(k)..] == *curr_doc);

            if !clash {
                return curr_doc.join(MAIN_SEPARATOR_STR);
            }
        }

        // Identical to at least one other document: number the duplicates in order.
        let full = doc_components.join(MAIN_SEPARATOR_STR);
        let ordinal = components[..doc_index]
            .iter()
            .filter(|parts| *parts == doc_components)
            .count();
        if ordinal == 0 {
            full
        } else {
            format!("{full} ({})", ordinal + 1)
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn render_breadcrumb(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
    ) {
        use helix_view::editor::BreadcrumbPathOptions::{File, Full};

        #[inline]
        #[must_use]
        fn draw_element(
            surface: &mut Surface,
            viewport: Rect,
            x: u16,
            content: &str,
            style: Style,
        ) -> u16 {
            let remaining = viewport.right().saturating_sub(x) as usize;
            surface
                .set_stringn(x, viewport.y, content, remaining, style)
                .0
        }

        let config = editor.config();

        let style = editor
            .theme
            .try_get_exact("ui.breadcrumb")
            .unwrap_or_else(|| editor.theme.get("ui.text"));

        surface.clear_with(viewport, style);

        let mut x = viewport.x.saturating_add(1);

        let separator = " > ";
        let separator_style = editor.theme.get("ui.breadcrumb.separator");
        let ellipsis_style = editor.theme.get("ui.breadcrumb.separator");
        let mut draw_separator = false;

        if matches!(config.breadcrumb.path, Full | File) {
            if let Some(path) = doc.relative_path() {
                let mut components = path.components().peekable();

                if matches!(config.breadcrumb.path, File) {
                    // Skip until the filename component.
                    //
                    // NOTE:
                    // We could use `next_back` to get this element directly, but
                    // this keeps our logic below simpler, with the logic for `Full`
                    // and `File` being the same below this point.
                    //
                    // PERF: The `clone` is cheap; should be equivalent to a `Copy`.
                    while components.clone().nth(1).is_some() {
                        components.next();
                    }
                }

                while let Some(component) = components.next() {
                    if draw_separator {
                        x = draw_element(surface, viewport, x, separator, separator_style);
                    } else {
                        draw_separator = true;
                    }

                    let segment = component.as_os_str().to_string_lossy();
                    let is_directory = components.peek().is_some();

                    let style = if is_directory {
                        editor.theme.get("ui.text.directory")
                    } else {
                        style
                    };

                    x = draw_element(surface, viewport, x, &segment, style);
                }
            } else {
                // Handle `[scratch]`
                x = draw_element(surface, viewport, x, SCRATCH_BUFFER_NAME, style);
            }
        }

        let icons = ICONS.load();
        let max_name_length = config.breadcrumb.max_name_length;

        // Draw symbols, if any.
        if let Some(breadcrumb) = doc.breadcrumbs.get(&view.id) {
            if breadcrumb.elided() {
                if draw_separator {
                    x = draw_element(surface, viewport, x, separator, separator_style);
                } else {
                    draw_separator = true;
                }
                x = draw_element(surface, viewport, x, "…", ellipsis_style);
            }

            for symbol in breadcrumb.iter() {
                if draw_separator {
                    x = draw_element(surface, viewport, x, separator, separator_style);
                } else {
                    draw_separator = true;
                }

                let kind_name = crate::commands::lsp::display_symbol_kind(symbol.kind);

                if let Some(icon) = icons.kind().get(kind_name) {
                    let icon_style = icon
                        .color()
                        .map(|color| Style::default().fg(color))
                        .unwrap_or_default();
                    x = draw_element(surface, viewport, x, icon.glyph(), icon_style);
                    x = draw_element(surface, viewport, x, " ", Style::default());
                }

                let style = match symbol.kind {
                    SymbolKind::MODULE
                    | SymbolKind::NAMESPACE
                    | SymbolKind::PACKAGE => editor.theme.get("namespace"),

                    SymbolKind::OBJECT // impl Block
                    | SymbolKind::STRUCT
                    | SymbolKind::INTERFACE
                    | SymbolKind::CLASS => editor.theme.get("type"),

                    SymbolKind::METHOD => editor.theme.get("function.method"),
                    SymbolKind::FUNCTION => editor.theme.get("function"),

                    SymbolKind::ENUM => editor.theme.get("type.enum"),
                    SymbolKind::ENUM_MEMBER => editor.theme.get("type.enum.variant"),

                    SymbolKind::FIELD | SymbolKind::PROPERTY => {
                        editor.theme.get("variable.other.member")
                    }

                    SymbolKind::VARIABLE => editor.theme.get("variable"),
                    SymbolKind::CONSTANT => editor.theme.get("constant"),
                    SymbolKind::CONSTRUCTOR => editor.theme.get("constructor"),
                    SymbolKind::STRING => editor.theme.get("string"),
                    SymbolKind::NUMBER => editor.theme.get("constant.numeric"),
                    SymbolKind::BOOLEAN => editor.theme.get("constant.builtin.boolean"),
                    SymbolKind::ARRAY => editor.theme.get("punctuation.bracket"),
                    SymbolKind::KEY => editor.theme.get("label"),
                    SymbolKind::NULL => editor.theme.get("constant.builtin"),
                    SymbolKind::TYPE_PARAMETER => editor.theme.get("type.parameter"),
                    _ => style,
                };

                let name = truncate_crumb_name(&symbol.name, max_name_length);
                x = draw_element(surface, viewport, x, &name, style);
            }
        }
    }

    pub fn render_gutter<'d>(
        editor: &'d Editor,
        doc: &'d Document,
        view: &View,
        viewport: Rect,
        theme: &Theme,
        is_focused: bool,
        decoration_manager: &mut DecorationManager<'d>,
    ) {
        let text = doc.text().slice(..);
        let cursors: Rc<[_]> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let mut offset = 0;

        let gutter_style = theme.get("ui.gutter");
        let gutter_selected_style = theme.get("ui.gutter.selected");
        let gutter_style_virtual = theme.get("ui.gutter.virtual");
        let gutter_selected_style_virtual = theme.get("ui.gutter.selected.virtual");

        for gutter_type in view.gutters() {
            let mut gutter = gutter_type.style(editor, doc, view, theme, is_focused);
            let width = gutter_type.width(view, doc);
            // avoid lots of small allocations by reusing a text buffer for each line
            let mut text = String::with_capacity(width);
            let cursors = cursors.clone();
            let gutter_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                // TODO handle softwrap in gutters
                let selected = cursors.contains(&pos.doc_line);
                let x = viewport.x + offset;
                let y = pos.visual_line;

                let gutter_style = match (selected, pos.first_visual_line) {
                    (false, true) => gutter_style,
                    (true, true) => gutter_selected_style,
                    (false, false) => gutter_style_virtual,
                    (true, false) => gutter_selected_style_virtual,
                };

                if let Some(style) =
                    gutter(pos.doc_line, selected, pos.first_visual_line, &mut text)
                {
                    renderer.set_stringn(x, y, &text, width, gutter_style.patch(style));
                } else {
                    renderer.set_style(
                        Rect {
                            x,
                            y,
                            width: width as u16,
                            height: 1,
                        },
                        gutter_style,
                    );
                }
                text.clear();
            };
            decoration_manager.add_decoration(gutter_decoration);

            offset += width as u16;
        }
    }

    pub fn render_diagnostics(
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        use helix_core::diagnostic::Severity;
        use tui::{
            layout::Alignment,
            text::Text,
            widgets::{Paragraph, Widget, Wrap},
        };

        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diagnostics = doc.diagnostics().iter().filter(|diagnostic| {
            diagnostic.range.start <= cursor && diagnostic.range.end >= cursor
        });

        let warning = theme.get("warning");
        let error = theme.get("error");
        let info = theme.get("info");
        let hint = theme.get("hint");

        let mut lines = Vec::new();
        let background_style = theme.get("ui.background");
        for diagnostic in diagnostics {
            let style = Style::reset()
                .patch(background_style)
                .patch(match diagnostic.severity {
                    Some(Severity::Error) => error,
                    Some(Severity::Warning) | None => warning,
                    Some(Severity::Info) => info,
                    Some(Severity::Hint) => hint,
                });
            let text = Text::styled(&diagnostic.message, style);
            lines.extend(text.lines);
            let code = diagnostic.code.as_ref().map(|x| match x {
                NumberOrString::Number(n) => format!("({n})"),
                NumberOrString::String(s) => format!("({s})"),
            });
            if let Some(code) = code {
                let span = Span::styled(code, style);
                lines.push(span.into());
            }
        }

        let text = Text::from(lines);
        let paragraph = Paragraph::new(&text)
            .alignment(Alignment::Right)
            .wrap(Wrap { trim: true });
        let width = 100.min(viewport.width);
        // below the first row, and inside the view
        let height = 15.min(viewport.height.saturating_sub(1));
        if width == 0 || height == 0 {
            return;
        }
        paragraph.render(
            Rect::new(viewport.right() - width, viewport.y + 1, width, height),
            surface,
        );
    }

    /// Apply the highlighting on the lines where a cursor is active
    pub fn cursorline(doc: &Document, view: &View, theme: &Theme) -> impl Decoration {
        let text = doc.text().slice(..);
        // TODO only highlight the visual line that contains the cursor instead of the full visual line
        let primary_line = doc.selection(view.id).primary().cursor_line(text);

        // The secondary_lines do contain the primary_line, it doesn't matter
        // as the else-if clause in the loop later won't test for the
        // secondary_lines if primary_line == line.
        // It's used inside a loop so the collect isn't needless:
        // https://github.com/rust-lang/rust-clippy/issues/6164
        #[allow(clippy::needless_collect)]
        let secondary_lines: Vec<_> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let primary_style = theme.get("ui.cursorline.primary");
        let secondary_style = theme.get("ui.cursorline.secondary");
        let viewport = view.area;

        move |renderer: &mut TextRenderer, pos: LinePos| {
            let area = Rect::new(viewport.x, pos.visual_line, viewport.width, 1);
            if primary_line == pos.doc_line {
                renderer.set_style(area, primary_style);
            } else if secondary_lines.binary_search(&pos.doc_line).is_ok() {
                renderer.set_style(area, secondary_style);
            }
        }
    }

    /// Apply the highlighting on the columns where a cursor is active
    pub fn highlight_cursorcolumn(
        doc: &Document,
        view: &View,
        surface: &mut Surface,
        theme: &Theme,
        viewport: Rect,
        text_annotations: &TextAnnotations,
    ) {
        let text = doc.text().slice(..);

        // Manual fallback behaviour:
        // ui.cursorcolumn.{p/s} -> ui.cursorcolumn -> ui.cursorline.{p/s}
        let primary_style = theme
            .try_get_exact("ui.cursorcolumn.primary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.primary"));
        let secondary_style = theme
            .try_get_exact("ui.cursorcolumn.secondary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.secondary"));

        let inner_area = view.inner_area(doc);

        let selection = doc.selection(view.id);
        let view_offset = doc.view_offset(view.id);
        let primary = selection.primary();
        let text_format = doc.text_format(viewport.width, None);
        for range in selection.iter() {
            let is_primary = primary == *range;
            let cursor = range.cursor(text);

            let Position { col, .. } =
                visual_offset_from_block(text, cursor, cursor, &text_format, text_annotations).0;

            // if the cursor is horizontally in the view
            if col >= view_offset.horizontal_offset
                && inner_area.width > (col - view_offset.horizontal_offset) as u16
            {
                // The text rows only: not over the breadcrumb bar above them.
                let area = Rect::new(
                    inner_area.x + (col - view_offset.horizontal_offset) as u16,
                    inner_area.y,
                    1,
                    inner_area.height,
                );
                if is_primary {
                    surface.set_style(area, primary_style)
                } else {
                    surface.set_style(area, secondary_style)
                }
            }
        }
    }

    /// Handle events by looking them up in `self.keymaps`. Returns None
    /// if event was handled (a command was executed or a subkeymap was
    /// activated). Only KeymapResult::{NotFound, Cancelled} is returned
    /// otherwise.
    fn handle_keymap_event(
        &mut self,
        mode: Mode,
        cxt: &mut commands::Context,
        event: KeyEvent,
    ) -> Option<KeymapResult> {
        let mut last_mode = mode;
        self.pseudo_pending.extend(self.keymaps.pending());
        let key_result = self.keymaps.get(mode, event);
        cxt.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());
        // Publish the pending key sequence so sibling components (e.g. the
        // explorer panel) can tell partial multi-key sequences like
        // `<space>x` apart from bare keys.
        cxt.editor.keymap_pending = self.keymaps.pending().to_vec();

        let routed_to_explorer = matches!(
            &key_result,
            KeymapResult::Matched(command) if route_motion_to_explorer(cxt, command)
        );

        let mut execute_command = |command: &commands::MappableCommand| {
            command.execute(cxt);
            helix_event::dispatch(PostCommand { command, cx: cxt });

            let current_mode = cxt.editor.mode();
            if current_mode != last_mode {
                helix_event::dispatch(OnModeSwitch {
                    old_mode: last_mode,
                    new_mode: current_mode,
                    cx: cxt,
                });

                // HAXX: if we just entered insert mode from normal, clear key buf
                // and record the command that got us into this mode.
                if current_mode == Mode::Insert {
                    // how we entered insert mode is important, and we should track that so
                    // we can repeat the side effect.
                    self.last_insert.0 = command.clone();
                    self.last_insert.1.clear();
                }
            }

            last_mode = current_mode;
        };

        match &key_result {
            KeymapResult::Matched(command) => {
                if !routed_to_explorer {
                    execute_command(command);
                }
            }
            KeymapResult::Pending(node) => cxt.editor.autoinfo = Some(node.infobox()),
            KeymapResult::MatchedSequence(commands) => {
                for command in commands {
                    execute_command(command);
                }
            }
            KeymapResult::NotFound | KeymapResult::Cancelled(_) => return Some(key_result),
            KeymapResult::Fallback(fallback, ch) => {
                fallback.execute(cxt, *ch);
            }
        }
        None
    }

    fn insert_mode(&mut self, cx: &mut commands::Context, event: KeyEvent) {
        if let Some(keyresult) = self.handle_keymap_event(Mode::Insert, cx, event) {
            match keyresult {
                KeymapResult::NotFound => {
                    if !self.on_next_key(OnKeyCallbackKind::Fallback, cx, event) {
                        if let Some(ch) = event.char() {
                            commands::insert::insert_char(cx, ch)
                        }
                    }
                }
                KeymapResult::Cancelled(pending) => {
                    for ev in pending {
                        match ev.char() {
                            Some(ch) => commands::insert::insert_char(cx, ch),
                            None => {
                                if let KeymapResult::Matched(command) =
                                    self.keymaps.get(Mode::Insert, ev)
                                {
                                    command.execute(cx);
                                }
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    /// Keys of normal mode, through the keys of the diff views first (`[editor.diff.keys]`)
    /// in a view of a diff: the keys of an action run it; the start of the keys of one waits
    /// for the next key; other keys (the held back ones with them) go to the keymap, so `]f`
    /// or `>` still work there.
    fn command_mode_in_diffs(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
        let in_diff = mode == Mode::Normal
            && self.keymaps.pending().is_empty()
            && cxt.editor.diff_session(cxt.editor.tree.focus).is_some();
        if !in_diff && self.diff_pending.is_empty() {
            return self.command_mode(mode, cxt, event);
        }
        self.diff_pending.push(event);
        let config = cxt.editor.config();
        match commands::diff::match_keys(&config.diff, &self.diff_pending) {
            commands::diff::KeyMatch::Action(action) => {
                self.diff_pending.clear();
                drop(config);
                // the count typed before the keys (`3]c`), as the keymap's commands get it
                cxt.count = cxt.editor.count;
                action(cxt);
                cxt.editor.count = None;
            }
            commands::diff::KeyMatch::Prefix => {}
            commands::diff::KeyMatch::None => {
                drop(config);
                for key in std::mem::take(&mut self.diff_pending) {
                    self.command_mode(mode, cxt, key);
                }
            }
        }
    }

    fn command_mode(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
        match (event, cxt.editor.count) {
            // If the count is already started and the input is a number, always continue the count.
            (key!(i @ '0'..='9'), Some(count)) => {
                let i = i.to_digit(10).unwrap() as usize;
                let count = count.get() * 10 + i;
                if count > 100_000_000 {
                    return;
                }
                cxt.editor.count = NonZeroUsize::new(count);
            }
            // A non-zero digit will start the count if that number isn't used by a keymap.
            (key!(i @ '1'..='9'), None) if !self.keymaps.contains_key(mode, event) => {
                let i = i.to_digit(10).unwrap() as usize;
                cxt.editor.count = NonZeroUsize::new(i);
            }
            // special handling for repeat operator
            (key!('.'), _) if self.keymaps.pending().is_empty() => {
                for _ in 0..cxt.editor.count.map_or(1, NonZeroUsize::into) {
                    // first execute whatever put us into insert mode
                    self.last_insert.0.execute(cxt);
                    let mut last_savepoint = None;
                    let mut last_request_savepoint = None;
                    // then replay the inputs
                    for key in self.last_insert.1.clone() {
                        match key {
                            InsertEvent::Key(key) => self.insert_mode(cxt, key),
                            InsertEvent::CompletionApply {
                                trigger_offset,
                                changes,
                            } => {
                                let (view, doc) = current!(cxt.editor);

                                if let Some(last_savepoint) = last_savepoint.as_deref() {
                                    doc.restore(view, last_savepoint, true);
                                }

                                let text = doc.text().slice(..);
                                let cursor = doc.selection(view.id).primary().cursor(text);

                                let shift_position = |pos: usize| -> usize {
                                    (pos + cursor).saturating_sub(trigger_offset)
                                };

                                let tx = Transaction::change(
                                    doc.text(),
                                    changes.iter().cloned().map(|(start, end, t)| {
                                        (shift_position(start), shift_position(end), t)
                                    }),
                                );
                                doc.apply(&tx, view.id);
                            }
                            InsertEvent::TriggerCompletion => {
                                last_savepoint = take(&mut last_request_savepoint);
                            }
                            InsertEvent::RequestCompletion => {
                                let (view, doc) = current!(cxt.editor);
                                last_request_savepoint = Some(doc.savepoint(view));
                            }
                        }
                    }
                }
                cxt.editor.count = None;
            }
            _ => {
                // set the count
                cxt.count = cxt.editor.count;
                // TODO: edge case: 0j -> reset to 1
                // if this fails, count was Some(0)
                // debug_assert!(cxt.count != 0);

                // set the register
                cxt.register = cxt.editor.selected_register.take();

                let res = self.handle_keymap_event(mode, cxt, event);
                if matches!(&res, Some(KeymapResult::NotFound)) {
                    self.on_next_key(OnKeyCallbackKind::Fallback, cxt, event);
                }
                if self.keymaps.pending().is_empty() {
                    cxt.editor.count = None
                } else {
                    cxt.editor.selected_register = cxt.register.take();
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_completion(
        &mut self,
        editor: &mut Editor,
        items: Vec<CompletionItem>,
        trigger_offset: usize,
        size: Rect,
    ) -> Option<Rect> {
        let mut completion = Completion::new(editor, items, trigger_offset);

        if !completion.is_empty() {
            // Ghost text pauses while the completion menu is open.
            doc_mut!(editor).inline_completions.clear();
        }

        if completion.is_empty() {
            // skip if we got no completion results
            return None;
        }

        let area = completion.area(size, editor);
        editor.last_completion = Some(CompleteAction::Triggered);
        self.last_insert.1.push(InsertEvent::TriggerCompletion);

        // TODO : propagate required size on resize to completion too
        self.completion = Some(completion);
        let (view, doc) = current_ref!(editor);
        self.completion_origin = Some((view.id, doc.id()));
        Some(area)
    }

    pub fn clear_completion(&mut self, editor: &mut Editor) -> Option<OnKeyCallback> {
        self.completion = None;
        let origin = self.completion_origin.take();
        let mut on_next_key: Option<OnKeyCallback> = None;
        editor.handlers.completions.request_controller.restart();
        editor.handlers.completions.active_completions.clear();
        if let Some(last_completion) = editor.last_completion.take() {
            match last_completion {
                CompleteAction::Triggered => (),
                CompleteAction::Applied {
                    trigger_offset,
                    changes,
                    placeholder,
                } => {
                    self.last_insert.1.push(InsertEvent::CompletionApply {
                        trigger_offset,
                        changes,
                    });
                    on_next_key = placeholder.then_some(Box::new(|cx, key| {
                        if let Some(c) = key.char() {
                            let (view, doc) = current!(cx.editor);
                            if let Some(snippet) = &doc.active_snippet {
                                doc.apply(&snippet.delete_placeholder(doc.text()), view.id);
                            }
                            commands::insert::insert_char(cx, c);
                        }
                    }))
                }
                CompleteAction::Selected { savepoint } => {
                    // Take the preview back out where it was shown, which is not necessarily
                    // the focused view and document anymore.
                    let (view_id, doc_id) = origin.unwrap_or_else(|| {
                        let (view, doc) = current_ref!(editor);
                        (view.id, doc.id())
                    });
                    if savepoint.view == view_id && editor.tree.contains(view_id) {
                        if let Some(doc) = editor.documents.get_mut(&doc_id) {
                            doc.restore(editor.tree.get_mut(view_id), &savepoint, false);
                        }
                    }
                }
            }
        }
        on_next_key
    }

    /// Clears a completion whose view or document lost the focus to something other than
    /// the editor's own mouse handling, e.g. a file opened from the explorer, a plugin or a
    /// buffer switch: it must not keep previewing into, or later be restored through, the
    /// newly focused document.
    fn clear_stale_completion(&mut self, editor: &mut Editor) {
        let Some(origin) = self.completion_origin else {
            return;
        };
        let (view, doc) = current_ref!(editor);
        if origin != (view.id, doc.id()) {
            // A pending placeholder edit belongs to the old document too: drop it.
            let _ = self.clear_completion(editor);
        }
    }

    /// Clears the completion before a mouse action focuses another view: its preview and
    /// savepoint belong to the view it was opened in, and restoring them through another view
    /// panics ("Savepoint must not be used with a different view!", helix-editor/helix#15349).
    fn clear_completion_on_view_switch(&mut self, editor: &mut Editor, view_id: ViewId) {
        if editor.tree.focus != view_id {
            self.clear_completion(editor);
        }
    }

    pub fn handle_idle_timeout(&mut self, cx: &mut commands::Context) -> EventResult {
        commands::compute_inlay_hints_for_all_views(cx.editor, cx.jobs);

        EventResult::Ignored(None)
    }
}

/// Whether the focused doc's workspace is in restricted mode and running `trust` would
/// change something visible at the workspace level.
fn workspace_trust_indicator_visible(editor: &Editor) -> bool {
    if editor.workspace_trust.implicit_level()
        == helix_loader::workspace_trust::ImplicitTrustLevel::Insecure
    {
        return false;
    }
    let (_, doc) = helix_view::current_ref!(editor);
    editor
        .workspace_trust
        .restricted_for_doc(doc.workspace_root(), doc.servers_to_load())
}

impl EditorView {
    /// must be called whenever the editor processed input that
    /// is not a `KeyEvent`. In these cases any pending keys/on next
    /// key callbacks must be canceled.
    fn handle_non_key_input(&mut self, cxt: &mut commands::Context) {
        cxt.editor.status_msg = None;
        cxt.editor.reset_idle_timer();
        // HACKS: create a fake key event that will never trigger any actual map
        // and therefore simply acts as "dismiss"
        let null_key_event = KeyEvent {
            code: KeyCode::Null,
            modifiers: KeyModifiers::empty(),
        };
        // dismiss any pending keys
        if let Some((on_next_key, _)) = self.on_next_key.take() {
            on_next_key(cxt, null_key_event);
        }
        self.handle_keymap_event(cxt.editor.mode, cxt, null_key_event);
        self.pseudo_pending.clear();
    }

    fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        cxt: &mut commands::Context,
    ) -> EventResult {
        if event.kind != MouseEventKind::Moved {
            self.handle_non_key_input(cxt)
        }

        let config = cxt.editor.config();
        let MouseEvent {
            kind,
            row,
            column,
            modifiers,
            ..
        } = *event;

        let pos_and_view = |editor: &Editor, row, column, ignore_virtual_text| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.pos_at_screen_coords(
                    &editor.documents[&view.doc],
                    row,
                    column,
                    ignore_virtual_text,
                )
                .map(|pos| (pos, view.id))
            })
        };

        let gutter_coords_and_view = |editor: &Editor, row, column| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.gutter_coords_at_screen_coords(row, column)
                    .map(|coords| (coords, view.id))
            })
        };

        // A divider being dragged owns the pointer until the button is
        // released: the splits on either side follow it.
        if let Some(divider) = self.split_drag {
            match kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    cxt.editor.tree.drag_divider(divider, row, column);
                    cxt.editor.pointer_shape = split_pointer(divider);
                    return EventResult::Consumed(None);
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.split_drag = None;
                    return EventResult::Consumed(None);
                }
                _ => {}
            }
        }

        // Hovering a split divider offers the resize pointer.
        if kind == MouseEventKind::Moved {
            if let Some(divider) = cxt.editor.tree.divider_at(row, column) {
                cxt.editor.pointer_shape = split_pointer(divider);
            }
        }

        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let editor = &mut cxt.editor;

                let config = editor.config();
                let bufferline_visible = match config.bufferline.render_mode {
                    helix_view::editor::BufferLineRenderMode::Always => true,
                    helix_view::editor::BufferLineRenderMode::Multiple => {
                        editor.documents.len() > 1
                    }
                    _ => false,
                };
                if bufferline_visible && row == 0 {
                    let clicked = self
                        .bufferline_info
                        .get_clicked_buffer(column)
                        .map(|buffer_info| buffer_info.document_id);
                    if let Some(document_id) = clicked {
                        // The clicked buffer replaces the focused view's document, which the
                        // completion's preview and savepoint belong to.
                        self.clear_completion(editor);
                        editor.switch(document_id, helix_view::editor::Action::Replace);
                    }

                    return EventResult::Consumed(None);
                }

                // Grabbing the line between two splits starts resizing them.
                if let Some(divider) = editor.tree.divider_at(row, column) {
                    editor.pointer_shape = split_pointer(divider);
                    self.split_drag = Some(divider);
                    return EventResult::Consumed(None);
                }

                if let Some((_, view_id)) = pos_and_view(editor, row, column, true) {
                    self.clear_completion_on_view_switch(editor, view_id);
                    // Clearing a preview can change the text in both views. Resolve the
                    // position again before focusing can scroll the target view.
                    let Some((pos, _)) = pos_and_view(editor, row, column, true) else {
                        return EventResult::Ignored(None);
                    };
                    editor.focus(view_id);
                    // The click belongs to the buffer, so keyboard focus leaves the explorer.
                    editor.explorer_panel.focused = false;

                    let doc = doc_mut!(editor, &view!(editor, view_id).doc);

                    if modifiers == KeyModifiers::ALT {
                        let selection = doc.selection(view_id).clone();
                        doc.set_selection(view_id, selection.push(Range::point(pos)));
                    } else if editor.mode == Mode::Select {
                        // Discards non-primary selections for consistent UX with normal mode
                        let primary = doc.selection(view_id).primary().put_cursor(
                            doc.text().slice(..),
                            pos,
                            true,
                        );
                        editor.mouse_down_range = Some(primary);
                        doc.set_selection(view_id, Selection::single(primary.anchor, primary.head));
                    } else {
                        doc.set_selection(view_id, Selection::point(pos));
                    }

                    editor.ensure_cursor_in_view(view_id);

                    return EventResult::Consumed(None);
                }

                if let Some((coords, view_id)) = gutter_coords_and_view(editor, row, column) {
                    self.clear_completion_on_view_switch(editor, view_id);
                    editor.focus(view_id);
                    editor.explorer_panel.focused = false;

                    let (view, doc) = current!(cxt.editor);

                    let Some(path) = doc.path().map(ToOwned::to_owned) else {
                        return EventResult::Ignored(None);
                    };

                    if let Some(char_idx) =
                        view.pos_at_visual_coords(doc, coords.row as u16, coords.col as u16, true)
                    {
                        let line = doc.text().char_to_line(char_idx);
                        commands::dap_toggle_breakpoints_impl(cxt, path, vec![line]);
                        return EventResult::Consumed(None);
                    }
                }

                EventResult::Ignored(None)
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                let (view, doc) = current!(cxt.editor);

                let pos = match view.pos_at_screen_coords(doc, row, column, true) {
                    Some(pos) => pos,
                    None => return EventResult::Ignored(None),
                };

                let mut selection = doc.selection(view.id).clone();
                let primary = selection.primary_mut();
                *primary = primary.put_cursor(doc.text().slice(..), pos, true);
                doc.set_selection(view.id, selection);
                let view_id = view.id;
                cxt.editor.ensure_cursor_in_view(view_id);
                EventResult::Consumed(None)
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let current_view = cxt.editor.tree.focus;

                let direction = match event.kind {
                    MouseEventKind::ScrollUp => Direction::Backward,
                    MouseEventKind::ScrollDown => Direction::Forward,
                    _ => unreachable!(),
                };

                match pos_and_view(cxt.editor, row, column, false) {
                    Some((_, view_id)) => cxt.editor.tree.focus = view_id,
                    None => return EventResult::Ignored(None),
                }

                let offset = config.scroll_lines.unsigned_abs();
                commands::scroll(cxt, offset, direction, false);

                cxt.editor.tree.focus = current_view;
                cxt.editor.ensure_cursor_in_view(current_view);

                EventResult::Consumed(None)
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if !config.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                let (view, doc) = current!(cxt.editor);

                let should_yank = match cxt.editor.mouse_down_range.take() {
                    Some(down_range) => doc.selection(view.id).primary() != down_range,
                    None => {
                        // This should not happen under normal cases. We fall back to the original
                        // behavior of yanking on non-single-char selections.
                        doc.selection(view.id)
                            .primary()
                            .slice(doc.text().slice(..))
                            .len_chars()
                            > 1
                    }
                };

                if should_yank {
                    commands::MappableCommand::yank_main_selection_to_primary_clipboard
                        .execute(cxt);
                    EventResult::Consumed(None)
                } else {
                    EventResult::Ignored(None)
                }
            }

            MouseEventKind::Up(MouseButton::Right) => {
                if let Some((pos, view_id)) = gutter_coords_and_view(cxt.editor, row, column) {
                    self.clear_completion_on_view_switch(cxt.editor, view_id);
                    cxt.editor.focus(view_id);

                    if let Some((pos, _)) = pos_and_view(cxt.editor, row, column, true) {
                        doc_mut!(cxt.editor).set_selection(view_id, Selection::point(pos));
                    } else {
                        let (view, doc) = current!(cxt.editor);

                        if let Some(pos) = view.pos_at_visual_coords(doc, pos.row as u16, 0, true) {
                            doc.set_selection(view_id, Selection::point(pos));
                            match modifiers {
                                KeyModifiers::ALT => {
                                    commands::MappableCommand::dap_edit_log.execute(cxt)
                                }
                                _ => commands::MappableCommand::dap_edit_condition.execute(cxt),
                            };
                        }
                    }

                    cxt.editor.ensure_cursor_in_view(view_id);
                    return EventResult::Consumed(None);
                }
                EventResult::Ignored(None)
            }

            MouseEventKind::Up(MouseButton::Middle) => {
                let editor = &mut cxt.editor;
                if !config.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                if modifiers == KeyModifiers::ALT {
                    commands::MappableCommand::replace_selections_with_primary_clipboard
                        .execute(cxt);

                    return EventResult::Consumed(None);
                }

                if let Some((_, view_id)) = pos_and_view(editor, row, column, true) {
                    self.clear_completion_on_view_switch(editor, view_id);
                    let Some((pos, _)) = pos_and_view(editor, row, column, true) else {
                        return EventResult::Ignored(None);
                    };
                    editor.focus(view_id);
                    let doc = doc_mut!(editor, &view!(editor, view_id).doc);
                    doc.set_selection(view_id, Selection::point(pos));
                    commands::MappableCommand::paste_primary_clipboard_before.execute(cxt);

                    return EventResult::Consumed(None);
                }

                EventResult::Ignored(None)
            }

            _ => EventResult::Ignored(None),
        }
    }
    fn on_next_key(
        &mut self,
        kind: OnKeyCallbackKind,
        ctx: &mut commands::Context,
        event: KeyEvent,
    ) -> bool {
        if let Some((on_next_key, kind_)) = self.on_next_key.take() {
            if kind == kind_ {
                on_next_key(ctx, event);
                true
            } else {
                self.on_next_key = Some((on_next_key, kind_));
                false
            }
        } else {
            false
        }
    }
}

impl Component for EditorView {
    fn handle_event(
        &mut self,
        event: &Event,
        context: &mut crate::compositor::Context,
    ) -> EventResult {
        self.clear_stale_completion(context.editor);
        let mut cx = commands::Context {
            editor: context.editor,
            count: None,
            register: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: context.jobs,
            plugin_manager: context.plugin_manager.clone(),
        };

        match event {
            Event::Paste(contents) => {
                self.handle_non_key_input(&mut cx);
                cx.count = cx.editor.count;
                commands::paste_bracketed_value(&mut cx, contents.clone());
                cx.editor.count = None;

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);
                view.ensure_cursor_in_view(doc, config.scrolloff);

                // Store a history state if not in insert mode. Otherwise wait till we exit insert
                // to include any edits to the paste in the history state.
                if mode != Mode::Insert {
                    doc.append_changes_to_history(view);
                }

                EventResult::Consumed(None)
            }
            Event::Resize(_width, _height) => {
                // Ignore this event, we handle resizing just before rendering to screen.
                // Handling it here but not re-rendering will cause flashing
                EventResult::Consumed(None)
            }
            Event::Key(mut key) => {
                cx.editor.reset_idle_timer();
                canonicalize_key(&mut key);

                // clear status
                cx.editor.status_msg = None;

                let mode = cx.editor.mode();

                if !self.on_next_key(OnKeyCallbackKind::PseudoPending, &mut cx, key) {
                    match mode {
                        Mode::Insert => {
                            // let completion swallow the event if necessary
                            let mut consumed = false;
                            if let Some(completion) = &mut self.completion {
                                let res = {
                                    // use a fake context here
                                    let mut cx = Context {
                                        editor: cx.editor,
                                        jobs: cx.jobs,
                                        scroll: None,
                                        plugin_manager: cx.plugin_manager.clone(),
                                    };

                                    if let EventResult::Consumed(callback) =
                                        completion.handle_event(event, &mut cx)
                                    {
                                        consumed = true;
                                        Some(callback)
                                    } else if let EventResult::Consumed(callback) =
                                        completion.handle_event(&Event::Key(key!(Enter)), &mut cx)
                                    {
                                        Some(callback)
                                    } else {
                                        None
                                    }
                                };

                                if let Some(callback) = res {
                                    if callback.is_some() {
                                        // assume close_fn
                                        if let Some(cb) = self.clear_completion(cx.editor) {
                                            if consumed {
                                                cx.on_next_key_callback =
                                                    Some((cb, OnKeyCallbackKind::Fallback))
                                            } else {
                                                self.on_next_key =
                                                    Some((cb, OnKeyCallbackKind::Fallback));
                                            }
                                        }
                                    }
                                }
                            }

                            // if completion didn't take the event, we pass it onto commands
                            if !consumed {
                                self.insert_mode(&mut cx, key);

                                // record last_insert key
                                self.last_insert.1.push(InsertEvent::Key(key));
                            }
                        }
                        mode => self.command_mode_in_diffs(mode, &mut cx, key),
                    }
                }

                self.on_next_key = cx.on_next_key_callback.take();
                match self.on_next_key {
                    Some((_, OnKeyCallbackKind::PseudoPending)) => self.pseudo_pending.push(key),
                    _ => self.pseudo_pending.clear(),
                }

                // appease borrowck
                let callbacks = take(&mut cx.callback);

                // if the command consumed the last view, skip the render.
                // on the next loop cycle the Application will then terminate.
                if cx.editor.should_close() {
                    return EventResult::Ignored(None);
                }

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);

                view.ensure_cursor_in_view(doc, config.scrolloff);

                // Store a history state if not in insert mode. This also takes care of
                // committing changes when leaving insert mode.
                if mode != Mode::Insert {
                    doc.append_changes_to_history(view);
                }
                let callback = if callbacks.is_empty() {
                    None
                } else {
                    let callback: crate::compositor::Callback = Box::new(move |compositor, cx| {
                        for callback in callbacks {
                            callback(compositor, cx)
                        }
                    });
                    Some(callback)
                };

                EventResult::Consumed(callback)
            }

            Event::Mouse(event) => self.handle_mouse_event(event, &mut cx),
            Event::IdleTimeout => self.handle_idle_timeout(&mut cx),
            Event::FocusGained => {
                if context.editor.config().auto_reload.focus_gained
                    && crate::handlers::auto_reload::count_externally_modified_documents(
                        context.editor.documents(),
                    ) > 0
                {
                    if let Err(e) = commands::typed::reload_all(
                        context,
                        helix_core::command_line::Args::default(),
                        super::PromptEvent::Validate,
                    ) {
                        context.editor.set_error(format!("{}", e));
                    } else {
                        context
                            .editor
                            .set_status("Reloaded files due to external changes");
                    }
                }
                self.terminal_focused = true;
                EventResult::Consumed(None)
            }
            Event::FocusLost => {
                if context.editor.config().auto_save.focus_lost {
                    let options = commands::WriteAllOptions {
                        force: false,
                        write_scratch: false,
                        auto_format: false,
                        code_actions: false,
                    };
                    if let Err(e) = commands::typed::write_all_impl(context, options) {
                        context.editor.set_error(format!("{}", e));
                    }
                }
                self.terminal_focused = false;
                EventResult::Consumed(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // Another component (the explorer, a plugin, ...) may have moved the focus away from
        // the completion since the last event: take its preview out before drawing.
        self.clear_stale_completion(cx.editor);
        // clear with background color
        surface.set_style(area, cx.editor.theme.get("ui.background"));

        let use_bufferline = uses_bufferline(cx.editor);
        // The views' frame, and in it the views: above the quickfix panel, when it is shown.
        let editor_area = crate::ui::quickfix::layout(cx.editor, frame(cx.editor, area));

        // if the terminal size suddenly changed, we need to trigger a resize
        cx.editor.resize(editor_area);

        if use_bufferline {
            // The pinned explorer owns the full height, so the buffer line
            // only spans the columns beside it.
            let bufferline_area = crate::ui::explorer::layout(cx.editor, area.with_height(1));
            self.render_bufferline(cx.editor, bufferline_area, surface);
        }

        // The diffs as the documents are now (or ended, when a view of one went).
        cx.editor.refresh_diff_sessions();
        // a side of a diff scrolled since the last frame takes the other one along
        cx.editor.sync_diff_scrolling();
        for (view, is_focused) in cx.editor.tree.views() {
            let doc = cx.editor.document(view.doc).unwrap();
            // The views' frame: a view ending short of it has a neighbour (and a divider) on
            // its right, not a pinned explorer panel.
            self.render_view(cx.editor, doc, view, editor_area, surface, is_focused);
        }
        crate::ui::diff_view::render_connectors(cx.editor, surface);
        self.terminal_secondary_cursors = self.secondary_cursor_cells(cx.editor, surface);

        // The info box of pending keys (`cx.editor.autoinfo`) is drawn by the compositor, over
        // every layer: see `Compositor::render`.

        let key_width = 15u16; // for showing pending keys
        let mut status_msg_width = 0;

        // render status msg
        if let Some((status_msg, severity)) = &cx.editor.status_msg {
            status_msg_width = status_msg.width();
            use helix_view::editor::Severity;
            let style = if *severity == Severity::Error {
                cx.editor.theme.get("error")
            } else {
                cx.editor.theme.get("ui.text")
            };

            surface.set_string(
                area.x,
                area.y + area.height.saturating_sub(1),
                status_msg,
                style,
            );
        }

        if area.width.saturating_sub(status_msg_width as u16) > key_width {
            let mut disp = String::new();
            if let Some(count) = cx.editor.count {
                disp.push_str(&count.to_string())
            }
            for key in self.keymaps.pending() {
                disp.push_str(&key.key_sequence_format());
            }
            for key in &self.pseudo_pending {
                disp.push_str(&key.key_sequence_format());
            }
            let style = cx.editor.theme.get("ui.text");
            let macro_width = if cx.editor.macro_recording.is_some() {
                3
            } else {
                0
            };
            let restricted = workspace_trust_indicator_visible(cx.editor);
            let trust_width = if restricted { 3 } else { 0 };
            surface.set_string(
                area.x
                    + area
                        .width
                        .saturating_sub(key_width + macro_width + trust_width),
                area.y + area.height.saturating_sub(1),
                disp.get(disp.len().saturating_sub(key_width as usize)..)
                    .unwrap_or(&disp),
                style,
            );
            if restricted {
                let style = style
                    .fg(helix_view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                surface.set_string(
                    area.x
                        .saturating_add(area.width.saturating_sub(3 + macro_width)),
                    area.y + area.height.saturating_sub(1),
                    "[⚠]",
                    style,
                );
            }
            if let Some((reg, _)) = cx.editor.macro_recording {
                let disp = format!("[{}]", reg);
                let style = style
                    .fg(helix_view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                surface.set_string(
                    area.x + area.width.saturating_sub(3),
                    area.y + area.height.saturating_sub(1),
                    &disp,
                    style,
                );
            }
        }

        // Cleanup expired notifications before rendering
        cx.editor.cleanup_notifications();

        // The quickfix panel (a layer above this one) takes the bottom rows of the views'
        // frame: what is drawn here over the views stays above it, or the panel would cover it.
        let panel = crate::ui::quickfix::panel_area(cx.editor, frame(cx.editor, area));
        let above_panel = if panel.height > 0 {
            area.with_height(panel.y.saturating_sub(area.y))
        } else {
            area
        };

        // The notifications: over the views, under what is being used. The completion menu
        // (below) and the layers above this one (pickers, popups, the command line) are
        // drawn over them. A pinned explorer panel is drawn above this view, so they are
        // laid out in the area the panel leaves free instead of being painted underneath it.
        let notification_area = crate::ui::explorer::layout(cx.editor, above_panel);
        self.notification_popup
            .render(notification_area, surface, cx);

        if let Some(completion) = self.completion.as_mut() {
            completion.render(above_panel, surface, cx);
        }
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        let (pos, kind) = editor.cursor();
        if self.terminal_focused {
            (pos, kind)
        } else {
            // use underline cursor when terminal loses focus for visibility
            (pos, CursorKind::Underline)
        }
    }
}

/// Whether the buffer line is drawn (`bufferline`).
fn uses_bufferline(editor: &Editor) -> bool {
    use helix_view::editor::BufferLineRenderMode;
    match editor.config().bufferline.render_mode {
        BufferLineRenderMode::Always => true,
        BufferLineRenderMode::Multiple => editor.documents.len() > 1,
        _ => false,
    }
}

/// The frame of the views in the screen `area`: without the command line's row (unless the
/// popup command line takes the full height), the buffer line and a pinned explorer panel.
/// The quickfix panel takes its bottom rows (see `ui::quickfix`).
pub fn frame(editor: &Editor, area: Rect) -> Rect {
    let config = editor.config();
    // Reserve bottom row for commandline only when NOT using popup with full-height
    let mut frame = if config.cmdline.style == helix_view::editor::CmdlineStyle::Popup
        && config.cmdline.use_full_height
    {
        area
    } else {
        area.clip_bottom(1)
    };
    if uses_bufferline(editor) {
        frame = frame.clip_top(1);
    }
    // Reserve the pinned explorer column so the views never allocate columns under the panel
    // (no-op when the panel is hidden or floating).
    crate::ui::explorer::layout(editor, frame)
}

/// Truncate a breadcrumb name in the middle with `…` when it exceeds
/// `max_len` characters (when `max_len` is non-zero). The result never has
/// more than `max_len` characters.
fn truncate_crumb_name(name: &str, max_len: usize) -> std::borrow::Cow<'_, str> {
    const ELLIPSIS: char = '…';
    let char_count = name.chars().count();
    if max_len == 0 || char_count <= max_len {
        return std::borrow::Cow::Borrowed(name);
    }
    // Room for the ellipsis plus `keep` characters of the name (possibly none).
    let keep = max_len - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out = String::with_capacity(max_len * 4);
    out.extend(name.chars().take(head));
    out.push(ELLIPSIS);
    out.extend(name.chars().skip(char_count - tail));
    std::borrow::Cow::Owned(out)
}

/// Drop the first `cols` display columns of `text`.
///
/// Returns the remaining text and the column (relative to the clipped start) at which it
/// begins; that is non-zero only when a wide grapheme straddles the clip point.
fn skip_display_columns(text: &str, cols: usize) -> (std::borrow::Cow<'_, str>, usize) {
    use helix_core::unicode::segmentation::UnicodeSegmentation;
    let mut skipped = 0;
    for (byte_idx, grapheme) in text.grapheme_indices(true) {
        if skipped >= cols {
            return (
                std::borrow::Cow::Borrowed(&text[byte_idx..]),
                skipped - cols,
            );
        }
        skipped += grapheme.width();
    }
    (std::borrow::Cow::Borrowed(""), skipped.saturating_sub(cols))
}

#[derive(Debug, Default)]
struct BufferLineInfo {
    visible_buffers: Vec<BufferInfo>,
}

impl BufferLineInfo {
    fn clear(&mut self) {
        self.visible_buffers.clear();
    }

    fn add_buffer_info(&mut self, document_id: DocumentId, columns: std::ops::Range<u16>) {
        self.visible_buffers.push(BufferInfo {
            document_id,
            columns,
        });
    }

    fn get_clicked_buffer(&self, column: u16) -> Option<&BufferInfo> {
        self.visible_buffers
            .iter()
            .find(|cell| cell.columns.contains(&column))
    }
}

#[derive(Debug)]
struct BufferInfo {
    document_id: DocumentId,
    // The bufferline column span used to show the document name
    columns: std::ops::Range<u16>,
}

fn canonicalize_key(key: &mut KeyEvent) {
    if let KeyEvent {
        code: KeyCode::Char(_),
        modifiers: _,
    } = key
    {
        key.modifiers.remove(KeyModifiers::SHIFT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn components(paths: &[&str]) -> Vec<Vec<String>> {
        paths
            .iter()
            .map(|p| p.split(MAIN_SEPARATOR).map(String::from).collect())
            .collect()
    }

    fn names(paths: &[&str]) -> Vec<String> {
        let components = components(paths);
        (0..paths.len())
            .map(|i| EditorView::compute_display_name(&components, i))
            .collect()
    }

    fn join(parts: &[&str]) -> String {
        parts.join(MAIN_SEPARATOR_STR)
    }

    #[test]
    fn display_name_unique_files_use_file_name() {
        let a = join(&["src", "a.rs"]);
        let b = join(&["src", "b.rs"]);
        assert_eq!(names(&[&a, &b]), ["a.rs", "b.rs"]);
    }

    #[test]
    fn display_name_nested_same_name_files() {
        let a = join(&["", "proj", "foo", "mod.rs"]);
        let b = join(&["", "proj", "bar", "mod.rs"]);
        let c = join(&["", "other", "foo", "mod.rs"]);
        assert_eq!(
            names(&[&a, &b, &c]),
            [
                join(&["proj", "foo", "mod.rs"]),
                join(&["bar", "mod.rs"]),
                join(&["other", "foo", "mod.rs"]),
            ]
        );
        // A path that is a suffix of another one.
        let short = join(&["foo", "mod.rs"]);
        let long = join(&["x", "foo", "mod.rs"]);
        assert_eq!(names(&[&short, &long]), [short.clone(), long.clone()]);
    }

    #[test]
    fn display_name_identical_paths_terminate() {
        // Several scratch buffers (welcome scratch + `:new`) used to loop forever.
        assert_eq!(
            names(&[
                SCRATCH_BUFFER_NAME,
                SCRATCH_BUFFER_NAME,
                SCRATCH_BUFFER_NAME
            ]),
            [
                SCRATCH_BUFFER_NAME.to_string(),
                format!("{SCRATCH_BUFFER_NAME} (2)"),
                format!("{SCRATCH_BUFFER_NAME} (3)"),
            ]
        );
        assert_eq!(names(&["", ""]), ["", " (2)"]);
        let p = join(&["a", "b.rs"]);
        let q = join(&["c", "d.rs"]);
        // Identical paths can't be told apart by a suffix: full path plus ordinal.
        assert_eq!(
            names(&[&p, &q, &p]),
            [p.clone(), "d.rs".into(), format!("{p} (2)")]
        );
        assert_eq!(names(&["x"]), ["x"]);
    }

    #[test]
    fn truncate_crumb_name_small_limits() {
        for name in ["abcdefgh", "äöüßéèêë", "日本語のテキスト", "ab", "a"] {
            let len = name.chars().count();
            for max_len in 0..=4 {
                let out = truncate_crumb_name(name, max_len);
                let out_len = out.chars().count();
                if max_len == 0 || len <= max_len {
                    assert_eq!(out, name);
                } else {
                    assert_eq!(out_len, max_len, "{name:?} max {max_len} -> {out:?}");
                    assert!(out.contains('…'));
                }
            }
        }
        assert_eq!(truncate_crumb_name("abcdefgh", 1), "…");
        assert_eq!(truncate_crumb_name("abcdefgh", 2), "a…");
        assert_eq!(truncate_crumb_name("abcdefgh", 3), "a…h");
        assert_eq!(truncate_crumb_name("äöüßéèêë", 4), "äö…ë");
    }

    #[test]
    fn skip_display_columns_handles_wide_chars() {
        assert_eq!(skip_display_columns("│ abc", 0), ("│ abc".into(), 0));
        assert_eq!(skip_display_columns("│ abc", 1), (" abc".into(), 0));
        assert_eq!(skip_display_columns("│ abc", 3), ("bc".into(), 0));
        // A double width char straddling the clip point is dropped whole.
        assert_eq!(skip_display_columns("日本", 1), ("本".into(), 1));
        assert_eq!(skip_display_columns("ab", 5), ("".into(), 0));
    }
}
