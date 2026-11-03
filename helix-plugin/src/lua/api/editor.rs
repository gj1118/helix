use crate::error::Result;
use helix_view::document::Mode;
use mlua::prelude::*;

/// Register editor API in the Helix Lua global table
pub fn register_editor_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let editor_module = lua.create_table()?;

    // helix.editor.mode() - Get current editor mode
    let mode = lua.create_function(|_lua, ()| {
        let mode = crate::lua::try_with_editor(|editor| Ok(editor.mode()))?;
        let mode = match mode {
            Some(Mode::Insert) => "insert",
            Some(Mode::Select) => "select",
            Some(Mode::Normal) | None => "normal",
        };
        Ok(mode.to_string())
    })?;
    editor_module.set("mode", mode)?;

    // Mode switching functions
    let normal_mode = lua.create_function(|_lua, ()| {
        crate::lua::try_with_editor(|editor| {
            // Leave insert/select mode like Helix does (restores the cursor, trailing
            // indentation); that needs a focused window.
            if editor.tree.try_get(editor.tree.focus).is_some() {
                editor.enter_normal_mode();
            } else {
                editor.mode = Mode::Normal;
            }
            Ok(())
        })?;
        Ok(())
    })?;
    editor_module.set("normal_mode", normal_mode)?;

    let insert_mode = lua.create_function(|_lua, ()| {
        crate::lua::try_with_editor(|editor| {
            editor.mode = Mode::Insert;
            Ok(())
        })?;
        Ok(())
    })?;
    editor_module.set("insert_mode", insert_mode)?;

    let select_mode = lua.create_function(|_lua, ()| {
        crate::lua::try_with_editor(|editor| {
            editor.mode = Mode::Select;
            Ok(())
        })?;
        Ok(())
    })?;
    editor_module.set("select_mode", select_mode)?;

    // helix.editor.execute_command(cmd, args) - Execute a typed command
    let execute_command =
        lua.create_function(|lua, (cmd, args): (String, Option<Vec<String>>)| {
            run_typable(lua, &cmd, &args.unwrap_or_default())
        })?;
    editor_module.set("execute_command", execute_command)?;

    // helix.editor.move_cursor(direction, count) - Move the cursors like h/j/k/l
    let move_cursor =
        lua.create_function(|_lua, (direction, count): (String, Option<usize>)| {
            use helix_core::movement::{move_horizontally, move_vertically, Direction, Movement};
            let (vertical, dir) = match direction.as_str() {
                "left" => (false, Direction::Backward),
                "right" => (false, Direction::Forward),
                "up" => (true, Direction::Backward),
                "down" => (true, Direction::Forward),
                _ => {
                    return Err(LuaError::RuntimeError(format!(
                        "Invalid direction '{}' (expected left, right, up or down)",
                        direction
                    )))
                }
            };
            crate::lua::with_editor(|editor| {
                let behaviour = if editor.mode == Mode::Select {
                    Movement::Extend
                } else {
                    Movement::Move
                };
                let (view, doc) = crate::lua::focused_mut(editor)?;
                let text = doc.text().slice(..);
                // Movements stop at the end of the text anyway.
                let count = count.unwrap_or(1).min(text.len_chars() + 1);
                let text_fmt = doc.text_format(view.inner_area(doc).width, None);
                let mut annotations = view.text_annotations(doc, None);
                let selection = doc.selection(view.id).clone().transform(|range| {
                    let move_fn = if vertical {
                        move_vertically
                    } else {
                        move_horizontally
                    };
                    move_fn(
                        text,
                        range,
                        dir,
                        count,
                        behaviour,
                        &text_fmt,
                        &mut annotations,
                    )
                });
                drop(annotations);
                doc.set_selection(view.id, selection);
                Ok(())
            })
        })?;
    editor_module.set("move_cursor", move_cursor)?;

    // helix.editor.get_cursor() - Get cursor position
    let get_cursor = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_ref(editor)?;
            let text = doc.text().slice(..);
            let cursor = doc.selection(view.id).primary().cursor(text);
            let row = text.char_to_line(cursor);
            let col = cursor - text.line_to_char(row);
            Ok(super::buffer::LuaPosition { row, col })
        })
    })?;
    editor_module.set("get_cursor", get_cursor)?;

    // helix.editor.set_cursor(row, col) - Set cursor position
    let set_cursor = lua.create_function(|_lua, (row, col): (usize, usize)| {
        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_mut(editor)?;

            let text = doc.text().slice(..);
            let row = row.min(text.len_lines().saturating_sub(1));
            // A column past the end of the line stays on the line (at its line ending)
            // instead of landing on the next one.
            let start = text.line_to_char(row);
            let end = helix_core::line_ending::line_end_char_index(&text, row);
            let offset = start + col.min(end - start);

            let selection = helix_core::Selection::point(offset);
            doc.set_selection(view.id, selection);
            Ok(())
        })
    })?;
    editor_module.set("set_cursor", set_cursor)?;

    // helix.editor.get_config() - Get editor configuration
    let get_config = lua.create_function(|lua, ()| {
        let config = crate::lua::with_editor(|editor| Ok(editor.config()))?;
        let table = lua.create_table()?;

        let scrolloff = lua.create_table()?;
        scrolloff.set("vertical", config.scrolloff.vertical)?;
        scrolloff.set("horizontal", config.scrolloff.horizontal)?;

        table.set("scrolloff", scrolloff)?;
        table.set("mouse", config.mouse)?;
        table.set("cursorline", config.cursorline)?;
        table.set("cursorcolumn", config.cursorcolumn)?;
        table.set("auto_format", config.auto_format)?;
        table.set("auto_completion", config.auto_completion)?;
        table.set("auto_info", config.auto_info)?;
        let inline_diagnostics = lua.create_table()?;
        inline_diagnostics.set(
            "show_virtual_lines",
            config.inline_diagnostics.show_virtual_lines,
        )?;
        table.set("inline_diagnostics", inline_diagnostics)?;
        table.set(
            "line_number",
            match config.line_number {
                helix_view::editor::LineNumber::Absolute => "absolute",
                helix_view::editor::LineNumber::Relative => "relative",
            },
        )?;

        Ok(table)
    })?;
    editor_module.set("get_config", get_config)?;

    // helix.editor.get_selections() - Get current selections
    let get_selections = lua.create_function(|lua, ()| {
        let ranges = crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_ref(editor)?;
            Ok(doc
                .selection(view.id)
                .iter()
                .map(|range| (range.anchor, range.head))
                .collect::<Vec<_>>())
        })?;
        super::buffer::selections_to_lua(lua, &ranges)
    })?;
    editor_module.set("get_selections", get_selections)?;

    // helix.editor.set_selections(selections) - Set current selections
    let set_selections = lua.create_function(|_lua, selections: Vec<LuaTable>| {
        // Read every value before touching the editor: `raw_get` never runs metamethods.
        let mut ranges = Vec::with_capacity(selections.len());
        for s in &selections {
            let anchor: usize = s.raw_get("anchor")?;
            let head: usize = s.raw_get("head")?;
            ranges.push((anchor, head));
        }
        drop(selections);
        if ranges.is_empty() {
            return Ok(());
        }

        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_mut(editor)?;
            let len = doc.text().len_chars();
            let ranges: Vec<helix_core::Range> = ranges
                .into_iter()
                .map(|(anchor, head)| helix_core::Range::new(anchor.min(len), head.min(len)))
                .collect();
            let selection = helix_core::Selection::new(ranges.into(), 0);
            doc.set_selection(view.id, selection);
            Ok(())
        })
    })?;
    editor_module.set("set_selections", set_selections)?;
    // helix.editor.set_status(message, level) - level: "info" (default), "warning", "error"
    let set_status = lua.create_function(|_lua, (message, level): (String, Option<String>)| {
        crate::lua::with_editor(|editor| {
            super::ui::show_message(editor, message, level.as_deref());
            Ok(())
        })
    })?;
    editor_module.set("set_status", set_status)?;

    // helix.editor.focus() - The focused window
    let focus = lua.create_function(|_lua, ()| {
        let view_id = crate::lua::with_editor(|editor| Ok(editor.tree.focus))?;
        Ok(super::window::LuaWindow { view_id })
    })?;
    editor_module.set("focus", focus)?;

    // helix.editor.save(force) - Save the current buffer (`:write` / `:write!`)
    let save = lua.create_function(|lua, force: Option<bool>| {
        let cmd = if force.unwrap_or(false) {
            "write!"
        } else {
            "write"
        };
        run_typable(lua, cmd, &[])
    })?;
    editor_module.set("save", save)?;

    // helix.editor.save_all(force) - Save all buffers (`:write-all` / `:write-all!`)
    let save_all = lua.create_function(|lua, force: Option<bool>| {
        let cmd = if force.unwrap_or(false) {
            "write-all!"
        } else {
            "write-all"
        };
        run_typable(lua, cmd, &[])
    })?;
    editor_module.set("save_all", save_all)?;

    // helix.editor.quit(force) - Close the focused window like `:quit` / `:quit!` (Helix
    // exits when it was the last one; without `force` unsaved changes prevent that)
    let quit = lua.create_function(|lua, force: Option<bool>| {
        let cmd = if force.unwrap_or(false) {
            "quit!"
        } else {
            "quit"
        };
        run_typable(lua, cmd, &[])
    })?;
    editor_module.set("quit", quit)?;

    // helix.editor.open(path, action) - Open a file
    let open = lua.create_function(|_lua, (path_str, action_str): (String, Option<String>)| {
        let path = std::path::PathBuf::from(path_str);

        let action = match action_str.as_deref() {
            Some("split") => helix_view::editor::Action::HorizontalSplit,
            Some("vsplit") => helix_view::editor::Action::VerticalSplit,
            Some("replace") => helix_view::editor::Action::Replace,
            _ => helix_view::editor::Action::Load,
        };

        let doc_id = crate::lua::with_editor(|editor| {
            // `Editor::open` assumes a focused window (there is none once `quit` closed
            // the last one).
            crate::lua::focused_ref(editor)?;
            editor
                .open(&path, action)
                .map_err(|e| LuaError::RuntimeError(format!("Failed to open file: {}", e)))
        })?;
        Ok(super::buffer::LuaBuffer::new(doc_id))
    })?;
    editor_module.set("open", open)?;

    // helix.editor.close() - Close the focused window
    let close = lua.create_function(|_lua, _force: Option<bool>| {
        crate::lua::with_editor(|editor| {
            let view_id = editor.tree.focus;
            if !editor.tree.contains(view_id) {
                return Err(LuaError::RuntimeError("No focused window".to_string()));
            }
            // Closing the last window makes Helix exit without the unsaved-buffer
            // checks of `:quit`, so plugins are not allowed to do that.
            if editor.tree.views().count() <= 1 {
                return Err(LuaError::RuntimeError(
                    "Cannot close the last window from a plugin; use :quit instead".to_string(),
                ));
            }
            editor.close(view_id);
            Ok(())
        })
    })?;
    editor_module.set("close", close)?;

    // helix.editor.undo() - Undo last change
    let undo = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_mut(editor)?;
            Ok(doc.undo(view))
        })
    })?;
    editor_module.set("undo", undo)?;

    // helix.editor.redo() - Redo last undone change
    let redo = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_mut(editor)?;
            Ok(doc.redo(view))
        })
    })?;
    editor_module.set("redo", redo)?;

    // helix.editor.select_all() - Select all text
    let select_all = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_mut(editor)?;
            let end = doc.text().len_chars();
            let selection = helix_core::Selection::single(0, end);
            doc.set_selection(view.id, selection);
            Ok(())
        })
    })?;
    editor_module.set("select_all", select_all)?;

    // helix.editor.get_register(name) - Get register values
    let get_register = lua.create_function(|lua, name_str: String| {
        let name = name_str.chars().next().ok_or_else(|| {
            LuaError::RuntimeError("Register name must be a single character".to_string())
        })?;
        let values: Vec<String> = crate::lua::with_editor(|editor| {
            // Some registers ('#', '.', '%') read the focused window.
            crate::lua::focused_ref(editor)?;
            Ok(editor
                .registers
                .read(name, editor)
                .map(|values| values.map(|val| val.to_string()).collect())
                .unwrap_or_default())
        })?;
        lua.create_sequence_from(values)
    })?;
    editor_module.set("get_register", get_register)?;

    // helix.editor.set_register(name, values) - Set register values
    let set_register = lua.create_function(|_lua, (name_str, values): (String, Vec<String>)| {
        let name = name_str.chars().next().ok_or_else(|| {
            LuaError::RuntimeError("Register name must be a single character".to_string())
        })?;
        crate::lua::with_editor(|editor| {
            editor.registers.write(name, values).map_err(|e| {
                LuaError::RuntimeError(format!("Failed to write to register {}: {}", name, e))
            })
        })
    })?;
    editor_module.set("set_register", set_register)?;

    helix_table.set("editor", editor_module)?;

    Ok(())
}

/// Run the typable command `cmd` (e.g. `"write"`) with `args`, one argument per element.
/// Does nothing outside of an editor context.
fn run_typable(lua: &Lua, cmd: &str, args: &[String]) -> LuaResult<()> {
    let registry = lua
        .app_data_ref::<crate::types::CommandRegistryWrapper>()
        .map(|wrapper| std::sync::Arc::clone(&wrapper.0));

    crate::lua::try_with_editor(|editor| {
        let registry = registry.ok_or_else(|| {
            LuaError::RuntimeError("Builtin command registry not available".to_string())
        })?;
        registry
            .execute(editor, cmd, args)
            .map_err(|e| LuaError::RuntimeError(format!("Command '{}' failed: {}", cmd, e)))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_editor_api_registration() {
        let lua = Lua::new();
        let helix_table = lua.create_table().unwrap();

        let result = register_editor_api(&lua, &helix_table);
        assert!(result.is_ok());

        // Verify editor module exists with expected functions
        let editor_module: LuaTable = helix_table.get("editor").unwrap();
        assert!(editor_module.contains_key("mode").unwrap());
        assert!(editor_module.contains_key("insert_mode").unwrap());
        assert!(editor_module.contains_key("normal_mode").unwrap());
        assert!(editor_module.contains_key("execute_command").unwrap());
        assert!(editor_module.contains_key("move_cursor").unwrap());
        assert!(editor_module.contains_key("get_cursor").unwrap());
        assert!(editor_module.contains_key("save").unwrap());
        assert!(editor_module.contains_key("open").unwrap());
    }

    #[test]
    fn test_editor_functions_callable() {
        let lua = Lua::new();
        let helix_table = lua.create_table().unwrap();
        register_editor_api(&lua, &helix_table).unwrap();

        // Test that we can call editor functions from Lua
        let code = r#"
            local mode = helix.editor.mode()
            assert(mode ~= nil)
            
            local result = helix.editor.execute_command("test")
            assert(result == nil)
        "#;

        lua.globals().set("helix", helix_table).unwrap();
        lua.load(code).exec().unwrap();
    }
}
