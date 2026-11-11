//! `helix.quickfix`: the quicklist, shown in the quickfix panel (`:copen`).

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::quicklist::{Entry, Position, Target};
use mlua::prelude::*;
use std::sync::Arc;

/// A quicklist entry from a plugin's `{ path | buffer, line, end_line, text }` (lines
/// 1-based), read without running metamethods.
fn read_entry(item: &LuaTable) -> LuaResult<Entry> {
    let path: Option<String> = item.raw_get("path")?;
    let buffer: LuaValue = item.raw_get("buffer")?;
    let target = match (path, buffer) {
        (Some(path), _) => Target::Path(helix_stdx::path::canonicalize(path)),
        (None, LuaValue::UserData(ud)) => {
            let buffer = ud.borrow::<super::buffer::LuaBuffer>().map_err(|_| {
                LuaError::RuntimeError("`buffer` must be a buffer (helix.buffer...)".into())
            })?;
            Target::Doc(buffer.document_id)
        }
        _ => {
            return Err(LuaError::RuntimeError(
                "A quickfix entry needs a `path` or a `buffer`".into(),
            ))
        }
    };
    let line: usize = item.raw_get::<Option<usize>>("line")?.unwrap_or(1);
    let start = line.saturating_sub(1);
    let end = item
        .raw_get::<Option<usize>>("end_line")?
        .map_or(start, |end| end.saturating_sub(1).max(start));
    let text: String = item.raw_get::<Option<String>>("text")?.unwrap_or_default();
    Ok(Entry::new(
        target,
        Position::Lines { start, end },
        start,
        text,
    ))
}

fn read_entries(items: &LuaTable) -> LuaResult<Vec<Entry>> {
    let mut entries = Vec::new();
    for i in 1..=items.raw_len() {
        let item: LuaTable = items.raw_get(i)?;
        entries.push(read_entry(&item)?);
    }
    Ok(entries)
}

/// Shows (`open`) or closes the quickfix panel, through the UI.
fn panel(lua: &Lua, open: bool) -> LuaResult<()> {
    if let Some(handler) = lua
        .app_data_ref::<crate::types::UiHandlerWrapper>()
        .map(|h| Arc::clone(&h.0))
    {
        handler.quickfix(open);
    }
    Ok(())
}

pub fn register_quickfix_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let quickfix = lua.create_table()?;

    // helix.quickfix.set(items, open) - Replace the quicklist (and show it when `open`)
    let set = lua.create_function(|lua, (items, open): (LuaTable, Option<bool>)| {
        require(lua, Permission::Edit, "helix.quickfix.set")?;
        if open.unwrap_or(false) {
            require(lua, Permission::Ui, "helix.quickfix.set")?;
        }
        let entries = read_entries(&items)?;
        crate::lua::with_editor(|editor| {
            editor.quicklist.replace(entries);
            Ok(())
        })?;
        if open.unwrap_or(false) {
            panel(lua, true)?;
        }
        Ok(())
    })?;
    quickfix.set("set", set)?;

    // helix.quickfix.add(item) - Add an entry at the end
    let add = lua.create_function(|lua, item: LuaTable| {
        require(lua, Permission::Edit, "helix.quickfix.add")?;
        let entry = read_entry(&item)?;
        crate::lua::with_editor(|editor| {
            editor.quicklist.push(entry);
            Ok(())
        })
    })?;
    quickfix.set("add", add)?;

    // helix.quickfix.clear() - Empty the quicklist
    let clear = lua.create_function(|lua, ()| {
        require(lua, Permission::Edit, "helix.quickfix.clear")?;
        crate::lua::with_editor(|editor| {
            editor.quicklist.replace(Vec::new());
            Ok(())
        })
    })?;
    quickfix.set("clear", clear)?;

    // helix.quickfix.get() - The entries: { {path, buffer, line, end_line, text}, ... }
    let get = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.quickfix.get")?;
        let entries: Vec<Entry> =
            crate::lua::with_editor(|editor| Ok(editor.quicklist.entries().to_vec()))?;
        let list = lua.create_table()?;
        for entry in entries {
            let item = lua.create_table()?;
            match entry.target {
                Target::Path(path) => item.set("path", path.to_string_lossy().into_owned())?,
                Target::Doc(id) => item.set("buffer", super::buffer::LuaBuffer::new(id))?,
            }
            let (start, end) = match entry.position {
                Position::Lines { start, end } => (start, end),
                Position::Selection(_) => (entry.line, entry.line),
            };
            item.set("line", start + 1)?;
            item.set("end_line", end + 1)?;
            item.set("text", entry.text)?;
            list.push(item)?;
        }
        Ok(list)
    })?;
    quickfix.set("get", get)?;

    // helix.quickfix.open() / close() - Show the quicklist in the panel / close the panel
    let open = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.quickfix.open")?;
        panel(lua, true)
    })?;
    quickfix.set("open", open)?;
    let close = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.quickfix.close")?;
        panel(lua, false)
    })?;
    quickfix.set("close", close)?;

    // helix.quickfix.is_open() - Whether the panel is shown
    let is_open = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.quickfix.is_open")?;
        crate::lua::with_editor(|editor| Ok(editor.quickfix_panel.visible))
    })?;
    quickfix.set("is_open", is_open)?;

    helix.set("quickfix", quickfix)?;
    Ok(())
}
