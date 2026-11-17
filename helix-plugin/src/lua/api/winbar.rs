//! `helix.winbar`: plugins' texts in a bar at the top of windows, drawn by Helix every frame.

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::editor::{BarSegment, SegmentPosition};
use mlua::prelude::*;

fn position(name: &str) -> LuaResult<SegmentPosition> {
    match name {
        "left" => Ok(SegmentPosition::Left),
        "center" => Ok(SegmentPosition::Center),
        "right" => Ok(SegmentPosition::Right),
        _ => Err(LuaError::RuntimeError(format!(
            "Invalid position '{name}' (expected left, center or right)"
        ))),
    }
}

fn position_name(position: SegmentPosition) -> &'static str {
    match position {
        SegmentPosition::Left => "left",
        SegmentPosition::Center => "center",
        SegmentPosition::Right => "right",
    }
}

pub fn register_winbar_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let winbar = lua.create_table()?;

    // helix.winbar.set(id, { text, position, window, buffer, priority, style, fg, bg,
    // modifiers }) - Shows (or changes) the plugin's segment `id`
    let set = lua.create_function(|lua, (id, options): (String, LuaTable)| {
        require(lua, Permission::Ui, "helix.winbar.set")?;
        let text: String = options
            .raw_get::<Option<String>>("text")?
            .unwrap_or_default();
        if text.contains(['\n', '\r']) {
            return Err(LuaError::RuntimeError(
                "A window bar segment is one line".into(),
            ));
        }
        let position = match options.raw_get::<Option<String>>("position")? {
            Some(name) => position(&name)?,
            None => SegmentPosition::Left,
        };
        let document = match options.raw_get::<LuaValue>("buffer")? {
            LuaValue::Nil => None,
            LuaValue::UserData(ud) => Some(
                ud.borrow::<super::buffer::LuaBuffer>()
                    .map_err(|_| LuaError::RuntimeError("`buffer` must be a buffer".to_string()))?
                    .document_id,
            ),
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "`buffer` must be a buffer, not a {}",
                    other.type_name()
                )))
            }
        };
        let window = match options.raw_get::<LuaValue>("window")? {
            LuaValue::Nil => None,
            LuaValue::UserData(ud) => Some(
                ud.borrow::<super::window::LuaWindow>()
                    .map_err(|_| LuaError::RuntimeError("`window` must be a window".to_string()))?
                    .view_id,
            ),
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "`window` must be a window, not a {}",
                    other.type_name()
                )))
            }
        };
        let priority: i64 = options.raw_get::<Option<i64>>("priority")?.unwrap_or(0);
        let (scope, style) = super::style::read_style(lua, &options)?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            if let Some(window) = window {
                if editor.tree.try_get(window).is_none() {
                    return Err(LuaError::RuntimeError(format!(
                        "Window {window:?} no longer exists"
                    )));
                }
            }
            let segment = BarSegment {
                plugin: plugin.clone(),
                id: id.clone(),
                text,
                scope,
                style,
                position,
                document,
                window,
                priority,
            };
            let segments = &mut editor.plugin_window_bar;
            match segments
                .iter_mut()
                .find(|s| s.plugin == plugin && s.id == id)
            {
                Some(existing) => *existing = segment,
                None => segments.push(segment),
            }
            editor.sync_window_bars();
            helix_event::request_redraw();
            Ok(())
        })
    })?;
    winbar.set("set", set)?;

    // helix.winbar.remove(id) - Removes the plugin's segment `id`; whether there was one
    let remove = lua.create_function(|lua, id: String| {
        require(lua, Permission::Ui, "helix.winbar.remove")?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            let before = editor.plugin_window_bar.len();
            editor
                .plugin_window_bar
                .retain(|s| !(s.plugin == plugin && s.id == id));
            editor.sync_window_bars();
            helix_event::request_redraw();
            Ok(before != editor.plugin_window_bar.len())
        })
    })?;
    winbar.set("remove", remove)?;

    // helix.winbar.clear() - Removes all the plugin's segments
    let clear = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.winbar.clear")?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            editor.plugin_window_bar.retain(|s| s.plugin != plugin);
            editor.sync_window_bars();
            helix_event::request_redraw();
            Ok(())
        })
    })?;
    winbar.set("clear", clear)?;

    // helix.winbar.list() - The segments of all plugins: { {plugin, id, text, position,
    // priority}, ... }
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.winbar.list")?;
        let segments = crate::lua::with_editor(|editor| Ok(editor.plugin_window_bar.clone()))?;
        let list = lua.create_table()?;
        for segment in segments {
            let entry = lua.create_table()?;
            entry.set("plugin", segment.plugin)?;
            entry.set("id", segment.id)?;
            entry.set("text", segment.text)?;
            entry.set("position", position_name(segment.position))?;
            entry.set("priority", segment.priority)?;
            list.push(entry)?;
        }
        Ok(list)
    })?;
    winbar.set("list", list)?;

    helix.set("winbar", winbar)?;
    Ok(())
}
