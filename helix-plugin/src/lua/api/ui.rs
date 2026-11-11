use crate::error::Result;
use crate::permissions::{require, Permission};
use mlua::prelude::*;
use std::sync::Arc;

/// Register UI API in the Helix Lua global table
pub fn register_ui_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let ui_module = lua.create_table()?;

    // helix.ui.notify(message, level) - level: "info" (default), "warning", "error"
    let notify = lua.create_function(|lua, (message, level): (String, Option<String>)| {
        require(lua, Permission::Ui, "helix.ui.notify")?;
        crate::lua::try_with_editor(|editor| {
            show_message(editor, message, level.as_deref());
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("notify", notify)?;

    // helix.ui.info(message) - Show info message
    let info = lua.create_function(|lua, message: String| {
        require(lua, Permission::Ui, "helix.ui.info")?;
        crate::lua::try_with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("info", info)?;

    // helix.ui.warn(message) - Show warning message
    let warn = lua.create_function(|lua, message: String| {
        require(lua, Permission::Ui, "helix.ui.warn")?;
        crate::lua::try_with_editor(|editor| {
            editor.set_warning(message);
            Ok(())
        })?;
        Ok(())
    })?;
    ui_module.set("warn", warn)?;

    // helix.ui.error(message) - Show error message
    let error = lua.create_function(|lua, message: String| {
        require(lua, Permission::Ui, "helix.ui.error")?;
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
            require(lua, Permission::Ui, "helix.ui.prompt")?;
            open_ui(lua, callback, move |handler, editor, plugin_name, id| {
                handler.prompt(editor, message, default, plugin_name, id)
            })
        },
    )?;
    ui_module.set("prompt", prompt)?;

    // helix.ui.confirm(message, callback) - Show confirmation dialog
    let confirm = lua.create_function(|lua, (message, callback): (String, LuaFunction)| {
        require(lua, Permission::Ui, "helix.ui.confirm")?;
        open_ui(lua, callback, move |handler, editor, plugin_name, id| {
            handler.confirm(editor, message, plugin_name, id)
        })
    })?;
    ui_module.set("confirm", confirm)?;

    // helix.ui.picker(options) - A picker over `options.items`: strings, or tables with
    // `text` (or `columns`, the text of each column), `path` or `buffer` to preview, and
    // `line`/`end_line` (1-based) to show. `on_select(item, action)` gets the item chosen
    // (the value from `items`) and how ("replace", "hsplit", "vsplit"). Returns a handle:
    // `handle:add(items)`, `handle:close()`.
    let picker = lua.create_function(|lua, options: LuaTable| {
        require(lua, Permission::Ui, "helix.ui.picker")?;
        let items: LuaTable = match options.get::<LuaValue>("items")? {
            LuaValue::Table(items) => items,
            LuaValue::Nil => lua.create_table()?,
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "items must be a list, not a {}",
                    other.type_name()
                )))
            }
        };
        let on_select: LuaFunction = options
            .get::<Option<LuaFunction>>("on_select")?
            .ok_or_else(|| LuaError::RuntimeError("on_select callback required".into()))?;
        let title: String = options
            .get::<Option<String>>("title")?
            .unwrap_or_else(|| "Plugin".to_string());
        let prompt: String = options
            .get::<Option<String>>("prompt")?
            .unwrap_or_else(|| "Select:".to_string());
        let columns: Vec<String> = options
            .get::<Option<Vec<String>>>("columns")?
            .unwrap_or_default();
        let preview: Option<bool> = options.get("preview")?;
        let docked: bool = options.get::<Option<bool>>("dock")?.unwrap_or(false);
        open_picker(
            lua,
            PickerOptions {
                items,
                on_select,
                title,
                prompt,
                columns,
                preview,
                docked,
            },
        )
    })?;
    ui_module.set("picker", picker)?;

    // helix.ui.menu(items, callback) - Pick one of `items` (a picker with the default label)
    let menu = lua.create_function(|lua, (items, callback): (LuaTable, LuaFunction)| {
        require(lua, Permission::Ui, "helix.ui.menu")?;
        open_picker(
            lua,
            PickerOptions {
                items,
                on_select: callback,
                title: "Plugin".to_string(),
                prompt: "Select:".to_string(),
                columns: Vec::new(),
                preview: None,
                docked: false,
            },
        )
    })?;
    ui_module.set("menu", menu)?;

    // helix.ui.set_style(scope, style) - Sets the style of a theme key ("ui.statusline",
    // "keyword", "diagnostic.error"...), written like in a theme; kept over theme changes
    let set_style = lua.create_function(|lua, (scope, style): (String, LuaTable)| {
        require(lua, Permission::Ui, "helix.ui.set_style")?;
        let (_, style) = super::style::read_style(lua, &style)?;
        crate::lua::with_editor(|editor| {
            editor.set_plugin_style(&scope, style);
            helix_event::request_redraw();
            Ok(())
        })
    })?;
    ui_module.set("set_style", set_style)?;

    // helix.ui.reset_style(scope) - The theme's own style of `scope` again
    let reset_style = lua.create_function(|lua, scope: String| {
        require(lua, Permission::Ui, "helix.ui.reset_style")?;
        crate::lua::with_editor(|editor| {
            editor
                .reset_plugin_style(&scope)
                .map_err(|e| LuaError::RuntimeError(format!("{e}")))
        })
    })?;
    ui_module.set("reset_style", reset_style)?;

    // helix.ui.popup{ text | lines | markdown, title, position, max_width, max_height,
    // persistent } - A popup; returns its handle: handle:update(options), handle:close()
    let popup = lua.create_function(|lua, options: LuaTable| {
        require(lua, Permission::Ui, "helix.ui.popup")?;
        let counter = lua
            .app_data_ref::<crate::types::UiCallbackCounter>()
            .map(|c| Arc::clone(&c.0));
        let Some(counter) = counter else {
            return Ok(LuaValue::Nil);
        };
        let id = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let popup = read_popup(lua, id, &options)?;
        if let Some(handler) = lua
            .app_data_ref::<crate::types::UiHandlerWrapper>()
            .map(|h| Arc::clone(&h.0))
        {
            handler.popup(popup);
        }
        lua.create_userdata(LuaPopupHandle { id })
            .map(LuaValue::UserData)
    })?;
    ui_module.set("popup", popup)?;

    // helix.ui.set_status(message) - Set status line message
    let set_status = lua.create_function(|lua, message: String| {
        require(lua, Permission::Ui, "helix.ui.set_status")?;
        crate::lua::with_editor(|editor| {
            editor.set_status(message);
            Ok(())
        })
    })?;
    ui_module.set("set_status", set_status)?;

    // helix.ui.get_theme() - Get current theme name
    let get_theme = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.ui.get_theme")?;
        crate::lua::with_editor(|editor| Ok(editor.theme.name().to_string()))
    })?;
    ui_module.set("get_theme", get_theme)?;

    // helix.ui.set_theme(name) - Set theme
    let set_theme = lua.create_function(|lua, name: String| {
        require(lua, Permission::Ui, "helix.ui.set_theme")?;
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
    let get_terminal_size = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.ui.get_terminal_size")?;
        let area = crate::lua::with_editor(|editor| Ok(editor.tree.area()))?;
        let size = lua.create_table()?;
        size.set("width", area.width)?;
        size.set("height", area.height)?;
        Ok(size)
    })?;
    ui_module.set("get_terminal_size", get_terminal_size)?;

    // helix.ui.redraw() - Force redraw
    let redraw = lua.create_function(|lua, ()| {
        require(lua, Permission::Ui, "helix.ui.redraw")?;
        crate::lua::with_editor(|editor| {
            editor.needs_redraw = true;
            Ok(())
        })
    })?;
    ui_module.set("redraw", redraw)?;

    // helix.ui.show_help(topic) - Not available: raise instead of silently doing nothing
    let show_help = lua.create_function(|_lua, _topic: Option<String>| -> LuaResult<()> {
        Err(LuaError::RuntimeError(
            "helix.ui.show_help is not implemented".to_string(),
        ))
    })?;
    ui_module.set("show_help", show_help)?;

    helix_table.set("ui", ui_module)?;

    Ok(())
}

/// Show `message` in the status line with the severity `level` ("info" by default,
/// "warn"/"warning", "error").
pub(crate) fn show_message(editor: &mut helix_view::Editor, message: String, level: Option<&str>) {
    match level {
        Some("error") => editor.set_error(message),
        Some("warn" | "warning") => editor.set_warning(message),
        _ => editor.set_status(message),
    }
}

/// Register `callback` as a pending UI callback and ask the UI handler to open
/// the prompt/picker. Does nothing when there is no editor context or no UI handler.
fn open_ui<F>(lua: &Lua, callback: LuaFunction, open: F) -> LuaResult<()>
where
    F: FnOnce(&dyn crate::types::UiHandler, &mut helix_view::Editor, String, u64),
{
    let plugin_name = crate::lua::current_plugin_name().to_string();

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

/// Where a later result goes: the UI handler to `deliver` it with, the plugin and the id
/// of its callback.
pub(crate) type PendingCallback = (Arc<dyn crate::types::UiHandler>, String, u64);

/// Keeps `callback` for a result that comes later from another thread (a program's output,
/// a language server's answer): returns the UI handler, the plugin and the callback's id to
/// `deliver` it with. `None` without an editor (nothing would ever deliver it).
pub(crate) fn pending_callback(
    lua: &Lua,
    callback: LuaFunction,
) -> LuaResult<Option<PendingCallback>> {
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
        return Ok(None);
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return Ok(None);
    }
    let plugin = crate::lua::current_plugin_name().to_string();
    let id = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let key = lua.create_registry_value(callback)?;
    callbacks.write().insert((plugin.clone(), id), key);
    Ok(Some((handler, plugin, id)))
}

/// A plugin popup's handle.
pub struct LuaPopupHandle {
    id: u64,
}

impl LuaUserData for LuaPopupHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // handle:update(options) - Shows other content (or puts it elsewhere): the options of
        // helix.ui.popup
        methods.add_method("update", |lua, this, options: LuaTable| {
            require(lua, Permission::Ui, "popup:update")?;
            let popup = read_popup(lua, this.id, &options)?;
            if let Some(handler) = lua
                .app_data_ref::<crate::types::UiHandlerWrapper>()
                .map(|h| Arc::clone(&h.0))
            {
                handler.popup(popup);
            }
            Ok(())
        });

        // handle:close() - Closes the popup
        methods.add_method("close", |lua, this, ()| {
            require(lua, Permission::Ui, "popup:close")?;
            if let Some(handler) = lua
                .app_data_ref::<crate::types::UiHandlerWrapper>()
                .map(|h| Arc::clone(&h.0))
            {
                handler.popup_close(this.id);
            }
            Ok(())
        });

        methods.add_method("id", |_lua, this, ()| Ok(this.id));
    }
}

/// A segment of a popup line: a string, or `{ text, style, fg, bg, modifiers }`.
fn read_segment(
    lua: &Lua,
    value: LuaValue,
) -> LuaResult<(String, Option<String>, helix_view::graphics::Style)> {
    match value {
        LuaValue::String(text) => Ok((text.to_str()?.to_string(), None, Default::default())),
        LuaValue::Table(table) => {
            let text: String = table.raw_get::<Option<String>>("text")?.unwrap_or_default();
            let (scope, style) = super::style::read_style(lua, &table)?;
            Ok((text, scope, style))
        }
        other => Err(LuaError::RuntimeError(format!(
            "A popup line is made of strings and tables, not a {}",
            other.type_name()
        ))),
    }
}

/// The popup of `helix.ui.popup`'s options.
fn read_popup(lua: &Lua, id: u64, options: &LuaTable) -> LuaResult<crate::types::PluginPopup> {
    use crate::types::{PopupContent, PopupPosition};
    let markdown: Option<String> = options.raw_get("markdown")?;
    let text: Option<String> = options.raw_get("text")?;
    let lines: Option<LuaTable> = options.raw_get("lines")?;
    let content = match (markdown, text, lines) {
        (Some(markdown), None, None) => PopupContent::Markdown(markdown),
        (None, Some(text), None) => PopupContent::Lines(
            text.lines()
                .map(|line| vec![(line.to_string(), None, Default::default())])
                .collect(),
        ),
        (None, None, Some(lines)) => {
            let mut content = Vec::new();
            for i in 1..=lines.raw_len() {
                let line: LuaValue = lines.raw_get(i)?;
                let segments = match line {
                    // a line of segments
                    LuaValue::Table(table) if table.raw_get::<LuaValue>("text")?.is_nil() => {
                        let mut segments = Vec::new();
                        for j in 1..=table.raw_len() {
                            segments.push(read_segment(lua, table.raw_get(j)?)?);
                        }
                        segments
                    }
                    other => vec![read_segment(lua, other)?],
                };
                if segments
                    .iter()
                    .any(|(text, _, _)| text.contains(['\n', '\r']))
                {
                    return Err(LuaError::RuntimeError(
                        "A line of `lines` has no line breaks".into(),
                    ));
                }
                content.push(segments);
            }
            PopupContent::Lines(content)
        }
        _ => {
            return Err(LuaError::RuntimeError(
                "A popup has one of `text`, `lines` and `markdown`".into(),
            ))
        }
    };
    let position = match options.raw_get::<LuaValue>("position")? {
        LuaValue::Nil => PopupPosition::Cursor,
        LuaValue::String(name) => match &*name.to_str()? {
            "cursor" => PopupPosition::Cursor,
            "center" => PopupPosition::Center,
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "Invalid position '{other}' (cursor, center or {{ row, col }})"
                )))
            }
        },
        LuaValue::Table(at) => PopupPosition::At {
            row: at.raw_get("row")?,
            col: at.raw_get("col")?,
        },
        other => {
            return Err(LuaError::RuntimeError(format!(
                "Invalid position: a name or {{ row, col }}, not a {}",
                other.type_name()
            )))
        }
    };
    Ok(crate::types::PluginPopup {
        id,
        title: options.raw_get("title")?,
        content,
        position,
        max_width: options.raw_get("max_width")?,
        max_height: options.raw_get("max_height")?,
        persistent: options
            .raw_get::<Option<bool>>("persistent")?
            .unwrap_or(false),
    })
}

/// What `helix.ui.picker` was given.
struct PickerOptions {
    items: LuaTable,
    on_select: LuaFunction,
    title: String,
    prompt: String,
    columns: Vec<String>,
    preview: Option<bool>,
    docked: bool,
}

/// A plugin picker's handle: items are added to the picker and its list (`items`, which the
/// selection callback reads).
pub struct LuaPickerHandle {
    id: u64,
    items: std::sync::Arc<LuaRegistryKey>,
    columns: usize,
}

impl LuaUserData for LuaPickerHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // handle:add(items) - Add items to the picker (and to its list)
        methods.add_method("add", |lua, this, new: LuaTable| {
            require(lua, Permission::Ui, "picker:add")?;
            let items: LuaTable = lua.registry_value(&this.items)?;
            let mut added = Vec::new();
            for i in 1..=new.raw_len() {
                let item: LuaValue = new.raw_get(i)?;
                let index = items.raw_len() + 1;
                added.push(read_item(&item, index, this.columns)?);
                items.raw_set(index, item)?;
            }
            if let Some(handler) = lua
                .app_data_ref::<crate::types::UiHandlerWrapper>()
                .map(|h| Arc::clone(&h.0))
            {
                handler.picker_add(this.id, added);
            }
            Ok(())
        });

        // handle:close() - Close the picker (its callback is not called)
        methods.add_method("close", |lua, this, ()| {
            require(lua, Permission::Ui, "picker:close")?;
            if let Some(handler) = lua
                .app_data_ref::<crate::types::UiHandlerWrapper>()
                .map(|h| Arc::clone(&h.0))
            {
                handler.picker_close(this.id);
            }
            Ok(())
        });

        methods.add_method("id", |_lua, this, ()| Ok(this.id));
    }
}

/// A picker item read from a plugin's `items[index]`.
fn read_item(
    item: &LuaValue,
    index: usize,
    columns: usize,
) -> LuaResult<crate::types::PluginPickerItem> {
    let mut picker_item = crate::types::PluginPickerItem {
        index,
        columns: Vec::new(),
        path: None,
        document: None,
        lines: None,
    };
    match item {
        LuaValue::String(text) => picker_item.columns.push(text.to_str()?.to_string()),
        LuaValue::Integer(_) | LuaValue::Number(_) => {
            picker_item.columns.push(item.to_string()?);
        }
        LuaValue::Table(table) => {
            // `raw_get`: no metamethods run here.
            let cells: Option<LuaTable> = table.raw_get("columns")?;
            match cells {
                Some(cells) => {
                    for i in 1..=cells.raw_len() {
                        let cell: LuaValue = cells.raw_get(i)?;
                        picker_item.columns.push(match cell {
                            LuaValue::Nil => String::new(),
                            LuaValue::String(s) => s.to_str()?.to_string(),
                            other => other.to_string()?,
                        });
                    }
                }
                None => {
                    let text: Option<String> = table.raw_get("text")?;
                    picker_item.columns.push(text.unwrap_or_default());
                }
            }
            let path: Option<String> = table.raw_get("path")?;
            picker_item.path = path.map(std::path::PathBuf::from);
            let buffer: LuaValue = table.raw_get("buffer")?;
            picker_item.document = match buffer {
                LuaValue::UserData(ud) => ud
                    .borrow::<super::buffer::LuaBuffer>()
                    .ok()
                    .map(|b| b.document_id),
                _ => None,
            };
            let line: Option<usize> = table.raw_get("line")?;
            let end_line: Option<usize> = table.raw_get("end_line")?;
            if let Some(line) = line {
                let start = line.saturating_sub(1);
                let end = end_line.map_or(start, |end| end.saturating_sub(1).max(start));
                picker_item.lines = Some((start, end));
            }
        }
        other => {
            return Err(LuaError::RuntimeError(format!(
                "A picker item is a string or a table, not a {}",
                other.type_name()
            )))
        }
    }
    picker_item.columns.resize(columns.max(1), String::new());
    Ok(picker_item)
}

/// Opens a plugin picker; returns its handle (nil without an editor).
fn open_picker(lua: &Lua, options: PickerOptions) -> LuaResult<LuaValue> {
    let columns = options.columns.len();
    if columns > crate::MAX_PICKER_COLUMNS {
        return Err(LuaError::RuntimeError(format!(
            "A picker has at most {} columns",
            crate::MAX_PICKER_COLUMNS
        )));
    }
    let mut items = Vec::new();
    for i in 1..=options.items.raw_len() {
        let item: LuaValue = options.items.raw_get(i)?;
        items.push(read_item(&item, i, columns)?);
    }
    let preview = options.preview.unwrap_or_else(|| {
        items
            .iter()
            .any(|i| i.path.is_some() || i.document.is_some())
    });
    let counter = lua
        .app_data_ref::<crate::types::UiCallbackCounter>()
        .map(|c| Arc::clone(&c.0));
    let Some(counter) = counter else {
        return Ok(LuaValue::Nil);
    };
    let id = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // The callback hands the plugin its own item: the list is kept (and grows with `add`).
    let list_key = std::sync::Arc::new(lua.create_registry_value(options.items.clone())?);
    let callback: LuaFunction = lua
        .load(
            r#"
            local items, on_select = ...
            return function(choice)
                return on_select(items[choice.index], choice.action)
            end
            "#,
        )
        .set_name("=picker")
        .call((options.items, options.on_select))?;
    let picker = crate::types::PluginPicker {
        id,
        title: options.title,
        prompt: options.prompt,
        columns: options.columns,
        items,
        preview,
        docked: options.docked,
    };
    open_ui(
        lua,
        callback,
        move |handler, editor, plugin_name, callback_id| {
            handler.open_picker(editor, picker, plugin_name, callback_id)
        },
    )?;
    lua.create_userdata(LuaPickerHandle {
        id,
        items: list_key,
        columns,
    })
    .map(LuaValue::UserData)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_api_registration() {
        let lua = Lua::new();
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
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
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
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
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
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
