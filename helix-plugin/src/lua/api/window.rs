use crate::error::Result;
use helix_view::ViewId;
use mlua::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct LuaWindow {
    pub view_id: ViewId,
}

impl LuaUserData for LuaWindow {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // Get window ID
        methods.add_method("id", |_lua, this, ()| Ok(format!("{:?}", this.view_id)));

        // Get buffer in this window
        methods.add_method("get_buffer", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let view = editor.tree.try_get(this.view_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Window {:?} no longer exists", this.view_id))
            })?;
            Ok(super::buffer::LuaBuffer::new(view.doc))
        });

        // Focus this window
        methods.add_method("focus", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            if editor.tree.contains(this.view_id) {
                editor.tree.focus = this.view_id;
            } else {
                return Err(LuaError::RuntimeError(format!(
                    "Window {:?} no longer exists",
                    this.view_id
                )));
            }
            Ok(())
        });
    }
}

pub fn register_window_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let window_module = lua.create_table()?;

    // helix.window.get_current() - Get focused window
    let get_current = lua.create_function(|_lua, ()| {
        let editor = crate::lua::get_editor_mut()?;
        Ok(LuaWindow {
            view_id: editor.tree.focus,
        })
    })?;
    window_module.set("get_current", get_current)?;

    // helix.window.list() - List all windows
    let list = lua.create_function(|lua, ()| {
        let editor = crate::lua::get_editor_mut()?;
        let windows = lua.create_table()?;
        for (i, (view, _focused)) in editor.tree.views().enumerate() {
            windows.set(i + 1, LuaWindow { view_id: view.id })?;
        }
        Ok(windows)
    })?;
    window_module.set("list", list)?;

    helix_table.set("window", window_module)?;

    Ok(())
}
