use crate::{
    align_view,
    annotations::{
        diagnostics::InlineDiagnostics, inline_completion::InlineCompletionLines,
        plugins::PluginLineAnnotations,
    },
    document::{DocumentColorSwatches, DocumentInlayHints},
    editor::{GutterConfig, GutterType, ScrolloffConfig},
    graphics::Rect,
    handlers::diagnostics::DiagnosticsHandler,
    Align, Document, DocumentId, Theme, ViewId,
};

use helix_core::{
    char_idx_at_visual_offset,
    doc_formatter::TextFormat,
    text_annotations::TextAnnotations,
    text_folding::{FoldAnnotations, RopeSliceFoldExt},
    visual_offset_from_anchor, visual_offset_from_block, Position, RopeSlice, Selection,
    VisualOffsetError::{PosAfterMaxRow, PosBeforeAnchorRow},
};
use serde::{Deserialize, Serialize};

use std::{collections::VecDeque, fmt};

const JUMP_LIST_CAPACITY: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jump {
    pub doc_id: DocumentId,
    selection: Selection,
    /// The history revision of the document `selection` belongs to.
    revision: usize,
}

impl Jump {
    /// Creates a jump to `selection` in the current text of `doc`, which must not have any
    /// changes that weren't committed to its history.
    pub fn new(doc: &Document, selection: Selection) -> Self {
        debug_assert!(doc.changes().is_empty(), "uncommitted changes");
        Self {
            doc_id: doc.id(),
            selection,
            revision: doc.current_revision(),
        }
    }

    /// The selection of the jump in the current text of `doc`.
    pub fn selection(&self, doc: &Document) -> Selection {
        doc.map_selection_from_revision(&self.selection, self.revision)
    }
}

#[derive(Debug, Clone)]
pub struct JumpList {
    jumps: VecDeque<Jump>,
    current: usize,
}

impl JumpList {
    pub fn new(initial: Jump) -> Self {
        let mut jumps = VecDeque::with_capacity(JUMP_LIST_CAPACITY);
        jumps.push_back(initial);
        Self { jumps, current: 0 }
    }

    fn push_impl(&mut self, jump: Jump) -> usize {
        let mut num_removed_from_front = 0;
        self.jumps.truncate(self.current);
        // don't push duplicates
        if self.jumps.back() != Some(&jump) {
            // If the jumplist is full, drop the oldest item.
            while self.jumps.len() >= JUMP_LIST_CAPACITY {
                self.jumps.pop_front();
                num_removed_from_front += 1;
            }

            self.jumps.push_back(jump);
            self.current = self.jumps.len();
        }
        num_removed_from_front
    }

    pub(crate) fn forward(&mut self, count: usize) -> Option<&Jump> {
        if self.current + count < self.jumps.len() {
            self.current += count;
            self.jumps.get(self.current)
        } else {
            None
        }
    }

    // Taking view and doc to prevent unnecessary cloning when jump is not required.
    pub(crate) fn backward(
        &mut self,
        view_id: ViewId,
        doc: &Document,
        count: usize,
    ) -> Option<&Jump> {
        if let Some(mut current) = self.current.checked_sub(count) {
            if self.current == self.jumps.len() {
                let jump = Jump::new(doc, doc.selection(view_id).clone());
                let num_removed = self.push_impl(jump);
                current = current.saturating_sub(num_removed);
            }
            self.current = current;

            // Avoid jumping to the current location.
            let jump = self.jumps.get(self.current)?;
            if doc.id() == jump.doc_id && doc.selection(view_id) == &jump.selection(doc) {
                self.current = self.current.checked_sub(1)?;
            }
            self.jumps.get(self.current)
        } else {
            None
        }
    }

    pub fn remove(&mut self, doc_id: &DocumentId) {
        // Keep `current` pointing at the same jump.
        let removed_before_current = self
            .jumps
            .iter()
            .take(self.current)
            .filter(|jump| jump.doc_id == *doc_id)
            .count();
        self.jumps.retain(|jump| jump.doc_id != *doc_id);
        self.current = self
            .current
            .saturating_sub(removed_before_current)
            .min(self.jumps.len());
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Jump> {
        self.jumps.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Copy, Default, Serialize, Deserialize)]
pub struct ViewPosition {
    pub anchor: usize,
    pub horizontal_offset: usize,
    pub vertical_offset: usize,
}

#[derive(Clone)]
pub struct View {
    pub id: ViewId,
    pub area: Rect,
    pub doc: DocumentId,
    pub jumps: JumpList,
    // documents accessed from this view from the oldest one to last viewed one
    pub docs_access_history: Vec<DocumentId>,
    /// the last modified files before the current one
    /// ordered from most frequent to least frequent
    // uses two docs because we want to be able to swap between the
    // two last modified docs which we need to manually keep track of
    pub last_modified_docs: [Option<DocumentId>; 2],
    /// used to store previous selections of tree-sitter objects
    /// (document_id, version)
    pub object_selections: Vec<((DocumentId, i32), Selection)>,
    /// all gutter-related configuration settings, used primarily for gutter rendering
    pub gutters: GutterConfig,
    // HACKS: there should really only be a global diagnostics handler (the
    // non-focused views should just not have different handling for the cursor
    // line). For that we would need accces to editor everywhere (we want to use
    // the positioning code) so this can only happen by refactoring View and
    // Document into entity component like structure. That is a huge refactor
    // left to future work. For now we treat all views as focused and give them
    // each their own handler.
    pub diagnostics_handler: DiagnosticsHandler,
    /// Whether a bar of plugins (`helix.winbar`) takes the top row (see
    /// [`crate::Editor::sync_window_bars`]).
    pub bar: bool,
    /// The window's own list of locations (Neovim's location list), beside the editor's
    /// quicklist. A window split from this one starts with a copy.
    pub location_list: crate::quicklist::Quicklist,
    /// Where this window is in the changelist of its buffer (`g;` / `g,`): the buffer, the
    /// changelist's generation, the index.
    pub changelist_walk: Option<(DocumentId, u64, usize)>,
    /// Where the labels of its closed folds (`▸ 12 lines · function`) were last drawn, for a
    /// click on one to open its fold.
    pub fold_labels: std::cell::RefCell<Vec<FoldLabel>>,
}

/// A closed fold's label on the screen: from `start` to `end` (exclusive) on `row`, for the
/// fold hanging from `line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldLabel {
    pub row: u16,
    pub start: u16,
    pub end: u16,
    pub line: usize,
}

impl fmt::Debug for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("View")
            .field("id", &self.id)
            .field("area", &self.area)
            .field("doc", &self.doc)
            .finish()
    }
}

impl View {
    pub fn new(doc: DocumentId, gutters: GutterConfig) -> Self {
        Self {
            id: ViewId::default(),
            doc,
            area: Rect::default(), // will get calculated upon inserting into tree
            jumps: JumpList::new(Jump {
                doc_id: doc,
                selection: Selection::point(0),
                revision: 0,
            }), // TODO: use actual sel
            docs_access_history: Vec::new(),
            last_modified_docs: [None, None],
            object_selections: Vec::new(),
            gutters,
            diagnostics_handler: DiagnosticsHandler::new(),
            bar: false,
            location_list: Default::default(),
            changelist_walk: None,
            fold_labels: Default::default(),
        }
    }

    pub fn add_to_history(&mut self, id: DocumentId) {
        if let Some(pos) = self.docs_access_history.iter().position(|&doc| doc == id) {
            self.docs_access_history.remove(pos);
        }
        self.docs_access_history.push(id);
    }

    /// The range of lines in the document that the view sees. Lines in folds within the range
    /// are hidden: a fold takes no rows, so the range reaches as far as the rows allow.
    pub fn line_range(&self, doc: &Document) -> std::ops::Range<usize> {
        let text = doc.text().slice(..);
        let first_line = text.char_to_line(doc.view_offset(self.id).anchor.min(text.len_chars()));
        let height = self.inner_height(doc);
        if height == 0 {
            return first_line..first_line;
        }
        let folds = self.fold_annotations(doc);
        let last_line = text.nth_next_folded_line(&folds, first_line, height - 1);

        first_line..(last_line + 1).min(text.len_lines())
    }

    pub fn inner_area(&self, doc: &Document) -> Rect {
        self.area
            .clip_top(self.bar_offset() + self.breadcrumb_offset(doc))
            .clip_left(self.gutter_offset(doc))
            .clip_right(self.scrollbar_offset(doc))
            .clip_bottom(1) // -1 for statusline
    }

    pub fn inner_height(&self, doc: &Document) -> usize {
        self.area
            .clip_top(self.bar_offset() + self.breadcrumb_offset(doc))
            .clip_bottom(1) // -1 for statusline
            .height
            .into()
    }

    pub fn inner_width(&self, doc: &Document) -> u16 {
        self.area
            .clip_left(self.gutter_offset(doc))
            .clip_right(self.scrollbar_offset(doc))
            .width
    }

    pub fn gutters(&self) -> &[GutterType] {
        &self.gutters.layout
    }

    pub fn gutter_offset(&self, doc: &Document) -> u16 {
        let total_width = self
            .gutters
            .layout
            .iter()
            .map(|gutter| gutter.width(self, doc) as u16)
            .sum::<u16>()
            .saturating_add(doc.plugin_gutters_width() as u16);
        if total_width < self.area.width {
            total_width
        } else {
            0
        }
    }

    /// The configured gutter at `column` cells from the window's left edge (`None` in a
    /// plugin's column, or past the gutters).
    pub fn gutter_at(&self, doc: &Document, column: u16) -> Option<GutterType> {
        let mut x: u16 = doc
            .plugin_gutters
            .iter()
            .filter(|gutter| gutter.left)
            .map(|gutter| gutter.width as u16)
            .sum();
        if column < x {
            return None;
        }
        for gutter in self.gutters() {
            let width = gutter.width(self, doc) as u16;
            if column < x + width {
                return Some(*gutter);
            }
            x += width;
        }
        None
    }

    /// Height in rows taken by the plugins' bar (0 or 1), above the breadcrumb bar.
    pub fn bar_offset(&self) -> u16 {
        u16::from(self.bar)
    }

    /// Height in rows taken by the breadcrumb bar (0 or 1).
    pub fn breadcrumb_offset(&self, doc: &Document) -> u16 {
        u16::from(doc.config.load().breadcrumb.enable)
    }

    /// Columns taken by the scrollbar on the right (0 or 1): taken while it is on for the
    /// buffer, drawn or not (in another view with `show-in-active-only`, with the whole buffer
    /// on screen...), so that the text keeps its width. Not in a view too narrow for text
    /// beside it.
    pub fn scrollbar_offset(&self, doc: &Document) -> u16 {
        let shown = crate::scrollbar::applies(&doc.config.load().scrollbar, doc);
        u16::from(shown && self.area.width > self.gutter_offset(doc) + 1)
    }

    //
    pub fn offset_coords_to_in_view(
        &self,
        doc: &Document,
        scrolloff: ScrolloffConfig,
    ) -> Option<ViewPosition> {
        self.offset_coords_to_in_view_center::<false>(doc, scrolloff)
    }

    pub fn offset_coords_to_in_view_center<const CENTERING: bool>(
        &self,
        doc: &Document,
        scrolloff: ScrolloffConfig,
    ) -> Option<ViewPosition> {
        let view_offset = doc.get_view_offset(self.id)?;
        let doc_text = doc.text().slice(..);
        let viewport = self.inner_area(doc);
        let vertical_viewport_end = view_offset.vertical_offset + viewport.height as usize;
        let text_fmt = doc.text_format(viewport.width, None);
        let annotations = self.text_annotations(doc, None);

        let (scrolloff_top, scrolloff_bottom) = if CENTERING {
            (0, 0)
        } else {
            (
                // - 1 from the top so we have at least one gap in the middle.
                scrolloff
                    .vertical
                    .min(viewport.height.saturating_sub(1) as usize / 2),
                scrolloff.vertical.min(viewport.height as usize / 2),
            )
        };
        let (scrolloff_left, scrolloff_right) = if CENTERING {
            (0, 0)
        } else {
            (
                // - 1 from the left so we have at least one gap in the middle.
                scrolloff
                    .horizontal
                    .min(viewport.width.saturating_sub(1) as usize / 2),
                scrolloff.horizontal.min(viewport.width as usize / 2),
            )
        };

        let cursor = doc.selection(self.id).primary().cursor(doc_text);
        let mut offset = view_offset;
        let off = visual_offset_from_anchor(
            doc_text,
            offset.anchor,
            cursor,
            &text_fmt,
            &annotations,
            vertical_viewport_end,
        );

        let (new_anchor, at_top) = match off {
            Ok((visual_pos, _)) if visual_pos.row < scrolloff_top + offset.vertical_offset => {
                if CENTERING {
                    // cursor out of view
                    return None;
                }
                (true, true)
            }
            Ok((visual_pos, _)) if visual_pos.row + scrolloff_bottom >= vertical_viewport_end => {
                (true, false)
            }
            Ok((_, _)) => (false, false),
            Err(_) if CENTERING => return None,
            Err(PosBeforeAnchorRow) => (true, true),
            Err(PosAfterMaxRow) => (true, false),
        };

        if new_anchor {
            let v_off = if at_top {
                scrolloff_top as isize
            } else {
                viewport.height as isize - scrolloff_bottom as isize - 1
            };
            (offset.anchor, offset.vertical_offset) =
                char_idx_at_visual_offset(doc_text, cursor, -v_off, 0, &text_fmt, &annotations);
        }

        if text_fmt.soft_wrap {
            offset.horizontal_offset = 0;
        } else {
            // determine the current visual column of the text
            let col = off
                .unwrap_or_else(|_| {
                    visual_offset_from_block(
                        doc_text,
                        offset.anchor,
                        cursor,
                        &text_fmt,
                        &annotations,
                    )
                })
                .0
                .col;

            let last_col = offset.horizontal_offset + viewport.width.saturating_sub(1) as usize;
            if col > last_col.saturating_sub(scrolloff_right) {
                // scroll right
                offset.horizontal_offset += col - (last_col.saturating_sub(scrolloff_right))
            } else if col < offset.horizontal_offset + scrolloff_left {
                // scroll left
                offset.horizontal_offset = col.saturating_sub(scrolloff_left)
            };
        }

        // if we are not centering return None if view position is unchanged
        if !CENTERING && offset == view_offset {
            return None;
        }

        Some(offset)
    }

    pub fn ensure_cursor_in_view(&self, doc: &mut Document, scrolloff: ScrolloffConfig) {
        if let Some(offset) = self.offset_coords_to_in_view_center::<false>(doc, scrolloff) {
            doc.set_view_offset(self.id, offset);
        }
    }

    pub fn ensure_cursor_in_view_center(&self, doc: &mut Document, scrolloff: ScrolloffConfig) {
        if let Some(offset) = self.offset_coords_to_in_view_center::<true>(doc, scrolloff) {
            doc.set_view_offset(self.id, offset);
        } else {
            align_view(doc, self, Align::Center);
        }
    }

    pub fn is_cursor_in_view(&mut self, doc: &Document, scrolloff: ScrolloffConfig) -> bool {
        self.offset_coords_to_in_view(doc, scrolloff).is_none()
    }

    /// Estimates the last visible document line on screen.
    /// This estimate is an upper bound obtained by calculating the first
    /// visible line and adding the viewport height.
    /// The actual last visible line may be smaller if softwrapping occurs
    /// or virtual text lines are visible
    #[inline]
    pub fn estimate_last_doc_line(&self, annotations: &TextAnnotations, doc: &Document) -> usize {
        let doc_text = doc.text().slice(..);
        let line = doc_text.char_to_line(doc.view_offset(self.id).anchor.min(doc_text.len_chars()));
        doc_text.nth_next_folded_line(
            &annotations.folds,
            line,
            self.inner_height(doc).saturating_sub(1),
        )
    }

    /// Calculates the last non-empty visual line on screen
    #[inline]
    pub fn last_visual_line(&self, doc: &Document) -> usize {
        let doc_text = doc.text().slice(..);
        let viewport = self.inner_area(doc);
        let text_fmt = doc.text_format(viewport.width, None);
        let annotations = self.text_annotations(doc, None);
        let view_offset = doc.view_offset(self.id);

        // last visual line in view is trivial to compute
        let visual_height = doc.view_offset(self.id).vertical_offset + viewport.height as usize;

        // fast path when the EOF is not visible on the screen,
        if self.estimate_last_doc_line(&annotations, doc) < doc_text.len_lines() - 1 {
            return visual_height.saturating_sub(1);
        }

        // translate to document line
        let pos = visual_offset_from_anchor(
            doc_text,
            view_offset.anchor,
            usize::MAX,
            &text_fmt,
            &annotations,
            visual_height,
        );

        match pos {
            Ok((Position { row, .. }, _)) => row.saturating_sub(view_offset.vertical_offset),
            Err(PosAfterMaxRow) => visual_height.saturating_sub(1),
            Err(PosBeforeAnchorRow) => 0,
        }
    }

    /// Translates a document position to an absolute position in the terminal.
    /// Returns a (line, col) position if the position is visible on screen.
    // TODO: Could return width as well for the character width at cursor.
    pub fn screen_coords_at_pos(
        &self,
        doc: &Document,
        text: RopeSlice,
        pos: usize,
    ) -> Option<Position> {
        let view_offset = doc.view_offset(self.id);

        let viewport = self.inner_area(doc);
        let text_fmt = doc.text_format(viewport.width, None);
        let annotations = self.text_annotations(doc, None);

        let mut pos = visual_offset_from_anchor(
            text,
            view_offset.anchor,
            pos,
            &text_fmt,
            &annotations,
            viewport.height as usize,
        )
        .ok()?
        .0;
        if pos.row < view_offset.vertical_offset {
            return None;
        }
        pos.row -= view_offset.vertical_offset;
        if pos.row >= viewport.height as usize {
            return None;
        }
        pos.col = pos.col.saturating_sub(view_offset.horizontal_offset);

        Some(pos)
    }

    /// Get the text annotations to display in the current view for the given document and theme.
    pub fn text_annotations<'a>(
        &self,
        doc: &'a Document,
        theme: Option<&Theme>,
    ) -> TextAnnotations<'a> {
        let mut text_annotations = TextAnnotations::default();

        if let Some(labels) = doc.jump_labels.get(&self.id) {
            let style = theme.and_then(|t| t.find_highlight("ui.virtual.jump-label"));
            text_annotations.add_overlay(labels, style);
        }

        if let Some(DocumentInlayHints {
            id: _,
            type_inlay_hints,
            parameter_inlay_hints,
            other_inlay_hints,
            padding_before_inlay_hints,
            padding_after_inlay_hints,
        }) = doc.inlay_hints.get(&self.id)
        {
            let type_style = theme.and_then(|t| t.find_highlight("ui.virtual.inlay-hint.type"));
            let parameter_style =
                theme.and_then(|t| t.find_highlight("ui.virtual.inlay-hint.parameter"));
            let other_style = theme.and_then(|t| t.find_highlight("ui.virtual.inlay-hint"));

            // Overlapping annotations are ignored apart from the first so the order here is not random:
            // types -> parameters -> others should hopefully be the "correct" order for most use cases,
            // with the padding coming before and after as expected.
            text_annotations
                .add_inline_annotations(padding_before_inlay_hints, None)
                .add_inline_annotations(type_inlay_hints, type_style)
                .add_inline_annotations(parameter_inlay_hints, parameter_style)
                .add_inline_annotations(other_inlay_hints, other_style)
                .add_inline_annotations(padding_after_inlay_hints, None);
        };
        let config = doc.config.load();

        if config.lsp.display_color_swatches {
            if let Some(DocumentColorSwatches {
                color_swatches,
                colors,
                color_swatches_padding,
            }) = &doc.color_swatches
            {
                for (color_swatch, color) in color_swatches.iter().zip(colors) {
                    text_annotations
                        .add_inline_annotations(std::slice::from_ref(color_swatch), Some(*color));
                }

                text_annotations.add_inline_annotations(color_swatches_padding, None);
            }
        }

        if config.debugger.inline_values
            && !doc.dap_inline_values.is_empty()
            // The values belong to the saved file.
            && !doc.is_modified()
        {
            let style = theme.and_then(|t| {
                t.find_highlight("ui.virtual.inline-value")
                    .or_else(|| t.find_highlight("ui.virtual.inlay-hint"))
            });
            text_annotations.add_inline_annotations(&doc.dap_inline_values, style);
        }

        let width = self.inner_width(doc);
        let enable_cursor_line = self
            .diagnostics_handler
            .show_cursorline_diagnostics(doc, self.id);
        let config = config.inline_diagnostics.prepare(width, enable_cursor_line);
        if config.show_virtual_lines && !config.disabled() {
            let cursor = doc
                .selection(self.id)
                .primary()
                .cursor(doc.text().slice(..));
            text_annotations.add_line_annotation(InlineDiagnostics::new(
                doc,
                cursor,
                width,
                doc.view_offset(self.id).horizontal_offset,
                config,
            ));
        }

        text_annotations
            .add_line_annotation(Box::new(PluginLineAnnotations::new(doc, self.id, width)));

        if let Some(completion) = doc.inline_completion(self.id) {
            let text = doc.text().slice(..);
            let line = text.char_to_line(completion.cursor);
            let line_end = helix_core::line_ending::line_end_char_index(&text, line);
            text_annotations.add_line_annotation(Box::new(InlineCompletionLines::new(
                completion.cursor,
                &completion.lines,
                doc.text_format(width, theme),
                line_end,
            )));
        }

        if let Some(fold_container) = doc.fold_container(self.id) {
            text_annotations.add_folds(fold_container);
        }

        text_annotations
    }

    pub fn fold_annotations<'a>(&self, doc: &'a Document) -> FoldAnnotations<'a> {
        FoldAnnotations::new(doc.fold_container(self.id))
    }

    pub fn text_pos_at_screen_coords(
        &self,
        doc: &Document,
        row: u16,
        column: u16,
        fmt: TextFormat,
        annotations: &TextAnnotations,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        let inner = self.inner_area(doc);
        // 1 for status
        if row < inner.top() || row >= inner.bottom() {
            return None;
        }

        if column < inner.left() || column > inner.right() {
            return None;
        }

        self.text_pos_at_visual_coords(
            doc,
            row - inner.y,
            column - inner.x,
            fmt,
            annotations,
            ignore_virtual_text,
        )
    }

    pub fn text_pos_at_visual_coords(
        &self,
        doc: &Document,
        row: u16,
        column: u16,
        text_fmt: TextFormat,
        annotations: &TextAnnotations,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        let text = doc.text().slice(..);
        let view_offset = doc.view_offset(self.id);

        let text_row = row as usize + view_offset.vertical_offset;
        let text_col = column as usize + view_offset.horizontal_offset;

        let (char_idx, virt_lines) = char_idx_at_visual_offset(
            text,
            view_offset.anchor,
            text_row as isize,
            text_col,
            &text_fmt,
            annotations,
        );

        // if the cursor is on a line with only virtual text return None
        if virt_lines != 0 && ignore_virtual_text {
            return None;
        }
        Some(char_idx)
    }

    /// Translates a screen position to position in the text document.
    /// Returns a usize typed position in bounds of the text if found in this view, None if out of view.
    pub fn pos_at_screen_coords(
        &self,
        doc: &Document,
        row: u16,
        column: u16,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        self.text_pos_at_screen_coords(
            doc,
            row,
            column,
            doc.text_format(self.inner_width(doc), None),
            &self.text_annotations(doc, None),
            ignore_virtual_text,
        )
    }

    pub fn pos_at_visual_coords(
        &self,
        doc: &Document,
        row: u16,
        column: u16,
        ignore_virtual_text: bool,
    ) -> Option<usize> {
        self.text_pos_at_visual_coords(
            doc,
            row,
            column,
            doc.text_format(self.inner_width(doc), None),
            &self.text_annotations(doc, None),
            ignore_virtual_text,
        )
    }

    /// Translates screen coordinates into coordinates on the gutter of the view.
    /// Returns a tuple of usize typed line and column numbers starting with 0.
    /// Returns None if coordinates are not on the gutter.
    pub fn gutter_coords_at_screen_coords(&self, row: u16, column: u16) -> Option<Position> {
        // 1 for status
        if row < self.area.top() || row >= self.area.bottom() {
            return None;
        }

        if column < self.area.left() || column > self.area.right() {
            return None;
        }

        Some(Position::new(
            (row - self.area.top()) as usize,
            (column - self.area.left()) as usize,
        ))
    }

    pub fn remove_document(&mut self, doc_id: &DocumentId) {
        self.jumps.remove(doc_id);
        self.docs_access_history.retain(|doc| doc != doc_id);
    }

    // pub fn traverse<F>(&self, text: RopeSlice, start: usize, end: usize, fun: F)
    // where
    //     F: Fn(usize, usize),
    // {
    //     let start = self.screen_coords_at_pos(text, start);
    //     let end = self.screen_coords_at_pos(text, end);

    //     match (start, end) {
    //         // fully on screen
    //         (Some(start), Some(end)) => {
    //             // we want to calculate ends of lines for each char..
    //         }
    //         // from start to end of screen
    //         (Some(start), None) => {}
    //         // from start of screen to end
    //         (None, Some(end)) => {}
    //         // not on screen
    //         (None, None) => return,
    //     }
    // }

    /// Adds the current selection of `doc` to the jumplist. Pending changes are committed to
    /// the document's history first since a jump refers to a history revision.
    pub fn push_jump(&mut self, doc: &mut Document) {
        doc.append_changes_to_history(self);
        let jump = Jump::new(doc, doc.selection(self.id).clone());
        self.jumps.push_impl(jump);
    }

    /// Like [`View::push_jump`] but for a selection other than the current one.
    pub fn push_jump_to(&mut self, doc: &mut Document, selection: Selection) {
        doc.append_changes_to_history(self);
        self.jumps.push_impl(Jump::new(doc, selection));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use arc_swap::ArcSwap;
    use helix_core::{syntax, Rope, Transaction};

    // 1 diagnostic + 1 spacer + 3 linenr (< 1000 lines) + 1 spacer + 1 diff
    const DEFAULT_GUTTER_OFFSET: u16 = 7;

    // 1 diagnostics + 1 spacer + 1 gutter
    const DEFAULT_GUTTER_OFFSET_ONLY_DIAGNOSTICS: u16 = 3;

    use crate::document::Document;
    use crate::editor::{Config, GutterConfig, GutterLineNumbersConfig, GutterType};

    /// The default gutters before the fold column was added: these tests' positions count
    /// on them.
    fn five_gutters() -> GutterConfig {
        let mut gutters = GutterConfig::default();
        gutters.layout.retain(|gutter| *gutter != GutterType::Folds);
        assert_eq!(gutters.layout.len(), 5_usize);
        gutters
    }

    #[test]
    fn test_text_pos_at_screen_coords() {
        let mut view = View::new(DocumentId::default(), five_gutters());
        view.area = Rect::new(40, 40, 40, 40);
        let rope = Rope::from_str("abc\n\tdef");
        let mut doc = Document::from(
            rope,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.ensure_view_init(view.id);

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                2,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                41,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                0,
                2,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                0,
                49,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                0,
                41,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                81,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                78,
                41,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            None
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 3,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(3)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                80,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(3)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                40 + DEFAULT_GUTTER_OFFSET + 1,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(4)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                40 + DEFAULT_GUTTER_OFFSET + 4,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(5)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                40 + DEFAULT_GUTTER_OFFSET + 7,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(8)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                80,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(8)
        );
    }

    #[test]
    fn test_text_pos_at_screen_coords_without_line_numbers_gutter() {
        let mut view = View::new(
            DocumentId::default(),
            GutterConfig {
                layout: vec![GutterType::Diagnostics],
                line_numbers: GutterLineNumbersConfig::default(),
            },
        );
        view.area = Rect::new(40, 40, 40, 40);
        let rope = Rope::from_str("abc\n\tdef");
        let mut doc = Document::from(
            rope,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.ensure_view_init(view.id);
        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                40 + DEFAULT_GUTTER_OFFSET_ONLY_DIAGNOSTICS + 1,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(4)
        );
    }

    #[test]
    fn test_text_pos_at_screen_coords_without_any_gutters() {
        let mut view = View::new(
            DocumentId::default(),
            GutterConfig {
                layout: vec![],
                line_numbers: GutterLineNumbersConfig::default(),
            },
        );
        view.area = Rect::new(40, 40, 40, 40);
        let rope = Rope::from_str("abc\n\tdef");
        let mut doc = Document::from(
            rope,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.ensure_view_init(view.id);
        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                41,
                40 + 1,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(4)
        );
    }

    #[test]
    fn test_text_pos_at_screen_coords_cjk() {
        let mut view = View::new(DocumentId::default(), five_gutters());
        view.area = Rect::new(40, 40, 40, 40);
        let rope = Rope::from_str("Hi! こんにちは皆さん");
        let mut doc = Document::from(
            rope,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.ensure_view_init(view.id);

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(0)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 4,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(4)
        );
        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 5,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(4)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 6,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(5)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 7,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(5)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 8,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(6)
        );
    }

    #[test]
    fn test_text_pos_at_screen_coords_graphemes() {
        let mut view = View::new(DocumentId::default(), five_gutters());
        view.area = Rect::new(40, 40, 40, 40);
        let rope = Rope::from_str("Hèl̀l̀ò world!");
        let mut doc = Document::from(
            rope,
            None,
            Arc::new(ArcSwap::new(Arc::new(Config::default()))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.ensure_view_init(view.id);

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(0)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 1,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(1)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 2,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(3)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 3,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(5)
        );

        assert_eq!(
            view.text_pos_at_screen_coords(
                &doc,
                40,
                40 + DEFAULT_GUTTER_OFFSET + 4,
                TextFormat::default(),
                &TextAnnotations::default(),
                true
            ),
            Some(7)
        );
    }

    fn jump(doc_id: DocumentId, pos: usize) -> Jump {
        Jump {
            doc_id,
            selection: Selection::point(pos),
            revision: 0,
        }
    }

    #[test]
    fn jumplist_remove_adjusts_current() {
        let doc_a = DocumentId::new(1);
        let doc_b = DocumentId::new(2);

        let entries = |jumps: &JumpList| -> Vec<DocumentId> {
            jumps.iter().map(|jump| jump.doc_id).collect()
        };
        let make = |current: usize| -> JumpList {
            let mut jumps = JumpList::new(jump(doc_a, 0));
            jumps.jumps = [
                jump(doc_a, 0),
                jump(doc_b, 1),
                jump(doc_a, 2),
                jump(doc_b, 3),
                jump(doc_a, 4),
            ]
            .into_iter()
            .collect();
            jumps.current = current;
            jumps
        };

        let mut jumps = make(3);
        jumps.remove(&doc_a);
        assert_eq!(entries(&jumps), vec![doc_b, doc_b]);
        assert_eq!(jumps.current, 1);
        assert_eq!(jumps.iter().nth(jumps.current), Some(&jump(doc_b, 3)));

        let mut jumps = make(5);
        jumps.remove(&doc_a);
        assert_eq!(jumps.current, 2);

        let mut jumps = make(3);
        jumps.remove(&doc_a);
        jumps.remove(&doc_b);
        assert_eq!(jumps.current, 0);

        let mut jumps = make(0);
        jumps.remove(&doc_a);
        assert_eq!(jumps.current, 0);
        assert_eq!(entries(&jumps), vec![doc_b, doc_b]);
    }

    #[test]
    fn jumps_follow_document_changes() {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut doc = Document::from(Rope::from_str("ab"), None, config, loader);
        let mut view1 = View::new(doc.id(), GutterConfig::default());
        let mut view2 = View::new(doc.id(), GutterConfig::default());
        doc.ensure_view_init(view1.id);
        doc.ensure_view_init(view2.id);

        view1.push_jump_to(&mut doc, Selection::point(2));
        let jump_pos = |view: &View, doc: &Document| {
            view.jumps
                .iter()
                .next_back()
                .unwrap()
                .selection(doc)
                .primary()
                .head
        };

        // Edits through another view.
        let insert = |doc: &Document, text: &str| {
            Transaction::change(doc.text(), std::iter::once((0, 0, Some(text.into()))))
        };
        doc.apply(&insert(&doc, "XXXX"), view2.id);
        doc.append_changes_to_history(&mut view2);
        assert_eq!(jump_pos(&view1, &doc), 6);

        // Changes that are not committed yet.
        doc.apply(&insert(&doc, "YY"), view2.id);
        assert_eq!(jump_pos(&view1, &doc), 8);

        // Undo.
        doc.append_changes_to_history(&mut view2);
        doc.undo(&mut view2);
        assert_eq!(jump_pos(&view1, &doc), 6);
    }
}
