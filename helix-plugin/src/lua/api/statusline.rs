//! `helix.statusline`: plugins' texts in the status line, drawn by Helix every frame.

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::editor::{SegmentPosition, StatusSegment};
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

pub fn register_statusline_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let statusline = lua.create_table()?;

    // helix.statusline.set(id, { text, position, buffer, priority, style, fg, bg, modifiers })
    // - Shows (or changes) the plugin's segment `id`
    let set = lua.create_function(|lua, (id, options): (String, LuaTable)| {
        require(lua, Permission::Ui, "helix.statusline.set")?;
        let text: String = options
            .raw_get::<Option<String>>("text")?
            .unwrap_or_default();
        if text.contains(['\n', '\r']) {
            return Err(LuaError::RuntimeError(
                "A status line segment is one line".into(),
            ));
        }
        let position = match options.raw_get::<Option<String>>("position")? {
            Some(name) => position(&name)?,
            None => SegmentPosition::Right,
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
        let priority: i64 = options.raw_get::<Option<i64>>("priority")?.unwrap_or(0);
        let (scope, style) = super::style::read_style(lua, &options)?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            let segment = StatusSegment {
                plugin: plugin.clone(),
                id: id.clone(),
                text,
                scope,
                style,
                position,
                document,
                priority,
            };
            let segments = &mut editor.plugin_statusline;
            match segments
                .iter_mut()
                .find(|s| s.plugin == plugin && s.id == id)
            {
                Some(existing) => *existing = segment,
                None => segments.push(segment),
            }
            helix_event::request_redraw();
            Ok(())
        })
    })?;
    statusline.set("set", set)?;

    // helix.statusline.remove(id) - Removes the plugin's segment `id`; whether there was one
    let remove = lua.create_function(|lua, id: String| {
        require(lua, Permission::Ui, "helix.statusline.remove")?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            let before = editor.plugin_statusline.len();
            editor
                .plugin_statusline
                .retain(|s| !(s.plugin == plugin && s.id == id));
            helix_event::request_redraw();
            Ok(before != editor.plugin_statusline.len())
        })
    })?;
    statusline.set("remove", remove)?;

    // helix.statusline.clear() - Removes all the plugin's segments
    let clear = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.statusline.clear")?;
        let plugin = crate::lua::current_plugin_name().to_string();
        crate::lua::with_editor(|editor| {
            editor.plugin_statusline.retain(|s| s.plugin != plugin);
            helix_event::request_redraw();
            Ok(())
        })
    })?;
    statusline.set("clear", clear)?;

    // helix.statusline.list() - The segments of all plugins: { {plugin, id, text, position}, ... }
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.statusline.list")?;
        let segments = crate::lua::with_editor(|editor| Ok(editor.plugin_statusline.clone()))?;
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
    statusline.set("list", list)?;

    helix.set("statusline", statusline)?;
    Ok(())
}
