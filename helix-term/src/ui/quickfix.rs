//! The quickfix panel: a list of locations (search results, diagnostics, references, symbols,
//! ...) pinned at the bottom of the editor, under the views, like Vim's quickfix window.
//!
//! With `[editor.quickfix] enable`, the lists of locations open in it instead of in a picker:
//! the panel is a picker in its docked mode (see `Picker::docked`), so the lists keep their
//! columns, filtering, live updates and preview. It stays open while you visit its entries:
//! `Enter` jumps and gives the focus to the code, `Ctrl-w j` (from the bottom view) gives it
//! back, `]q`/`[q` visit the next and previous entry from anywhere, and `q` closes it.

use helix_view::{graphics::Rect, Editor};

use crate::compositor::{Component, Compositor};
use crate::ui::{overlay::overlaid, picker::Picker};

/// The id of the panel's layer.
pub const ID: &str = "quickfix";

/// The fewest rows the panel is drawn with: its border, the query, the separator and a row.
const MIN_HEIGHT: u16 = 5;
/// The fewest rows the panel leaves to the views.
const MIN_VIEW_ROWS: u16 = 4;

/// The rows the panel takes at the bottom of the views' `frame` (see `ui::editor::frame`):
/// `[editor.quickfix] height`, or less on a short screen; none when hidden, or when not even
/// [`MIN_HEIGHT`] rows are left for it.
pub fn height(editor: &Editor, frame: Rect) -> u16 {
    if !editor.quickfix_panel.visible {
        return 0;
    }
    let height = editor
        .config()
        .quickfix
        .height
        .min(frame.height.saturating_sub(MIN_VIEW_ROWS));
    if height < MIN_HEIGHT {
        0
    } else {
        height
    }
}

/// The views' area in their `frame`: above the panel.
pub fn layout(editor: &Editor, frame: Rect) -> Rect {
    frame.clip_bottom(height(editor, frame))
}

/// The panel's area: the bottom rows of the views' `frame`.
pub fn panel_area(editor: &Editor, frame: Rect) -> Rect {
    let height = height(editor, frame);
    Rect::new(
        frame.x,
        frame.bottom().saturating_sub(height),
        frame.width,
        height,
    )
}

/// Whether the keys go to the panel: it has the focus, and the explorer does not have it.
pub fn focused(editor: &Editor) -> bool {
    editor.quickfix_panel.visible && editor.quickfix_panel.focused && !editor.explorer_panel.focused
}

/// Gives the focus to the panel (from the code or the explorer).
pub fn focus(editor: &mut Editor) {
    if editor.quickfix_panel.visible {
        editor.quickfix_panel.focused = true;
        editor.explorer_panel.focused = false;
    }
}

/// Shows `picker`, a list of locations: in the panel with `[editor.quickfix] enable`, as a
/// picker otherwise.
pub fn show<T: Send + Sync + 'static, D: Send + Sync + 'static>(
    compositor: &mut Compositor,
    editor: &mut Editor,
    picker: Picker<T, D>,
) {
    if editor.config().quickfix.enable {
        dock(compositor, editor, picker);
    } else {
        compositor.push(Box::new(overlaid(
            picker,
            editor.config().fullscreen_overlay,
        )));
    }
}

/// [`show`] from a command.
pub fn show_from_command<T: Send + Sync + 'static, D: Send + Sync + 'static>(
    cx: &mut crate::commands::Context,
    picker: Picker<T, D>,
) {
    cx.callback.push(Box::new(move |compositor, cx| {
        show(compositor, cx.editor, picker)
    }));
}

/// Opens `picker` in the panel (replacing the list it shows), with the focus.
pub fn dock<T: Send + Sync + 'static, D: Send + Sync + 'static>(
    compositor: &mut Compositor,
    editor: &mut Editor,
    picker: Picker<T, D>,
) {
    open(compositor, editor, Box::new(picker.docked()));
}

/// Shows `panel` (a docked picker) in the panel, with the focus. Under the layers above the
/// editor (pickers, popups, the floating explorer): it is part of the editor's frame.
fn open(compositor: &mut Compositor, editor: &mut Editor, panel: Box<dyn Component>) {
    compositor.remove(ID);
    editor.quickfix_panel.visible = true;
    focus(editor);
    compositor.insert_above_editor(panel);
}

/// Closes the panel; [`reopen`] shows it again.
pub fn close(compositor: &mut Compositor, editor: &mut Editor) {
    editor.quickfix_panel = Default::default();
    if let Some(panel) = compositor.remove(ID) {
        compositor.last_quickfix = Some(panel);
    }
}

/// Shows the panel again (`:copen`): focuses it when it is shown, reopens the last list it
/// showed, or else shows the quicklist. Whether `[editor.quickfix] enable` or not: asked for.
pub fn reopen(compositor: &mut Compositor, editor: &mut Editor) {
    if editor.quickfix_panel.visible {
        focus(editor);
    } else if let Some(panel) = compositor.last_quickfix.take() {
        open(compositor, editor, panel);
    } else if let Some(picker) = crate::commands::quicklist::picker(editor) {
        dock(compositor, editor, picker);
    } else {
        editor.set_status("The quicklist is empty, add locations with Alt-q or Ctrl-q in a picker");
    }
}
