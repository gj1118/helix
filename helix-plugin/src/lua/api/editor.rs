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
            editor.mode = Mode::Normal;
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
            let args = args.unwrap_or_default();
            let registry = lua
                .app_data_ref::<crate::types::CommandRegistryWrapper>()
                .map(|wrapper| std::sync::Arc::clone(&wrapper.0));

            crate::lua::try_with_editor(|editor| {
                let registry = registry.ok_or_else(|| {
                    LuaError::RuntimeError("Builtin command registry not available".to_string())
                })?;
                registry
                    .execute(editor, &cmd, &args)
                    .map_err(|e| LuaError::RuntimeError(format!("Command '{}' failed: {}", cmd, e)))
            })?;
            Ok(())
        })?;
    editor_module.set("execute_command", execute_command)?;

    // helix.editor.move_cursor(direction, count) - Move cursor
    let move_cursor =
        lua.create_function(|_lua, (direction, count): (String, Option<usize>)| {
            let count = count.unwrap_or(1);
            // TODO: Implement actual cursor movement
            Ok(format!("Would move cursor {} {} times", direction, count))
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

            let text = doc.text();
            let row = row.min(text.len_lines().saturating_sub(1));
            let offset = text.line_to_char(row) + col.min(text.line(row).len_chars());

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
    let set_status = lua.create_function(|_lua, (message, _level): (String, Option<String>)| {
        crate::lua::with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })
    })?;
    editor_module.set("set_status", set_status)?;

    // helix.editor.focus() - Get focused view/buffer
    let focus = lua.create_function(|_lua, ()| {
        // TODO: Implement actual focus retrieval
        Ok("current_view_id")
    })?;
    editor_module.set("focus", focus)?;

    // helix.editor.save() - Save current buffer
    let save = lua.create_function(|_lua, force: Option<bool>| {
        let force = force.unwrap_or(false);
        crate::lua::with_editor(|_editor| Ok(()))?;
        // This usually triggers an async save, but for now we'll use the sync command approach
        // if possible, or just log.
        // Actually, helix has typed commands for this.
        Ok(format!("Would save buffer (force: {})", force))
    })?;
    editor_module.set("save", save)?;

    // helix.editor.save_all() - Save all buffers
    let save_all = lua.create_function(|_lua, ()| {
        // TODO: Implement actual save all
        Ok("Would save all buffers")
    })?;
    editor_module.set("save_all", save_all)?;

    // helix.editor.quit() - Quit editor
    let quit = lua.create_function(|_lua, force: Option<bool>| {
        let force = force.unwrap_or(false);
        // TODO: Implement actual quit (this needs careful handling!)
        Ok(format!("Would quit (force: {})", force))
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
