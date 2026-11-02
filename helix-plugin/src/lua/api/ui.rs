use crate::error::Result;
use mlua::prelude::*;
use std::sync::Arc;

/// Register UI API in the Helix Lua global table
pub fn register_ui_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let ui_module = lua.create_table()?;

    // helix.ui.notify(message, level) - Show notification
    let notify = lua.create_function(|_lua, (message, _level): (String, Option<String>)| {
        crate::lua::try_with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("notify", notify)?;

    // helix.ui.info(message) - Show info message
    let info = lua.create_function(|_lua, message: String| {
        crate::lua::try_with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("info", info)?;

    // helix.ui.warn(message) - Show warning message
    let warn = lua.create_function(|_lua, message: String| {
        crate::lua::try_with_editor(|editor| {
            editor.set_status(format!("Warning: {}", message));
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("warn", warn)?;

    // helix.ui.error(message) - Show error message
    let error = lua.create_function(|_lua, message: String| {
        crate::lua::try_with_editor(|editor| {
            editor.set_error(message);
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("error", error)?;

    // helix.ui.prompt(message, default, callback) - Show input prompt
    let prompt = lua.create_function(
        |lua, (message, default, callback): (String, Option<String>, LuaFunction)| {
            open_ui(lua, callback, move |handler, editor, plugin_name, id| {
                handler.prompt(editor, message, default, plugin_name, id)
            })
        },
    )?;
    ui_module.set("prompt", prompt)?;

    // helix.ui.confirm(message, callback) - Show confirmation dialog
    let confirm = lua.create_function(|lua, (message, callback): (String, LuaFunction)| {
        open_ui(lua, callback, move |handler, editor, plugin_name, id| {
            handler.confirm(editor, message, plugin_name, id)
        })
    })?;
    ui_module.set("confirm", confirm)?;

    // helix.ui.picker(options) - Show picker/menu
    let picker = lua.create_function(|lua, options: LuaTable| {
        // Extract picker options before touching the editor (`get` may run metamethods)
        let items: Vec<String> = options
            .get::<Option<Vec<String>>>("items")?
            .unwrap_or_default();
        let prompt_text: String = options
            .get::<Option<String>>("prompt")?
            .unwrap_or_else(|| "Select:".to_string());
        let callback: LuaFunction = options
            .get("on_select")
            .map_err(|_| LuaError::RuntimeError("on_select callback required".into()))?;

        open_ui(lua, callback, move |handler, editor, plugin_name, id| {
            handler.picker(editor, items, prompt_text, plugin_name, id)
        })
    })?;
    ui_module.set("picker", picker)?;

    // helix.ui.menu(items, callback) - Show menu
    let menu = lua.create_function(
        |_lua, (items, _callback): (Vec<String>, Option<LuaFunction>)| {
            // TODO: Implement actual menu
            Ok(format!("Would show menu with {} items", items.len()))
        },
    )?;
    ui_module.set("menu", menu)?;

    // helix.ui.set_status(message) - Set status line message
    let set_status = lua.create_function(|_lua, message: String| {
        crate::lua::with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })
    })?;
    ui_module.set("set_status", set_status)?;

    // helix.ui.get_theme() - Get current theme name
    let get_theme = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| Ok(editor.theme.name().to_string()))
    })?;
    ui_module.set("get_theme", get_theme)?;

    // helix.ui.set_theme(name) - Set theme
    let set_theme = lua.create_function(|_lua, name: String| {
        crate::lua::with_editor(|editor| match editor.theme_loader.load(&name) {
            Ok(theme) => editor.set_theme(theme).map_err(|e| {
                LuaError::RuntimeError(format!("Failed to set theme {}: {}", name, e))
            }),
            Err(e) => Err(LuaError::RuntimeError(format!(
                "Failed to load theme {}: {}",
                name, e
            ))),
        })
    })?;
    ui_module.set("set_theme", set_theme)?;

    // helix.ui.get_terminal_size() - Get terminal dimensions
    let get_terminal_size = lua.create_function(|_lua, ()| {
        let area = crate::lua::with_editor(|editor| Ok(editor.tree.area()))?;
        let size = _lua.create_table()?;
        size.set("width", area.width)?;
        size.set("height", area.height)?;
        Ok(size)
    })?;
    ui_module.set("get_terminal_size", get_terminal_size)?;

    // helix.ui.redraw() - Force redraw
    let redraw = lua.create_function(|_lua, ()| {
        crate::lua::with_editor(|editor| {
            editor.needs_redraw = true;
            Ok(())
        })
    })?;
    ui_module.set("redraw", redraw)?;

    // helix.ui.show_help(topic) - Show help
    let show_help = lua.create_function(|_lua, topic: Option<String>| {
        // TODO: Implement actual help display
        Ok(format!("Would show help for: {:?}", topic))
    })?;
    ui_module.set("show_help", show_help)?;

    helix_table.set("ui", ui_module)?;

    Ok(())
}

/// Register `callback` as a pending UI callback and ask the UI handler to open
/// the prompt/picker. Does nothing when there is no editor context or no UI handler.
fn open_ui<F>(lua: &Lua, callback: LuaFunction, open: F) -> LuaResult<()>
where
    F: FnOnce(&dyn crate::types::UiHandler, &mut helix_view::Editor, String, u64),
{
    let plugin_name = lua
        .globals()
        .get::<String>("_current_plugin_name")
        .unwrap_or_else(|_| "unknown".to_string());

    // Clone the shared state out of the app data so no borrow is held below.
    let handler = lua
        .app_data_ref::<crate::types::UiHandlerWrapper>()
        .map(|h| Arc::clone(&h.0));
    let callbacks = lua
        .app_data_ref::<crate::types::UiCallbackRegistry>()
        .map(|r| Arc::clone(&r.0));
    let counter = lua
        .app_data_ref::<crate::types::UiCallbackCounter>()
        .map(|c| Arc::clone(&c.0));
    let (Some(handler), Some(callbacks), Some(counter)) = (handler, callbacks, counter) else {
        return Ok(());
    };

    let callback_id = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let callback_ref = lua.create_registry_value(callback)?;
    let key = (plugin_name.clone(), callback_id);
    callbacks.write().insert(key.clone(), callback_ref);

    let opened = crate::lua::try_with_editor(|editor| {
        open(handler.as_ref(), editor, plugin_name, callback_id);
        Ok(())
    });
    if !matches!(opened, Ok(Some(()))) {
        // Nothing was opened, so nothing will ever release the callback.
        callbacks.write().remove(&key);
    }
    opened.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_api_registration() {
        let lua = Lua::new();
        let helix_table = lua.create_table().unwrap();

        let result = register_ui_api(&lua, &helix_table);
        assert!(result.is_ok());

        // Verify ui module exists with expected functions
        let ui_module: LuaTable = helix_table.get("ui").unwrap();
        assert!(ui_module.contains_key("notify").unwrap());
        assert!(ui_module.contains_key("info").unwrap());
        assert!(ui_module.contains_key("warn").unwrap());
        assert!(ui_module.contains_key("error").unwrap());
        assert!(ui_module.contains_key("prompt").unwrap());
        assert!(ui_module.contains_key("picker").unwrap());
        assert!(ui_module.contains_key("menu").unwrap());
    }

    #[test]
    fn test_ui_notification_functions() {
        let lua = Lua::new();
        let helix_table = lua.create_table().unwrap();
        register_ui_api(&lua, &helix_table).unwrap();

        // Test notification functions
        let code = r#"
            helix.ui.info("Test message")
            helix.ui.warn("Warning!")
            helix.ui.error("Error!")
        "#;

        lua.globals().set("helix", helix_table).unwrap();
        lua.load(code).exec().unwrap();
    }

    #[test]
    fn test_ui_picker() {
        let lua = Lua::new();
        let helix_table = lua.create_table().unwrap();
        register_ui_api(&lua, &helix_table).unwrap();

        // Test picker with options
        let code = r#"
            local result = helix.ui.picker({
                items = {"item1", "item2", "item3"},
                prompt = "Choose:",
                on_select = function(item)
                    print("Selected: " .. item)
                end
            })
            assert(result == nil)
        "#;

        lua.globals().set("helix", helix_table).unwrap();
        lua.load(code).exec().unwrap();
    }
}
