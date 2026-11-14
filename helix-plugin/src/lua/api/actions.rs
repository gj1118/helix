//! Plugin actions: keys of a plugin in the diff viewer (`helix.diff`), the explorer
//! (`helix.explorer`) and the quickfix panel (`helix.quickfix`), each running a function of
//! the plugin with what is under the cursor there.
//!
//! The editor keeps the keys ([`helix_view::plugin_ui::PluginActions`]); the functions stay
//! in the Lua state of their plugin, by id, until the action is removed or replaced.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::plugin_ui::{ActionContext, ActionPlace, PluginAction};
use helix_view::Editor;
use mlua::prelude::*;

/// The ids of the actions, over all plugins.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The key of the table of a state's action functions, by id, in its registry.
const FUNCTIONS: &str = "helix.actions";

/// The action functions of `lua`'s plugin.
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

/// The function of action `id` of `lua`'s plugin, if it has it.
pub(crate) fn function(lua: &Lua, id: u64) -> LuaResult<Option<LuaFunction>> {
    functions(lua)?.raw_get(id)
}

/// Installs `add_action`, `remove_action` and `list_actions` in `table` (`helix.diff`...): the
/// actions of `place`, which need `permission`.
pub(crate) fn register(
    lua: &Lua,
    table: &LuaTable,
    place: ActionPlace,
    permission: Permission,
) -> Result<()> {
    let namespace = place.as_str();

    // add_action({ key, desc, run }) - Bind `key` there to `run(context)`
    let add = lua.create_function(move |lua, spec: LuaTable| {
        let api = format!("helix.{namespace}.add_action");
        require(lua, permission, &api)?;
        let keys: String = match spec.raw_get::<Option<String>>("key")? {
            Some(keys) => keys,
            None => spec
                .raw_get::<Option<String>>("keys")?
                .ok_or_else(|| LuaError::RuntimeError(format!("{api} needs a `key`")))?,
        };
        let parsed = crate::lua::parse_keys(&keys)?;
        if place != ActionPlace::Diff && parsed.len() != 1 {
            return Err(LuaError::RuntimeError(format!(
                "{api}: the keys of the {namespace} panel are one key, not '{keys}'"
            )));
        }
        let run: LuaFunction = match spec.raw_get::<LuaValue>("run")? {
            LuaValue::Function(run) => run,
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "{api} needs a function `run` (got a value of type {})",
                    other.type_name()
                )))
            }
        };
        let plugin = crate::lua::current_plugin_name().to_string();
        let description: String = spec
            .raw_get::<Option<String>>("desc")?
            .unwrap_or_else(|| format!("{plugin}: {keys}"));
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let functions = functions(lua)?;
        functions.raw_set(id, run)?;
        let replaced = crate::lua::with_editor(|editor| {
            Ok(editor.plugin_actions.add(PluginAction {
                plugin: plugin.clone(),
                place,
                keys: parsed,
                description,
                id,
            }))
        })?;
        // the plugin's own function goes with its action (another plugin's stays in that
        // plugin's state, never called, until it reloads)
        if let Some(replaced) = replaced {
            functions.raw_set(replaced, LuaValue::Nil)?;
        }
        Ok(())
    })?;
    table.set("add_action", add)?;

    // remove_action(key) - Remove the calling plugin's action of `key` there
    let remove = lua.create_function(move |lua, keys: String| {
        require(lua, permission, &format!("helix.{namespace}.remove_action"))?;
        let parsed = crate::lua::parse_keys(&keys)?;
        let plugin = crate::lua::current_plugin_name();
        let removed = crate::lua::with_editor(|editor| {
            Ok(editor.plugin_actions.remove(&plugin, place, &parsed))
        })?;
        if let Some(id) = removed {
            functions(lua)?.raw_set(id, LuaValue::Nil)?;
        }
        Ok(removed.is_some())
    })?;
    table.set("remove_action", remove)?;

    // list_actions() - The actions of all plugins there: { { key, desc, plugin } }
    let list = lua.create_function(move |lua, ()| {
        require(
            lua,
            Permission::Read,
            &format!("helix.{namespace}.list_actions"),
        )?;
        let actions: Vec<(String, String, String)> = crate::lua::with_editor(|editor| {
            Ok(editor
                .plugin_actions
                .of(place)
                .map(|action| {
                    let keys: String = action
                        .keys
                        .iter()
                        .map(|key| key.key_sequence_format())
                        .collect();
                    (keys, action.description.clone(), action.plugin.clone())
                })
                .collect())
        })?;
        let list = lua.create_table()?;
        for (keys, description, plugin) in actions {
            let entry = lua.create_table()?;
            entry.set("key", keys)?;
            entry.set("desc", description)?;
            entry.set("plugin", plugin)?;
            list.raw_push(entry)?;
        }
        Ok(list)
    })?;
    table.set("list_actions", list)?;
    Ok(())
}

/// What an action's function gets: `context` as a table. Run with the editor.
pub(crate) fn context_table(
    lua: &Lua,
    editor: &Editor,
    context: &ActionContext,
) -> LuaResult<LuaValue> {
    match context {
        ActionContext::Diff { view } => match editor.diff_session(*view) {
            Some(session) => {
                let table = super::diff::session_table(lua, editor, session)?;
                if let Some(change) = super::diff::change_under_cursor(lua, editor, *view)? {
                    table.set("change", change)?;
                }
                Ok(LuaValue::Table(table))
            }
            None => Ok(LuaValue::Nil),
        },
        ActionContext::Explorer { path, is_dir } => {
            let table = lua.create_table()?;
            table.set("path", path.to_string_lossy().into_owned())?;
            table.set("is_dir", *is_dir)?;
            Ok(LuaValue::Table(table))
        }
        ActionContext::Quickfix {
            target,
            start_line,
            end_line,
            text,
        } => {
            let table = lua.create_table()?;
            match target {
                helix_view::quicklist::Target::Path(path) => {
                    table.set("path", path.to_string_lossy().into_owned())?
                }
                helix_view::quicklist::Target::Doc(id) => {
                    table.set("buffer", super::buffer::LuaBuffer::new(*id))?
                }
            }
            table.set("line", start_line + 1)?;
            table.set("end_line", end_line + 1)?;
            table.set("text", text.as_str())?;
            Ok(LuaValue::Table(table))
        }
    }
}
