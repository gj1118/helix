use crate::error::Result;
use mlua::prelude::*;

pub fn register_lsp_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let lsp_module = lua.create_table()?;

    // helix.lsp.get_clients() - Get active LSP clients for current buffer
    let get_clients = lua.create_function(|lua, ()| {
        // This is a bit complex as we need to find which clients are attached to the doc
        // For now, let's just list all active clients in the registry
        let infos: Vec<(String, String)> = crate::lua::with_editor(|editor| {
            Ok(editor
                .language_servers
                .iter_clients()
                .map(|client| (client.name().to_string(), client.id().to_string()))
                .collect())
        })?;

        let clients = lua.create_table()?;
        for (i, (name, id)) in infos.into_iter().enumerate() {
            let c = lua.create_table()?;
            c.raw_set("name", name)?;
            c.raw_set("id", id)?;
            clients.raw_set(i + 1, c)?;
        }
        Ok(clients)
    })?;
    lsp_module.set("get_clients", get_clients)?;

    helix_table.set("lsp", lsp_module)?;

    Ok(())
}
