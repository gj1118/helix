//! Completion sources of plugins (`helix.completion`): a function of the plugin, asked for
//! items while typing, whose items join the completion menu beside the language servers'.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;
use crate::permissions::{require, Permission};
use helix_view::plugin_ui::{
    PluginCompletionItem, PluginCompletionRequest, PluginCompletionSource,
};
use mlua::prelude::*;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The key of the table of a state's completion functions, by id, in its registry.
const FUNCTIONS: &str = "helix.completion";

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

/// A list of strings, or one string, as given for `languages` and `triggers`.
fn strings(value: LuaValue) -> LuaResult<Vec<String>> {
    match value {
        LuaValue::Nil => Ok(Vec::new()),
        LuaValue::String(s) => Ok(vec![s.to_str()?.to_string()]),
        LuaValue::Table(table) => table.sequence_values::<String>().collect(),
        other => Err(LuaError::RuntimeError(format!(
            "a string or a list of strings was expected, not a {}",
            other.type_name()
        ))),
    }
}

pub fn register_completion_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let completion = lua.create_table()?;

    // register_source({ name, complete, languages, triggers }) - a source of the calling
    // plugin; a source of the same name (of this plugin) is replaced
    let register = lua.create_function(|lua, spec: LuaTable| {
        require(lua, Permission::Ui, "helix.completion.register_source")?;
        let name: String = spec.raw_get::<Option<String>>("name")?.ok_or_else(|| {
            LuaError::RuntimeError("helix.completion.register_source needs a `name`".into())
        })?;
        let complete: LuaFunction = match spec.raw_get::<LuaValue>("complete")? {
            LuaValue::Function(complete) => complete,
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "helix.completion.register_source needs a function `complete` (got a value of type {})",
                    other.type_name()
                )))
            }
        };
        let languages = strings(spec.raw_get("languages")?)?;
        let triggers = strings(spec.raw_get("triggers")?)?;
        let plugin = crate::lua::current_plugin_name().to_string();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let functions = functions(lua)?;
        functions.raw_set(id, complete)?;
        let replaced = crate::lua::with_editor(|editor| {
            let sources = &mut editor.plugin_completion_sources;
            let replaced = sources
                .iter()
                .position(|source| source.plugin == plugin && source.name == name)
                .map(|index| sources.remove(index).id);
            sources.push(PluginCompletionSource {
                plugin: plugin.clone(),
                id,
                name: name.clone(),
                languages,
                triggers,
            });
            Ok(replaced)
        })?;
        if let Some(replaced) = replaced {
            functions.raw_set(replaced, LuaValue::Nil)?;
        }
        Ok(())
    })?;
    completion.set("register_source", register)?;

    // remove_source(name) - removes the calling plugin's source
    let remove = lua.create_function(|lua, name: String| {
        require(lua, Permission::Ui, "helix.completion.remove_source")?;
        let plugin = crate::lua::current_plugin_name().to_string();
        let removed = crate::lua::with_editor(|editor| {
            let sources = &mut editor.plugin_completion_sources;
            Ok(sources
                .iter()
                .position(|source| source.plugin == plugin && source.name == name)
                .map(|index| sources.remove(index).id))
        })?;
        if let Some(id) = removed {
            functions(lua)?.raw_set(id, LuaValue::Nil)?;
        }
        Ok(removed.is_some())
    })?;
    completion.set("remove_source", remove)?;

    // list_sources() - every plugin's: { { name, plugin, languages, triggers } }
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.completion.list_sources")?;
        let sources =
            crate::lua::with_editor(|editor| Ok(editor.plugin_completion_sources.clone()))?;
        let list = lua.create_table()?;
        for source in sources {
            let entry = lua.create_table()?;
            entry.set("name", source.name)?;
            entry.set("plugin", source.plugin)?;
            entry.set("languages", lua.create_sequence_from(source.languages)?)?;
            entry.set("triggers", lua.create_sequence_from(source.triggers)?)?;
            list.raw_push(entry)?;
        }
        Ok(list)
    })?;
    completion.set("list_sources", list)?;

    helix.set("completion", completion)?;
    Ok(())
}

/// Asks source `id` of `lua`'s plugin: calls its function with the request (as a table),
/// and reads the items it returns. Runs with the editor.
pub(crate) fn complete(
    lua: &Lua,
    id: u64,
    request: &PluginCompletionRequest,
) -> LuaResult<Vec<PluginCompletionItem>> {
    let Some(function) = functions(lua)?.raw_get::<Option<LuaFunction>>(id)? else {
        return Ok(Vec::new());
    };
    let context = lua.create_table()?;
    context.set("buffer", super::buffer::LuaBuffer::new(request.document))?;
    context.set("line", request.line + 1)?;
    context.set("col", request.col)?;
    context.set("prefix", request.prefix.as_str())?;
    context.set("before", request.before.as_str())?;
    context.set("manual", request.manual)?;
    let items: LuaValue = function.call(context)?;
    let LuaValue::Table(items) = items else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for item in items.sequence_values::<LuaValue>() {
        match item? {
            LuaValue::String(label) => found.push(PluginCompletionItem {
                label: label.to_str()?.to_string(),
                insert: None,
                kind: None,
                documentation: None,
            }),
            LuaValue::Table(item) => {
                let Some(label) = item.raw_get::<Option<String>>("label")? else {
                    return Err(LuaError::RuntimeError(
                        "a completion item needs a `label`".into(),
                    ));
                };
                found.push(PluginCompletionItem {
                    label,
                    insert: item.raw_get("insert")?,
                    kind: item.raw_get("kind")?,
                    documentation: item.raw_get("documentation")?,
                });
            }
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "a completion item is a string or a table, not a {}",
                    other.type_name()
                )))
            }
        }
    }
    Ok(found)
}
