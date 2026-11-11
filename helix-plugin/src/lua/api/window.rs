use crate::error::Result;
use crate::permissions::{require, Permission};
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

    helix_table.set("window", window_module)?;

    Ok(())
}
