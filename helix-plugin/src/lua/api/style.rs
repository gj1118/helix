//! Styles of plugins, written like in a theme: `{ fg = "red", bg = "#123456",
//! modifiers = { "bold" }, underline = { style = "curl", color = "red" } }`, and `style`, a
//! theme key they go over.

use helix_view::graphics::Style;
use mlua::prelude::*;

/// The style fields of `table` (`fg`, `bg`, `modifiers`, `underline`) and its theme key
/// (`style`). Read without running metamethods.
pub(crate) fn read_style(lua: &Lua, table: &LuaTable) -> LuaResult<(Option<String>, Style)> {
    let scope: Option<String> = table.raw_get("style")?;
    let mut fields = serde_json::Map::new();
    for key in ["fg", "bg", "modifiers", "underline"] {
        let value: LuaValue = table.raw_get(key)?;
        if value.is_nil() {
            continue;
        }
        let value: serde_json::Value = lua.from_value(value)?;
        fields.insert(key.to_string(), value);
    }
    if fields.is_empty() {
        return Ok((scope, Style::default()));
    }
    let value = toml::Value::try_from(serde_json::Value::Object(fields))
        .map_err(|e| LuaError::RuntimeError(format!("Invalid style: {e}")))?;
    let style = helix_view::theme::parse_style(value)
        .map_err(|e| LuaError::RuntimeError(format!("Invalid style: {e}")))?;
    Ok((scope, style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::graphics::{Color, Modifier, UnderlineStyle};

    #[test]
    fn styles_are_read_like_in_a_theme() {
        let lua = Lua::new();
        let table: LuaTable = lua
            .load(
                r##"{ fg = "red", bg = "#102030", modifiers = { "bold", "italic" },
                     underline = { style = "curl", color = "blue" }, style = "comment" }"##,
            )
            .eval()
            .unwrap();
        let (scope, style) = read_style(&lua, &table).unwrap();
        assert_eq!(scope.as_deref(), Some("comment"));
        let expected = Style::default()
            .fg(Color::Red)
            .bg(Color::Rgb(0x10, 0x20, 0x30))
            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
            .underline_style(UnderlineStyle::Curl)
            .underline_color(Color::Blue);
        assert_eq!(style, expected);

        let empty: LuaTable = lua.load("{ text = 'x' }").eval().unwrap();
        let (scope, style): (Option<String>, Style) = read_style(&lua, &empty).unwrap();
        assert_eq!((scope, style), (None, Style::default()));

        let bad: LuaTable = lua.load("{ fg = 'not-a-color' }").eval().unwrap();
        let error = read_style(&lua, &bad).unwrap_err().to_string();
        assert!(error.contains("Invalid style"), "{error}");
    }
}
