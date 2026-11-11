use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::{Document, Editor, View, ViewId};
use mlua::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct LuaWindow {
    pub view_id: ViewId,
}

/// The view `id` and its document, or an error once the window is closed.
fn view_doc(editor: &mut Editor, id: ViewId) -> LuaResult<(&mut View, &mut Document)> {
    let view = editor
        .tree
        .try_get(id)
        .ok_or_else(|| LuaError::RuntimeError(format!("Window {:?} no longer exists", id)))?;
    let doc_id = view.doc;
    let view = editor.tree.get_mut(id);
    let doc = editor
        .documents
        .get_mut(&doc_id)
        .ok_or_else(|| LuaError::RuntimeError("Window has no document".to_string()))?;
    Ok((view, doc))
}

impl LuaUserData for LuaWindow {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // Get window ID (the `window_id` of events)
        methods.add_method("id", |_lua, this, ()| Ok(format!("{:?}", this.view_id)));

        // Get buffer in this window
        methods.add_method("get_buffer", |lua, this, ()| {
            require(lua, Permission::Read, "window:get_buffer")?;
            let doc_id = crate::lua::with_editor(|editor| {
                let view = editor.tree.try_get(this.view_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!("Window {:?} no longer exists", this.view_id))
                })?;
                Ok(view.doc)
            })?;
            Ok(super::buffer::LuaBuffer::new(doc_id))
        });

        // Whether the window exists and has the focus
        methods.add_method("is_focused", |lua, this, ()| {
            require(lua, Permission::Read, "window:is_focused")?;
            crate::lua::with_editor(|editor| {
                Ok(editor.tree.contains(this.view_id) && editor.tree.focus == this.view_id)
            })
        });

        // Whether the window still exists
        methods.add_method("is_valid", |lua, this, ()| {
            require(lua, Permission::Read, "window:is_valid")?;
            crate::lua::with_editor(|editor| Ok(editor.tree.contains(this.view_id)))
        });

        // Focus this window
        methods.add_method("focus", |lua, this, ()| {
            require(lua, Permission::Windows, "window:focus")?;
            crate::lua::with_editor(|editor| {
                if editor.tree.try_get(this.view_id).is_some() {
                    // Like a window switch by the user: commits pending changes, leaves
                    // insert mode, keeps the cursor in view and dispatches focus events.
                    editor.focus(this.view_id);
                    Ok(())
                } else {
                    Err(LuaError::RuntimeError(format!(
                        "Window {:?} no longer exists",
                        this.view_id
                    )))
                }
            })
        });

        // The area of the window on the screen: { x, y, width, height } (cells)
        methods.add_method("get_area", |lua, this, ()| {
            require(lua, Permission::Read, "window:get_area")?;
            let area = crate::lua::with_editor(|editor| {
                let view = editor.tree.try_get(this.view_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!("Window {:?} no longer exists", this.view_id))
                })?;
                Ok(view.area)
            })?;
            let table = lua.create_table()?;
            table.set("x", area.x)?;
            table.set("y", area.y)?;
            table.set("width", area.width)?;
            table.set("height", area.height)?;
            Ok(table)
        });

        // The primary cursor in this window: { row, col } (0-based)
        methods.add_method("get_cursor", |lua, this, ()| {
            require(lua, Permission::Read, "window:get_cursor")?;
            let (row, col) = crate::lua::with_editor(|editor| {
                let (view, doc) = view_doc(editor, this.view_id)?;
                let text = doc.text().slice(..);
                let cursor = doc.selection(view.id).primary().cursor(text);
                let row = text.char_to_line(cursor);
                Ok((row, cursor - text.line_to_char(row)))
            })?;
            let table = lua.create_table()?;
            table.set("row", row)?;
            table.set("col", col)?;
            Ok(table)
        });

        // Move the cursor of this window (a single cursor; the column stays on its line)
        methods.add_method("set_cursor", |lua, this, (row, col): (usize, usize)| {
            require(lua, Permission::Edit, "window:set_cursor")?;
            crate::lua::with_editor(|editor| {
                let (view, doc) = view_doc(editor, this.view_id)?;
                let text = doc.text().slice(..);
                let row = row.min(text.len_lines().saturating_sub(1));
                let start = text.line_to_char(row);
                let end = helix_core::line_ending::line_end_char_index(&text, row);
                let pos = start + col.min(end - start);
                doc.set_selection(view.id, helix_core::Selection::point(pos));
                Ok(())
            })
        });

        // The selections of this window: { {anchor, head}, ... } and the primary index (1-based)
        methods.add_method("get_selections", |lua, this, ()| {
            require(lua, Permission::Read, "window:get_selections")?;
            let (ranges, primary) = crate::lua::with_editor(|editor| {
                let (view, doc) = view_doc(editor, this.view_id)?;
                let selection = doc.selection(view.id);
                Ok((
                    selection
                        .ranges()
                        .iter()
                        .map(|r| (r.anchor, r.head))
                        .collect::<Vec<_>>(),
                    selection.primary_index() + 1,
                ))
            })?;
            Ok((super::buffer::selections_to_lua(lua, &ranges)?, primary))
        });

        // Set the selections of this window (char indices, clamped); `primary` is 1-based
        methods.add_method(
            "set_selections",
            |lua, this, (ranges, primary): (LuaTable, Option<usize>)| {
                require(lua, Permission::Edit, "window:set_selections")?;
                let ranges = super::editor::read_ranges(&ranges)?;
                crate::lua::with_editor(|editor| {
                    let (view, doc) = view_doc(editor, this.view_id)?;
                    let selection = super::editor::selection_from(doc, &ranges, primary)?;
                    doc.set_selection(view.id, selection);
                    Ok(())
                })
            },
        );

        // The first line shown (0-based)
        methods.add_method("get_top_line", |lua, this, ()| {
            require(lua, Permission::Read, "window:get_top_line")?;
            crate::lua::with_editor(|editor| {
                let (view, doc) = view_doc(editor, this.view_id)?;
                let anchor = doc.view_offset(view.id).anchor;
                Ok(doc.text().char_to_line(anchor.min(doc.text().len_chars())))
            })
        });

        // Scroll so that `line` (0-based, clamped) is the first line shown
        methods.add_method("scroll_to", |lua, this, line: usize| {
            require(lua, Permission::Windows, "window:scroll_to")?;
            crate::lua::with_editor(|editor| {
                let (view, doc) = view_doc(editor, this.view_id)?;
                let text = doc.text();
                let line = line.min(text.len_lines().saturating_sub(1));
                let mut offset = doc.view_offset(view.id);
                offset.anchor = text.line_to_char(line);
                offset.vertical_offset = 0;
                doc.set_view_offset(view.id, offset);
                Ok(())
            })
        });

        // Show `buffer` in this window
        methods.add_method(
            "set_buffer",
            |lua, this, buffer: LuaUserDataRef<super::buffer::LuaBuffer>| {
                require(lua, Permission::Windows, "window:set_buffer")?;
                let doc_id = buffer.document_id;
                crate::lua::with_editor(|editor| {
                    if !editor.tree.contains(this.view_id) {
                        return Err(LuaError::RuntimeError(format!(
                            "Window {:?} no longer exists",
                            this.view_id
                        )));
                    }
                    if editor.document(doc_id).is_none() {
                        return Err(LuaError::RuntimeError(format!(
                            "Buffer {:?} no longer exists",
                            doc_id
                        )));
                    }
                    // `switch` replaces the buffer of the focused window: focus this one for it.
                    let previous = editor.tree.focus;
                    if previous != this.view_id {
                        editor.focus(this.view_id);
                    }
                    editor.switch(doc_id, helix_view::editor::Action::Replace);
                    if previous != this.view_id && editor.tree.contains(previous) {
                        editor.focus(previous);
                    }
                    Ok(())
                })
            },
        );

        // Close this window (not the last one: use helix.editor.quit)
        methods.add_method("close", |lua, this, ()| {
            require(lua, Permission::Windows, "window:close")?;
            crate::lua::with_editor(|editor| {
                if !editor.tree.contains(this.view_id) {
                    return Err(LuaError::RuntimeError(format!(
                        "Window {:?} no longer exists",
                        this.view_id
                    )));
                }
                if editor.tree.views().count() <= 1 {
                    return Err(LuaError::RuntimeError(
                        "Cannot close the last window from a plugin; use :quit instead".to_string(),
                    ));
                }
                editor.close(this.view_id);
                Ok(())
            })
        });
    }
}

pub fn register_window_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let window_module = lua.create_table()?;

    // helix.window.get_current() - Get focused window
    let get_current = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.window.get_current")?;
        let view_id = crate::lua::with_editor(|editor| Ok(editor.tree.focus))?;
        Ok(LuaWindow { view_id })
    })?;
    window_module.set("get_current", get_current)?;

    // helix.window.list() - List all windows
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.window.list")?;
        // Collect ids first: creating Lua values may run finalizers.
        let ids: Vec<ViewId> = crate::lua::with_editor(|editor| {
            Ok(editor
                .tree
                .views()
                .map(|(view, _focused)| view.id)
                .collect())
        })?;
        lua.create_sequence_from(ids.into_iter().map(|view_id| LuaWindow { view_id }))
    })?;
    window_module.set("list", list)?;

    // helix.window.get_by_id(id) - The window with that id (`window:id()`, events'
    // `window_id`), or nil
    let get_by_id = lua.create_function(|lua, id: String| {
        require(lua, Permission::Read, "helix.window.get_by_id")?;
        let view_id = crate::lua::with_editor(|editor| {
            Ok(editor
                .tree
                .views()
                .map(|(view, _)| view.id)
                .find(|view_id| format!("{:?}", view_id) == id))
        })?;
        Ok(view_id.map(|view_id| LuaWindow { view_id }))
    })?;
    window_module.set("get_by_id", get_by_id)?;

    // helix.window.split(direction, buffer) - Split the focused window ("vertical" or
    // "horizontal"), showing `buffer` (default: the same one); the new window is focused
    let split = lua.create_function(
        |lua, (direction, buffer): (String, Option<LuaUserDataRef<super::buffer::LuaBuffer>>)| {
            require(lua, Permission::Windows, "helix.window.split")?;
            let action = match direction.as_str() {
                "vertical" => helix_view::editor::Action::VerticalSplit,
                "horizontal" => helix_view::editor::Action::HorizontalSplit,
                _ => {
                    return Err(LuaError::RuntimeError(format!(
                        "Invalid direction '{direction}' (expected vertical or horizontal)"
                    )))
                }
            };
            let buffer = buffer.map(|buffer| buffer.document_id);
            let view_id = crate::lua::with_editor(|editor| {
                let (_, doc) = crate::lua::focused_ref(editor)?;
                let doc_id = buffer.unwrap_or(doc.id());
                if editor.document(doc_id).is_none() {
                    return Err(LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        doc_id
                    )));
                }
                editor.switch(doc_id, action);
                Ok(editor.tree.focus)
            })?;
            Ok(LuaWindow { view_id })
        },
    )?;
    window_module.set("split", split)?;

    helix_table.set("window", window_module)?;

    Ok(())
}
