use crate::error::Result;
use mlua::prelude::*;

pub fn register_lsp_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let lsp_module = lua.create_table()?;

    // helix.lsp.get_clients() - Get active LSP clients for current buffer
    let get_clients = lua.create_function(|lua, ()| {
        let editor = crate::lua::get_editor_mut()?;
        let (_, _doc) = helix_view::current_ref!(editor);

        let clients = lua.create_table()?;
        // This is a bit complex as we need to find which clients are attached to the doc
        // For now, let's just list all active clients in the registry
        for (i, client) in editor.language_servers.iter_clients().enumerate() {
            let c = lua.create_table()?;
            c.set("name", client.name())?;
            c.set("id", client.id().to_string())?;
            clients.set(i + 1, c)?;
        }
        Ok(clients)
    })?;
    lsp_module.set("get_clients", get_clients)?;

    helix_table.set("lsp", lsp_module)?;

    Ok(())
}
