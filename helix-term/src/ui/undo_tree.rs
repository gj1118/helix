//! The undo tree (`:undo-tree`, `Space u`): every revision of the buffer's history, branches
//! included, in a panel at the right. Moving through it turns the buffer into that revision,
//! so the text it held shows behind the panel; `Enter` keeps it, `Esc` goes back to where it
//! started.

use std::time::{Duration, Instant};

use helix_core::history::{RevisionSummary, TreeRow};
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::{
    graphics::{Margin, Rect},
    input::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind},
    DocumentId, Editor, ViewId,
};
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::gradient_border::PanelBorder;

pub const ID: &str = "undo-tree";

/// The glyph of the revision shown in the buffer, and of the others.
const SHOWN: &str = "●";
const OTHER: &str = "○";

pub struct UndoTree {
    doc_id: DocumentId,
    view_id: ViewId,
    rows: Vec<TreeRow>,
    /// The row under the cursor, whose revision the buffer shows.
    cursor: usize,
    /// The first row drawn.
    scroll: usize,
    /// The revision the buffer showed when the tree opened, for `Esc`.
    start: usize,
    /// Where the rows were drawn, for the mouse.
    list_area: Rect,
    border: PanelBorder,
}

impl UndoTree {
    /// The undo tree of the buffer in `view_id`, with the cursor on the revision it shows.
    pub fn new(editor: &mut Editor, doc_id: DocumentId, view_id: ViewId) -> Option<Self> {
        if !editor.tree.contains(view_id) {
            return None;
        }
        let view = editor.tree.get_mut(view_id);
        let doc = editor.documents.get_mut(&doc_id)?;
        doc.append_changes_to_history(view);
        let history = doc.history.take();
        let rows = history.tree_rows();
        let start = history.current_revision();
        doc.history.set(history);
        let cursor = rows
            .iter()
            .position(|row| row.revision == start)
            .unwrap_or(0);
        Some(Self {
            doc_id,
            view_id,
            rows,
            cursor,
            scroll: 0,
            start,
            list_area: Rect::default(),
            border: PanelBorder::default(),
        })
    }

    /// The revision under the cursor.
    pub fn selected_revision(&self) -> Option<usize> {
        self.rows.get(self.cursor).map(|row| row.revision)
    }

    /// Turns the buffer into `revision`, with its cursor in view. `false` when the buffer or
    /// its window is gone.
    fn show(editor: &mut Editor, doc_id: DocumentId, view_id: ViewId, revision: usize) -> bool {
        let scrolloff = editor.config().scrolloff;
        if !editor.tree.contains(view_id) {
            return false;
        }
        let view = editor.tree.get_mut(view_id);
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            return false;
        };
        doc.jump_to_revision(view, revision);
        view.ensure_cursor_in_view(doc, scrolloff);
        true
    }

    fn move_to(&mut self, row: usize, editor: &mut Editor) -> EventResult {
        if self.rows.is_empty() {
            return EventResult::Consumed(None);
        }
        self.cursor = row.min(self.rows.len() - 1);
        let revision = self.rows[self.cursor].revision;
        if Self::show(editor, self.doc_id, self.view_id, revision) {
            EventResult::Consumed(None)
        } else {
            close()
        }
    }

    fn move_by(&mut self, delta: isize, editor: &mut Editor) -> EventResult {
        let row = self.cursor.saturating_add_signed(delta);
        self.move_to(row, editor)
    }

    fn half_page(&self) -> isize {
        (self.list_area.height as isize / 2).max(1)
    }

    fn on_key(&mut self, key: KeyEvent, editor: &mut Editor) -> EventResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if !ctrl => self.move_by(1, editor),
            KeyCode::Char('n') if ctrl => self.move_by(1, editor),
            KeyCode::Up | KeyCode::Char('k') if !ctrl => self.move_by(-1, editor),
            KeyCode::Char('p') if ctrl => self.move_by(-1, editor),
            KeyCode::Char('d') if ctrl => self.move_by(self.half_page(), editor),
            KeyCode::Char('u') if ctrl => self.move_by(-self.half_page(), editor),
            KeyCode::PageDown => self.move_by(self.half_page() * 2, editor),
            KeyCode::PageUp => self.move_by(-self.half_page() * 2, editor),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0, editor),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX, editor),
            // the revision the buffer was in when the tree opened
            KeyCode::Char('o') => {
                let start = self
                    .rows
                    .iter()
                    .position(|row| row.revision == self.start)
                    .unwrap_or(0);
                self.move_to(start, editor)
            }
            KeyCode::Enter => close(),
            KeyCode::Esc | KeyCode::Char('q') => {
                Self::show(editor, self.doc_id, self.view_id, self.start);
                close()
            }
            KeyCode::Char('c') if ctrl => {
                Self::show(editor, self.doc_id, self.view_id, self.start);
                close()
            }
            // The tree has the keys while it is open.
            _ => EventResult::Consumed(None),
        }
    }

    /// The panel: at the right of `area`, the whole height but the status line.
    fn panel(area: Rect) -> Rect {
        let width = (area.width * 2 / 5).clamp(30.min(area.width), 64.min(area.width));
        Rect::new(
            area.x + area.width - width,
            area.y,
            width,
            area.height.saturating_sub(1),
        )
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

fn close() -> EventResult {
    EventResult::Consumed(Some(Box::new(|compositor, _| {
        compositor.remove(ID);
    })))
}

/// `3s`, `5m`, `2h`, `4d`: how long ago.
fn age(since: Duration) -> String {
    let secs = since.as_secs();
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// `+3 −1 lines`, `+12 −4`: what a revision changed, short.
fn change(summary: &RevisionSummary) -> String {
    let parts = |plus: usize, minus: usize| -> String {
        let mut parts = Vec::new();
        if plus > 0 {
            parts.push(format!("+{plus}"));
        }
        if minus > 0 {
            parts.push(format!("−{minus}"));
        }
        parts.join(" ")
    };
    if summary.inserted_lines + summary.deleted_lines > 0 {
        let lines = summary.inserted_lines + summary.deleted_lines;
        let unit = if lines == 1 { "line" } else { "lines" };
        format!(
            "{} {unit}",
            parts(summary.inserted_lines, summary.deleted_lines)
        )
    } else {
        parts(summary.inserted_chars, summary.deleted_chars)
    }
}

impl Component for UndoTree {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.on_key(*key, cx.editor),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => self.move_by(1, cx.editor),
                MouseEventKind::ScrollUp => self.move_by(-1, cx.editor),
                MouseEventKind::Down(_) if contains(self.list_area, mouse.column, mouse.row) => {
                    let row = self.scroll + (mouse.row - self.list_area.y) as usize;
                    self.move_to(row, cx.editor)
                }
                _ => EventResult::Ignored(None),
            },
            _ => EventResult::Ignored(None),
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let Some(saved) = cx
            .editor
            .documents
            .get_mut(&self.doc_id)
            .map(|doc| doc.get_last_saved_revision())
        else {
            return;
        };
        let editor = &*cx.editor;
        let Some(doc) = editor.documents.get(&self.doc_id) else {
            return;
        };
        let history = doc.history.take();
        let shown = history.current_revision();
        let now = Instant::now();

        let theme = &editor.theme;
        let background = theme.get("ui.popup");
        let text_style = theme.get("ui.text");
        let dim = theme.get("ui.text.inactive");
        let guide = theme.try_get("ui.virtual.indent-guide").unwrap_or(dim);
        let selected = theme.get("ui.menu.selected");
        let shown_style = theme.get("ui.text.focus");
        let border_style = crate::ui::border_style(theme);

        let rect = Self::panel(area);
        crate::ui::clear_bordered(
            surface,
            rect,
            background,
            !crate::ui::border_cells_take_background(&editor.config()),
        );
        self.border.render(rect, surface, editor);
        let inner = rect.inner(Margin::all(1));
        let title = format!(" Undo tree · {} revisions ", history.revision_count());
        surface.set_stringn(
            rect.x + 2,
            rect.y,
            &title,
            rect.width.saturating_sub(4) as usize,
            border_style,
        );
        let hint = " j/k move · Enter keep · Esc back ";
        if hint.width() + 4 <= rect.width as usize {
            surface.set_string(
                rect.x + rect.width - 2 - hint.width() as u16,
                rect.y + rect.height.saturating_sub(1),
                hint,
                border_style,
            );
        }
        // the rows, then a separator and two lines on the revision under the cursor
        if inner.height < 4 || inner.width < 10 {
            doc.history.set(history);
            return;
        }
        let list = Rect::new(inner.x, inner.y, inner.width, inner.height - 3);
        self.list_area = list;
        let height = list.height as usize;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }

        let width = list.width as usize;
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(height) {
            let y = list.y + (i - self.scroll) as u16;
            let is_cursor = i == self.cursor;
            if is_cursor {
                surface.set_style(Rect::new(list.x, y, list.width, 1), selected);
            }
            let patch = |style: helix_view::graphics::Style| {
                if is_cursor {
                    style.patch(selected)
                } else {
                    style
                }
            };
            let mut x = list.x + 1;
            let right = list.x + list.width - 1;
            let (next, _) = surface.set_stringn(x, y, &row.prefix, width, patch(guide));
            x = next;
            let (marker, marker_style) = if row.revision == shown {
                (SHOWN, shown_style)
            } else {
                (OTHER, dim)
            };
            let (next, _) =
                surface.set_stringn(x, y, marker, (right - x) as usize, patch(marker_style));
            x = next + 1;
            let ago = history
                .timestamp(row.revision)
                .map(|time| age(now.saturating_duration_since(time)))
                .unwrap_or_default();
            let label = if row.revision == 0 {
                "original".to_string()
            } else {
                format!(
                    "#{} {}",
                    row.revision,
                    change(&history.summary(row.revision))
                )
            };
            let tag = if row.revision == saved { " saved" } else { "" };
            let room = (right.saturating_sub(x) as usize).saturating_sub(ago.width() + 1);
            let (next, _) = surface.set_stringn(x, y, &label, room, patch(text_style));
            if !tag.is_empty() {
                surface.set_stringn(next, y, tag, room.saturating_sub(label.width()), patch(dim));
            }
            if ago.width() < right.saturating_sub(x) as usize {
                surface.set_string(right - ago.width() as u16, y, &ago, patch(dim));
            }
        }

        // the revision under the cursor: what it changed, and when
        let separator_y = list.y + list.height;
        surface.set_string(
            inner.x,
            separator_y,
            &"─".repeat(inner.width as usize),
            border_style,
        );
        if let Some(row) = self.rows.get(self.cursor) {
            let summary = history.summary(row.revision);
            let ago = history
                .timestamp(row.revision)
                .map(|time| age(now.saturating_duration_since(time)))
                .unwrap_or_default();
            let head = if row.revision == 0 {
                "The text before the first edit".to_string()
            } else {
                let changed = change(&summary);
                let changed = if changed.is_empty() {
                    "no text changed".to_string()
                } else {
                    changed
                };
                format!(
                    "#{} · {changed} · {}",
                    row.revision,
                    if ago == "now" {
                        ago
                    } else {
                        format!("{ago} ago")
                    }
                )
            };
            surface.set_stringn(
                inner.x + 1,
                separator_y + 1,
                &head,
                inner.width.saturating_sub(2) as usize,
                text_style,
            );
            surface.set_stringn(
                inner.x + 1,
                separator_y + 2,
                &summary.excerpt,
                inner.width.saturating_sub(2) as usize,
                dim,
            );
        }
        doc.history.set(history);
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_short() {
        let ages: Vec<String> = [0, 4, 5, 59, 60, 3599, 3600, 86_400 * 3]
            .into_iter()
            .map(|secs| age(Duration::from_secs(secs)))
            .collect();
        assert_eq!(ages, ["now", "now", "5s", "59s", "1m", "59m", "1h", "3d"]);
    }

    #[test]
    fn changes_are_counted_in_lines_when_lines_changed() {
        let summary =
            |inserted_chars, deleted_chars, inserted_lines, deleted_lines| RevisionSummary {
                inserted_chars,
                deleted_chars,
                inserted_lines,
                deleted_lines,
                excerpt: String::new(),
            };
        assert_eq!(change(&summary(5, 2, 0, 0)), "+5 −2".to_string());
        assert_eq!(change(&summary(5, 0, 0, 0)), "+5".to_string());
        assert_eq!(change(&summary(40, 3, 3, 0)), "+3 lines".to_string());
        assert_eq!(change(&summary(4, 10, 0, 1)), "−1 line".to_string());
        assert_eq!(change(&summary(0, 0, 0, 0)), String::new());
    }

    #[test]
    fn the_panel_is_at_the_right_and_fits_small_screens() {
        let panel = UndoTree::panel(Rect::new(0, 0, 100, 40));
        assert_eq!(
            (panel.x, panel.width, panel.height),
            (60_u16, 40_u16, 39_u16)
        );
        let small = UndoTree::panel(Rect::new(0, 0, 20, 10));
        assert_eq!((small.x, small.width), (0_u16, 20_u16));
        let wide = UndoTree::panel(Rect::new(0, 0, 300, 40));
        assert_eq!(wide.width, 64_u16);
    }
}
