//! `helix.explorer`: the file explorer panel.

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::explorer::{ExplorerDecoration, ExplorerRequest};
use helix_view::plugin_ui::ActionPlace;
use mlua::prelude::*;
use std::path::PathBuf;

/// A `"#rrggbb"` (or `"#rgb"`) color (`fg`) and a theme key (`style`) of `table`, checked.
pub(crate) fn read_color(table: &LuaTable) -> LuaResult<(Option<String>, Option<String>)> {
    let fg: Option<String> = table.raw_get("fg")?;
    if let Some(fg) = &fg {
        if helix_view::graphics::Color::from_hex(fg).is_err() {
            return Err(LuaError::RuntimeError(format!(
                "fg must be \"#rrggbb\", not {fg:?}"
            )));
        }
    }
    Ok((fg, table.raw_get("style")?))
}

/// A decoration from a plugin's `{ text, fg, style, name = { fg, style }, after = { text, fg,
/// style } }`, read without running metamethods.
fn read_decoration(table: &LuaTable, plugin: String) -> LuaResult<ExplorerDecoration> {
    let text: String = table.raw_get::<Option<String>>("text")?.unwrap_or_default();
    let (fg, style) = read_color(table)?;
    let (name_fg, name_style) = match table.raw_get::<Option<LuaTable>>("name")? {
        Some(name) => read_color(&name)?,
        None => (None, None),
    };
    let (after, after_fg, after_style) = match table.raw_get::<Option<LuaTable>>("after")? {
        Some(after) => {
            let text: String = after.raw_get::<Option<String>>("text")?.unwrap_or_default();
            let (fg, style) = read_color(&after)?;
            (text, fg, style)
        }
        None => (String::new(), None, None),
    };
    Ok(ExplorerDecoration {
        plugin,
        text,
        fg,
        style,
        name_fg,
        name_style,
        after,
        after_fg,
        after_style,
    })
}

fn path_string(path: Option<PathBuf>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}

/// Shows the explorer unless it is shown (`:explorer`).
fn show(lua: &Lua) -> LuaResult<()> {
    let visible = crate::lua::with_editor(|editor| Ok(editor.explorer_panel.visible))?;
    if !visible {
        super::editor::run_typable(lua, "explorer", &[], false)?;
    }
    Ok(())
}

pub fn register_explorer_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let explorer = lua.create_table()?;

    // helix.explorer.is_open() / is_focused()
    let is_open = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.explorer.is_open")?;
        crate::lua::with_editor(|editor| Ok(editor.explorer_panel.visible))
    })?;
    explorer.set("is_open", is_open)?;
    let is_focused = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.explorer.is_focused")?;
        crate::lua::with_editor(|editor| Ok(editor.explorer_panel.focused))
    })?;
    explorer.set("is_focused", is_focused)?;

    // helix.explorer.get_root() / get_selected() - The root of the tree and the path under
    // the cursor (nil until the explorer was shown)
    let get_root = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.explorer.get_root")?;
        crate::lua::with_editor(|editor| Ok(path_string(editor.plugin_explorer.root.clone())))
    })?;
    explorer.set("get_root", get_root)?;
    let get_selected = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.explorer.get_selected")?;
        crate::lua::with_editor(|editor| Ok(path_string(editor.plugin_explorer.selected.clone())))
    })?;
    explorer.set("get_selected", get_selected)?;

    // helix.explorer.open() / close() / focus() / refresh()
    let open = lua.create_function(|lua, ()| {
        require(lua, Permission::Explorer, "helix.explorer.open")?;
        show(lua)
    })?;
    explorer.set("open", open)?;
    let close = lua.create_function(|lua, ()| {
        require(lua, Permission::Explorer, "helix.explorer.close")?;
        crate::lua::with_editor(|editor| {
            editor.explorer_panel.hide();
            Ok(())
        })
    })?;
    explorer.set("close", close)?;
    let focus = lua.create_function(|lua, ()| {
        require(lua, Permission::Explorer, "helix.explorer.focus")?;
        super::editor::run_typable(lua, "explorer-focus", &[], false)
    })?;
    explorer.set("focus", focus)?;
    let refresh = lua.create_function(|lua, ()| {
        require(lua, Permission::Explorer, "helix.explorer.refresh")?;
        crate::lua::with_editor(|editor| {
            editor
                .plugin_explorer
                .requests
                .push(ExplorerRequest::Refresh);
            Ok(())
        })
    })?;
    explorer.set("refresh", refresh)?;

    // helix.explorer.reveal(path) - Show the explorer with the cursor on `path`
    let reveal = lua.create_function(|lua, path: String| {
        require(lua, Permission::Explorer, "helix.explorer.reveal")?;
        let path = helix_stdx::path::canonicalize(path);
        crate::lua::with_editor(|editor| {
            editor
                .plugin_explorer
                .requests
                .push(ExplorerRequest::Reveal(path));
            Ok(())
        })?;
        show(lua)
    })?;
    explorer.set("reveal", reveal)?;

    // helix.explorer.decorate(path, { text, fg, style, name = { fg, style }, after = { text,
    // fg, style } }) - What the calling plugin shows on the row of `path`: `text` at its right,
    // a color for the name, a text right after the name (`nil`: nothing). Colors are
    // `"#rrggbb"` (`fg`) or a theme key (`style`).
    let decorate = lua.create_function(|lua, (path, decoration): (String, Option<LuaTable>)| {
        require(lua, Permission::Ui, "helix.explorer.decorate")?;
        let path = helix_stdx::path::canonicalize(path);
        let plugin = crate::lua::current_plugin_name().to_string();
        let decoration = match decoration {
            Some(table) => Some(read_decoration(&table, plugin.clone())?),
            None => None,
        };
        crate::lua::with_editor(|editor| {
            match decoration {
                Some(decoration) => editor.plugin_explorer.decorate(path, decoration),
                None => editor.plugin_explorer.undecorate(&plugin, &path),
            }
            Ok(())
        })
    })?;
    explorer.set("decorate", decorate)?;

    // helix.explorer.clear_decorations() - Remove the calling plugin's decorations
    let clear = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.explorer.clear_decorations")?;
        let plugin = crate::lua::current_plugin_name();
        crate::lua::with_editor(|editor| {
            editor.plugin_explorer.clear_plugin(&plugin);
            Ok(())
        })
    })?;
    explorer.set("clear_decorations", clear)?;

    // helix.explorer.add_action({ key, desc, run }) / remove_action(key) - A key of the
    // explorer panel running `run({ path, is_dir })` with the entry under the cursor
    super::actions::register(lua, &explorer, ActionPlace::Explorer, Permission::Explorer)?;

    helix.set("explorer", explorer)?;
    Ok(())
}
