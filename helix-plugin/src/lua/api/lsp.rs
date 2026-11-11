use crate::error::Result;
use crate::permissions::{require, Permission};
use mlua::prelude::*;

/// Requests that only read (`textDocument/*`, `workspace/symbol`) need `read`; the others
/// (`workspace/executeCommand`, a server's own methods...) can make the server do anything:
/// `shell`.
fn require_for_method(lua: &Lua, method: &str) -> LuaResult<()> {
    let reads = method.starts_with("textDocument/")
        || method == "workspace/symbol"
        || method.starts_with("workspaceSymbol/");
    let permission = if reads {
        Permission::Read
    } else {
        Permission::Shell
    };
    require(
        lua,
        permission,
        &format!("the language server request {method}"),
    )
}

/// `(callback)` or `(options, callback)`.
fn options_and_callback(rest: LuaMultiValue) -> LuaResult<(Option<LuaTable>, LuaFunction)> {
    let mut values = rest.into_iter();
    match (values.next(), values.next()) {
        (Some(LuaValue::Function(callback)), None) => Ok((None, callback)),
        (Some(LuaValue::Table(options)), Some(LuaValue::Function(callback))) => {
            Ok((Some(options), callback))
        }
        (Some(LuaValue::Nil), Some(LuaValue::Function(callback))) => Ok((None, callback)),
        _ => Err(LuaError::RuntimeError(
            "Expected options (a table, optional) and a callback function".into(),
        )),
    }
}

/// The callback gets `(result, error)` from `{ result = ..., error = ... }`.
fn wrap(lua: &Lua, callback: LuaFunction) -> LuaResult<LuaFunction> {
    lua.load(
        r#"
        local callback = ...
        return function(answer)
            return callback(answer.result, answer.error)
        end
        "#,
    )
    .set_name("=lsp")
    .call(callback)
}

/// The language server of `options.buffer` (default: the focused buffer) named
/// `options.server`, or its first one.
fn choose_server(
    lua: &Lua,
    options: Option<&LuaTable>,
) -> LuaResult<std::sync::Arc<helix_lsp::Client>> {
    let _ = lua;
    let (buffer, server): (Option<helix_view::DocumentId>, Option<String>) = match options {
        Some(options) => {
            let buffer = match options.raw_get::<LuaValue>("buffer")? {
                LuaValue::UserData(ud) => Some(
                    ud.borrow::<super::buffer::LuaBuffer>()
                        .map_err(|_| LuaError::RuntimeError("`buffer` must be a buffer".into()))?
                        .document_id,
                ),
                _ => None,
            };
            (buffer, options.raw_get("server")?)
        }
        None => (None, None),
    };
    crate::lua::with_editor(|editor| {
        let doc = match buffer {
            Some(id) => editor
                .document(id)
                .ok_or_else(|| LuaError::RuntimeError("The buffer is closed".to_string()))?,
            None => crate::lua::focused_ref(editor)?.1,
        };
        let client = doc
            .language_servers()
            .find(|ls| server.as_deref().is_none_or(|name| ls.name() == name))
            .ok_or_else(|| {
                LuaError::RuntimeError(match &server {
                    Some(name) => format!("No language server '{name}' for the buffer"),
                    None => "No language server for the buffer".to_string(),
                })
            })?;
        editor
            .language_servers
            .get_by_id(client.id())
            .cloned()
            .ok_or_else(|| LuaError::RuntimeError("The language server stopped".to_string()))
    })
}

pub fn register_lsp_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let lsp_module = lua.create_table()?;

    // helix.lsp.get_clients() - Get active LSP clients for current buffer
    let get_clients = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.lsp.get_clients")?;
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

    // helix.lsp.request(method, params, { buffer, server }, function(result, error)) - Sends a
    // request to a language server of `buffer` (default: the focused one), the one named
    // `server` or the first; the answer comes to the callback
    let request = lua.create_function(
        |lua, (method, params, rest): (String, LuaValue, LuaMultiValue)| {
            require_for_method(lua, &method)?;
            let (options, callback) = options_and_callback(rest)?;
            let params: serde_json::Value = lua.from_value(params)?;
            let client = choose_server(lua, options.as_ref())?;
            let Some((handler, plugin, id)) =
                super::ui::pending_callback(lua, wrap(lua, callback)?)?
            else {
                return Err(LuaError::RuntimeError(
                    "Language servers can only be asked with the editor running".into(),
                ));
            };
            let future = client.call_raw(method, params);
            tokio::spawn(async move {
                let value = match future.await {
                    Ok(result) => serde_json::json!({ "result": result }),
                    Err(error) => serde_json::json!({ "error": error.to_string() }),
                };
                handler.deliver(plugin, id, value);
            });
            Ok(())
        },
    )?;
    lsp_module.set("request", request)?;

    // helix.lsp.notify(method, params, { buffer, server }) - Sends a notification
    let notify = lua.create_function(
        |lua, (method, params, options): (String, LuaValue, Option<LuaTable>)| {
            require(lua, Permission::Shell, "helix.lsp.notify")?;
            let params: serde_json::Value = lua.from_value(params)?;
            let client = choose_server(lua, options.as_ref())?;
            client.notify_raw(method, params);
            Ok(())
        },
    )?;
    lsp_module.set("notify", notify)?;

    // helix.lsp.position_params({ server }) - `{ textDocument = { uri }, position = { line,
    // character } }` of the focused window's cursor, in the server's position encoding: the
    // params of most `textDocument/*` requests
    let position_params = lua.create_function(|lua, options: Option<LuaTable>| {
        require(lua, Permission::Read, "helix.lsp.position_params")?;
        let server: Option<String> = match &options {
            Some(options) => options.raw_get("server")?,
            None => None,
        };
        let value = crate::lua::with_editor(|editor| {
            let (view, doc) = crate::lua::focused_ref(editor)?;
            let uri = doc
                .url()
                .ok_or_else(|| LuaError::RuntimeError("The buffer has no file".to_string()))?;
            let encoding = doc
                .language_servers()
                .find(|ls| server.as_deref().is_none_or(|name| ls.name() == name))
                .map(|ls| ls.offset_encoding())
                .unwrap_or_default();
            let text = doc.text();
            let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
            let position = helix_lsp::util::pos_to_lsp_pos(text, cursor, encoding);
            Ok(serde_json::json!({
                "textDocument": { "uri": uri.to_string() },
                "position": { "line": position.line, "character": position.character },
            }))
        })?;
        lua.to_value(&value)
    })?;
    lsp_module.set("position_params", position_params)?;

    helix_table.set("lsp", lsp_module)?;

    Ok(())
}
