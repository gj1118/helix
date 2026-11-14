//! `helix.diff`: the side-by-side diffs. Read them, mark their changes, add keys to them,
//! open and close them. Everything needs the `diff` permission.
//!
//! A diff's two sides are buffers: a plugin draws on their lines with the buffer API
//! (`set_highlights`, `set_signs`, annotations), the old (left, read-only) side too.

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_core::diff_hunks::{DiffHunk, HunkKind};
use helix_view::diff_view::{DiffSession, OldVersion, Stage};
use helix_view::plugin_ui::{ActionPlace, DiffMark, Mark};
use helix_view::{Editor, ViewId};
use mlua::prelude::*;

use super::buffer::LuaBuffer;
use super::window::LuaWindow;

/// A change as a table: `{ index, kind, left_line, left_count, right_line, right_count,
/// stage }`, lines 1-based. A side with no lines has its count 0 and its line where the lines
/// would be.
fn change_table(
    lua: &Lua,
    session: &DiffSession,
    index: usize,
    hunk: &DiffHunk,
) -> LuaResult<LuaTable> {
    let table = lua.create_table()?;
    table.set("index", index + 1)?;
    table.set(
        "kind",
        match hunk.kind() {
            HunkKind::Added => "added",
            HunkKind::Removed => "removed",
            HunkKind::Changed => "changed",
        },
    )?;
    table.set("left_line", hunk.left.start + 1)?;
    table.set("left_count", hunk.left.len())?;
    table.set("right_line", hunk.right.start + 1)?;
    table.set("right_count", hunk.right.len())?;
    let stage = session
        .git
        .as_ref()
        .filter(|git| git.left_is_head)
        .and_then(|git| git.stage(index))
        .map(|stage| match stage {
            Stage::Staged => "staged",
            Stage::Unstaged => "unstaged",
            Stage::Partial => "partly",
        });
    table.set("stage", stage)?;
    Ok(table)
}

/// A diff as a table: `{ left, right, left_window, right_window, title, path, git, changes }`.
pub(crate) fn session_table(
    lua: &Lua,
    editor: &Editor,
    session: &DiffSession,
) -> LuaResult<LuaTable> {
    let table = lua.create_table()?;
    table.set("left", LuaBuffer::new(session.left_doc))?;
    table.set("right", LuaBuffer::new(session.right_doc))?;
    table.set(
        "left_window",
        LuaWindow {
            view_id: session.left_view,
        },
    )?;
    table.set(
        "right_window",
        LuaWindow {
            view_id: session.right_view,
        },
    )?;
    let title = editor
        .documents
        .get(&session.left_doc)
        .and_then(|doc| doc.title.clone());
    table.set("title", title)?;
    table.set(
        "path",
        session
            .right_doc_path(editor)
            .map(|path| path.to_string_lossy().into_owned()),
    )?;
    // against git: `"HEAD"` or `"index"`
    table.set(
        "git",
        session
            .git
            .as_ref()
            .map(|git| if git.left_is_head { "HEAD" } else { "index" }),
    )?;
    let changes = lua.create_table()?;
    for (index, hunk) in session.hunks.iter().enumerate() {
        changes.raw_push(change_table(lua, session, index, hunk)?)?;
    }
    table.set("changes", changes)?;
    Ok(table)
}

/// The change under the cursor of `view`, a side of a diff, as a table.
pub(crate) fn change_under_cursor(
    lua: &Lua,
    editor: &Editor,
    view: ViewId,
) -> LuaResult<Option<LuaTable>> {
    let Some(session) = editor.diff_session(view) else {
        return Ok(None);
    };
    match editor.diff_change_under_cursor(view) {
        Some(index) => Ok(Some(change_table(
            lua,
            session,
            index,
            &session.hunks[index],
        )?)),
        None => Ok(None),
    }
}

/// The view of `window`: a window, a window's id (`window:id()`, the `*_window_id` of the
/// events), `nil` for the focused one.
fn view_of(window: LuaValue, editor: &Editor) -> LuaResult<ViewId> {
    match window {
        LuaValue::Nil => Ok(editor.tree.focus),
        LuaValue::UserData(window) => Ok(window
            .borrow::<LuaWindow>()
            .map_err(|_| LuaError::RuntimeError("expected a window (helix.window...)".into()))?
            .view_id),
        LuaValue::String(id) => {
            let id = id.to_str()?;
            editor
                .tree
                .views()
                .map(|(view, _)| view.id)
                .find(|view| format!("{view:?}") == *id)
                .ok_or_else(|| LuaError::RuntimeError(format!("no window {id}")))
        }
        other => Err(LuaError::RuntimeError(format!(
            "expected a window or a window id (got a value of type {})",
            other.type_name()
        ))),
    }
}

pub fn register_diff_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let diff = lua.create_table()?;

    // helix.diff.get(window) - The diff `window` (default: the focused one) is a side of, or nil
    let get = lua.create_function(|lua, window: LuaValue| {
        require(lua, Permission::Diff, "helix.diff.get")?;
        crate::lua::with_editor(|editor| {
            let view = view_of(window, editor)?;
            match editor.diff_session(view) {
                Some(session) => Ok(Some(session_table(lua, editor, session)?)),
                None => Ok(None),
            }
        })
    })?;
    diff.set("get", get)?;

    // helix.diff.list() - The diffs shown
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Diff, "helix.diff.list")?;
        crate::lua::with_editor(|editor| {
            let list = lua.create_table()?;
            for session in &editor.diff_sessions {
                list.raw_push(session_table(lua, editor, session)?)?;
            }
            Ok(list)
        })
    })?;
    diff.set("list", list)?;

    // helix.diff.change(window) - The change under the cursor of `window` (default: the
    // focused one), a side of a diff, or nil
    let change = lua.create_function(|lua, window: LuaValue| {
        require(lua, Permission::Diff, "helix.diff.change")?;
        crate::lua::with_editor(|editor| {
            let view = view_of(window, editor)?;
            change_under_cursor(lua, editor, view)
        })
    })?;
    diff.set("change", change)?;

    // helix.diff.mark(index, { text, fg, style }, window) - A mark of the calling plugin on
    // change `index` of the diff of `window` (default: the focused one), at the left edge of
    // the connector on the change's first row (`nil`: none). Marks last until the changes
    // are worked out again (`diff_update`).
    let mark = lua.create_function(
        |lua, (index, mark, window): (usize, Option<LuaTable>, LuaValue)| {
            require(lua, Permission::Diff, "helix.diff.mark")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mark = match mark {
                Some(table) => {
                    let text: String = table.raw_get::<Option<String>>("text")?.unwrap_or_default();
                    let (fg, style) = super::explorer::read_color(&table)?;
                    Some(Mark {
                        plugin: plugin.clone(),
                        text,
                        fg,
                        style,
                    })
                }
                None => None,
            };
            crate::lua::with_editor(|editor| {
                let view = view_of(window, editor)?;
                let session = editor
                    .diff_sessions
                    .iter_mut()
                    .find(|session| session.side_of(view).is_some())
                    .ok_or_else(|| LuaError::RuntimeError("not a window of a diff".into()))?;
                if index == 0 || index > session.hunks.len() {
                    return Err(LuaError::RuntimeError(format!(
                        "the diff has no change {index} (it has {})",
                        session.hunks.len()
                    )));
                }
                let change = index - 1;
                session
                    .plugin_marks
                    .retain(|m| !(m.change == change && m.mark.plugin == plugin));
                if let Some(mark) = mark.filter(|mark| !mark.text.is_empty()) {
                    session.plugin_marks.push(DiffMark { change, mark });
                }
                Ok(())
            })
        },
    )?;
    diff.set("mark", mark)?;

    // helix.diff.clear_marks(window) - Remove the calling plugin's marks of the diff of
    // `window` (default: of every diff)
    let clear_marks = lua.create_function(|lua, window: LuaValue| {
        require(lua, Permission::Diff, "helix.diff.clear_marks")?;
        let plugin = crate::lua::current_plugin_name();
        crate::lua::with_editor(|editor| {
            let view = match window {
                LuaValue::Nil => None,
                window => Some(view_of(window, editor)?),
            };
            for session in &mut editor.diff_sessions {
                if view.is_none_or(|view| session.side_of(view).is_some()) {
                    session.plugin_marks.retain(|m| *m.mark.plugin != *plugin);
                }
            }
            Ok(())
        })
    })?;
    diff.set("clear_marks", clear_marks)?;

    // helix.diff.open({ buffer, text, title }) - Show `buffer` (default: the focused one) in
    // a diff against `text` (on the left, read-only), titled `title`; returns the diff
    let open = lua.create_function(|lua, spec: LuaTable| {
        require(lua, Permission::Diff, "helix.diff.open")?;
        let text: String = spec
            .raw_get::<Option<String>>("text")?
            .ok_or_else(|| LuaError::RuntimeError("helix.diff.open needs a `text`".into()))?;
        let title: String = spec
            .raw_get::<Option<String>>("title")?
            .unwrap_or_else(|| "old".to_string());
        let buffer = match spec.raw_get::<LuaValue>("buffer")? {
            LuaValue::Nil => None,
            LuaValue::UserData(buffer) => Some(
                buffer
                    .borrow::<LuaBuffer>()
                    .map_err(|_| {
                        LuaError::RuntimeError("`buffer` must be a buffer (helix.buffer...)".into())
                    })?
                    .document_id,
            ),
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "`buffer` must be a buffer (got a value of type {})",
                    other.type_name()
                )))
            }
        };
        crate::lua::with_editor(|editor| {
            let right = match buffer {
                Some(id) => id,
                None => editor.tree.get(editor.tree.focus).doc,
            };
            let Some(doc) = editor.documents.get(&right) else {
                return Err(LuaError::RuntimeError("the buffer is closed".into()));
            };
            let syntax_path = doc.path().map(|path| path.to_path_buf());
            editor.open_diff(
                OldVersion {
                    text: helix_core::Rope::from(text),
                    title,
                    syntax_path,
                },
                right,
            );
            let view = editor.tree.focus;
            match editor.diff_session(view) {
                Some(session) => Ok(Some(session_table(lua, editor, session)?)),
                None => Ok(None),
            }
        })
    })?;
    diff.set("open", open)?;

    // helix.diff.close(window) - Close the diff of `window` (default: the focused one); returns
    // whether there was one
    let close = lua.create_function(|lua, window: LuaValue| {
        require(lua, Permission::Diff, "helix.diff.close")?;
        crate::lua::with_editor(|editor| {
            let view = view_of(window, editor)?;
            Ok(editor.close_diff(view))
        })
    })?;
    diff.set("close", close)?;

    // helix.diff.add_action({ key, desc, run }) / remove_action(key) / list_actions() - Keys of
    // the diff views (normal mode, after `[editor.diff.keys]`), running `run(diff)` with the
    // focused diff and its change under the cursor (`diff.change`)
    super::actions::register(lua, &diff, ActionPlace::Diff, Permission::Diff)?;

    helix.set("diff", diff)?;
    Ok(())
}
