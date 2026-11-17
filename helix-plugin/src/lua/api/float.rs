//! Floating windows of plugins (`helix.ui.float`): a buffer drawn in a frame over the windows.
//!
//! The editor keeps what is drawn ([`helix_view::plugin_ui::PluginFloat`]); the functions of
//! its keys stay in the Lua state of its plugin, by float, until it is closed.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::permissions::{require, Permission};
use helix_view::plugin_ui::{FloatPosition, PluginFloat};
use helix_view::Editor;
use mlua::prelude::*;

/// The ids of the floats, over all plugins.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The key of the table of a state's float functions: float id -> { function id -> fn }.
const FUNCTIONS: &str = "helix.floats";

fn functions(lua: &Lua) -> LuaResult<LuaTable> {
    match lua.named_registry_value::<Option<LuaTable>>(FUNCTIONS)? {
        Some(table) => Ok(table),
        None => {
            let table = lua.create_table()?;
            lua.set_named_registry_value(FUNCTIONS, &table)?;
            Ok(table)
        }
    }
}

/// The function `function` of float `float` of `lua`'s plugin, if it has it.
pub(crate) fn function(lua: &Lua, float: u64, function: u64) -> LuaResult<Option<LuaFunction>> {
    match functions(lua)?.raw_get::<Option<LuaTable>>(float)? {
        Some(table) => table.raw_get(function),
        None => Ok(None),
    }
}

/// Drops the functions of float `float`.
pub(crate) fn forget(lua: &Lua, float: u64) -> LuaResult<()> {
    functions(lua)?.raw_set(float, LuaValue::Nil)
}

/// A handle on a float (`helix.ui.float` returns it).
#[derive(Debug, Clone, Copy)]
pub struct LuaFloat {
    pub id: u64,
}

/// Runs `f` with the float `id`, or errors once it is closed.
fn with_float<R>(id: u64, f: impl FnOnce(&mut Editor, usize) -> LuaResult<R>) -> LuaResult<R> {
    crate::lua::with_editor(|editor| {
        let index = editor
            .plugin_floats
            .iter()
            .position(|float| float.id == id)
            .ok_or_else(|| LuaError::RuntimeError(format!("Float {id} is closed")))?;
        f(editor, index)
    })
}

fn lines_of(editor: &Editor, float: &PluginFloat) -> usize {
    editor
        .document(float.document)
        .map_or(1, |doc| doc.text().len_lines())
}

fn position(value: LuaValue) -> LuaResult<FloatPosition> {
    match value {
        LuaValue::Nil => Ok(FloatPosition::Center),
        LuaValue::String(name) => match &*name.to_str()? {
            "center" => Ok(FloatPosition::Center),
            "cursor" => Ok(FloatPosition::Cursor),
            other => Err(LuaError::RuntimeError(format!(
                "Invalid position '{other}' (expected \"center\", \"cursor\" or {{ row, col }})"
            ))),
        },
        LuaValue::Table(at) => Ok(FloatPosition::At {
            row: at.get::<Option<u16>>("row")?.unwrap_or(0),
            col: at.get::<Option<u16>>("col")?.unwrap_or(0),
        }),
        other => Err(LuaError::RuntimeError(format!(
            "Invalid position (a value of type {})",
            other.type_name()
        ))),
    }
}

impl LuaUserData for LuaFloat {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, ()| Ok(this.id));

        methods.add_method("is_open", |lua, this, ()| {
            require(lua, Permission::Read, "float:is_open")?;
            crate::lua::with_editor(|editor| {
                Ok(editor.plugin_floats.iter().any(|float| float.id == this.id))
            })
        });

        // Closes it (its `on_close` is not run); whether it was open
        methods.add_method("close", |lua, this, ()| {
            require(lua, Permission::Ui, "float:close")?;
            let closed = crate::lua::with_editor(|editor| {
                let before = editor.plugin_floats.len();
                editor.plugin_floats.retain(|float| float.id != this.id);
                Ok(before != editor.plugin_floats.len())
            })?;
            forget(lua, this.id)?;
            helix_event::request_redraw();
            Ok(closed)
        });

        // Gives it the keys (`true`) or gives them back to the windows (`false`)
        methods.add_method("focus", |lua, this, focused: Option<bool>| {
            require(lua, Permission::Ui, "float:focus")?;
            let focused = focused.unwrap_or(true);
            with_float(this.id, |editor, index| {
                for (i, float) in editor.plugin_floats.iter_mut().enumerate() {
                    if i == index {
                        float.focused = focused;
                    } else if focused {
                        float.focused = false;
                    }
                }
                helix_event::request_redraw();
                Ok(())
            })
        });

        methods.add_method("is_focused", |lua, this, ()| {
            require(lua, Permission::Read, "float:is_focused")?;
            with_float(this.id, |editor, index| {
                Ok(editor.plugin_floats[index].focused)
            })
        });

        // The line of its cursor (1-based)
        methods.add_method("get_cursor", |lua, this, ()| {
            require(lua, Permission::Read, "float:get_cursor")?;
            with_float(this.id, |editor, index| {
                Ok(editor.plugin_floats[index].cursor_line + 1)
            })
        });

        methods.add_method("set_cursor", |lua, this, line: usize| {
            require(lua, Permission::Ui, "float:set_cursor")?;
            with_float(this.id, |editor, index| {
                let lines = lines_of(editor, &editor.plugin_floats[index]);
                editor.plugin_floats[index].set_cursor_line(line.saturating_sub(1), lines);
                helix_event::request_redraw();
                Ok(())
            })
        });

        // Changes the title, size or position: { title, width, height, position }
        methods.add_method("update", |lua, this, options: LuaTable| {
            require(lua, Permission::Ui, "float:update")?;
            let title: LuaValue = options.get("title")?;
            let width: Option<u16> = options.get("width")?;
            let height: Option<u16> = options.get("height")?;
            let position_value: LuaValue = options.get("position")?;
            let new_position = match position_value {
                LuaValue::Nil => None,
                value => Some(position(value)?),
            };
            let title = match title {
                LuaValue::Nil => None,
                LuaValue::Boolean(false) => Some(None),
                LuaValue::String(title) => Some(Some(title.to_str()?.to_string())),
                other => {
                    return Err(LuaError::RuntimeError(format!(
                        "A title is a string, or false for none (got a {})",
                        other.type_name()
                    )))
                }
            };
            with_float(this.id, |editor, index| {
                let float = &mut editor.plugin_floats[index];
                if let Some(title) = title {
                    float.title = title;
                }
                if let Some(width) = width {
                    float.width = width.max(1);
                }
                if let Some(height) = height {
                    float.height = height.max(1);
                }
                if let Some(position) = new_position {
                    float.position = position;
                }
                let (line, lines) = (
                    float.cursor_line,
                    lines_of(editor, &editor.plugin_floats[index]),
                );
                editor.plugin_floats[index].set_cursor_line(line, lines);
                helix_event::request_redraw();
                Ok(())
            })
        });

        methods.add_method("get_buffer", |lua, this, ()| {
            require(lua, Permission::Read, "float:get_buffer")?;
            let document = with_float(this.id, |editor, index| {
                Ok(editor.plugin_floats[index].document)
            })?;
            Ok(super::buffer::LuaBuffer::new(document))
        });
    }
}

/// `helix.ui.float{ buffer, title, width, height, position, focus, cursorline, cursor, keys,
/// on_close }`: shows a floating window; returns its handle.
pub(crate) fn open(lua: &Lua, options: LuaTable) -> LuaResult<LuaFloat> {
    require(lua, Permission::Ui, "helix.ui.float")?;
    let buffer: LuaUserDataRef<super::buffer::LuaBuffer> = options
        .get::<Option<LuaUserDataRef<super::buffer::LuaBuffer>>>("buffer")?
        .ok_or_else(|| LuaError::RuntimeError("A float needs a `buffer`".into()))?;
    let document = buffer.document_id;
    let title: Option<String> = options.get("title")?;
    let width: u16 = options.get::<Option<u16>>("width")?.unwrap_or(40).max(1);
    let height: u16 = options.get::<Option<u16>>("height")?.unwrap_or(10).max(1);
    let position = position(options.get("position")?)?;
    let focused: bool = options.get::<Option<bool>>("focus")?.unwrap_or(true);
    let cursorline: bool = options.get::<Option<bool>>("cursorline")?.unwrap_or(true);
    let cursor: usize = options.get::<Option<usize>>("cursor")?.unwrap_or(1);

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let functions_of_float = lua.create_table()?;
    let mut next_function = 1_u64;
    let mut keys = Vec::new();
    if let Some(table) = options.get::<Option<LuaTable>>("keys")? {
        for pair in table.pairs::<String, LuaFunction>() {
            let (key, function) = pair?;
            let parsed = crate::lua::parse_keys(&key)?;
            let [key] = parsed.as_slice() else {
                return Err(LuaError::RuntimeError(format!(
                    "A float's key is one key (got '{key}')"
                )));
            };
            functions_of_float.raw_set(next_function, function)?;
            keys.push((*key, next_function));
            next_function += 1;
        }
    }
    let on_close = match options.get::<Option<LuaFunction>>("on_close")? {
        Some(function) => {
            functions_of_float.raw_set(next_function, function)?;
            Some(next_function)
        }
        None => None,
    };
    let plugin = crate::lua::current_plugin_name().to_string();
    crate::lua::with_editor(|editor| {
        let lines = editor
            .document(document)
            .ok_or_else(|| LuaError::RuntimeError(format!("Buffer {document:?} no longer exists")))?
            .text()
            .len_lines();
        let mut float = PluginFloat {
            id,
            plugin,
            document,
            title,
            width,
            height,
            position,
            focused,
            cursor_line: 0,
            top_line: 0,
            cursorline,
            keys,
            on_close,
        };
        float.set_cursor_line(cursor.saturating_sub(1), lines);
        if focused {
            for other in &mut editor.plugin_floats {
                other.focused = false;
            }
        }
        editor.plugin_floats.push(float);
        Ok(())
    })?;
    functions(lua)?.raw_set(id, functions_of_float)?;
    helix_event::request_redraw();
    Ok(LuaFloat { id })
}

/// Runs function `function` of float `float` (a key, or its `on_close`) with `{ float, line,
/// buffer }`: the float's handle, its cursor line (1-based) and its buffer.
pub(crate) fn run(lua: &Lua, float: u64, function_id: u64) -> LuaResult<()> {
    let Some(function) = function(lua, float, function_id)? else {
        return Ok(());
    };
    let (line, document) = crate::lua::with_editor(|editor| {
        Ok(editor
            .plugin_floats
            .iter()
            .find(|f| f.id == float)
            .map(|f| (f.cursor_line + 1, f.document)))
    })?
    .unzip();
    let context = lua.create_table()?;
    context.raw_set("float", LuaFloat { id: float })?;
    context.raw_set("line", line)?;
    context.raw_set("buffer", document.map(super::buffer::LuaBuffer::new))?;
    function.call::<()>(context)
}
