//! Styles of plugins, written like in a theme: `{ fg = "red", bg = "#123456",
//! modifiers = { "bold" }, underline = { style = "curl", color = "red" } }`, and `style`, a
//! theme key they go over.

use helix_view::graphics::Style;
use mlua::prelude::*;

/// The style fields of `table` (`fg`, `bg`, `modifiers`, `underline`) and its theme key
/// (`style`). Read without running metamethods.
pub(crate) fn read_style(lua: &Lua, table: &LuaTable) -> LuaResult<(Option<String>, Style)> {
    let scope: Option<String> = table.raw_get("style")?;
    let values: [(&str, LuaValue); 4] = [
        ("fg", table.raw_get("fg")?),
        ("bg", table.raw_get("bg")?),
        ("modifiers", table.raw_get("modifiers")?),
        ("underline", table.raw_get("underline")?),
    ];
    // The usual style, colors only (a cell of an overlay, a sign): read straight, without
    // going through a theme's table. Plugins set thousands of them.
    let colors_only = values[2].1.is_nil()
        && values[3].1.is_nil()
        && values[..2]
            .iter()
            .all(|(_, value)| matches!(value, LuaValue::Nil | LuaValue::String(_)));
    if colors_only {
        let color = |value: &LuaValue| -> LuaResult<Option<helix_view::graphics::Color>> {
            let LuaValue::String(text) = value else {
                return Ok(None);
            };
            helix_view::theme::parse_color(&text.to_str()?)
                .map(Some)
                .map_err(|e| LuaError::RuntimeError(format!("Invalid style: {e}")))
        };
        let style = Style {
            fg: color(&values[0].1)?,
            bg: color(&values[1].1)?,
            ..Style::default()
        };
        return Ok((scope, style));
    }
    let mut fields = serde_json::Map::new();
    for (key, value) in values {
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

/// `style` as a table written like in a theme, the inverse of [`read_style`]: `fg`, `bg`
/// (`"#rrggbb"`, a color name such as `"light-blue"`, or a terminal palette index such as
/// `"8"`), `modifiers` (`{ "bold", ... }`) and `underline` (`{ style, color }`), each only
/// when the style sets it.
pub(crate) fn write_style(lua: &Lua, style: &Style) -> LuaResult<LuaTable> {
    use helix_view::graphics::{Color, Modifier, UnderlineStyle};
    fn color(color: Color) -> String {
        match color {
            Color::Reset => "default".into(),
            Color::Black => "black".into(),
            Color::Red => "red".into(),
            Color::Green => "green".into(),
            Color::Yellow => "yellow".into(),
            Color::Blue => "blue".into(),
            Color::Magenta => "magenta".into(),
            Color::Cyan => "cyan".into(),
            Color::Gray => "gray".into(),
            Color::LightRed => "light-red".into(),
            Color::LightGreen => "light-green".into(),
            Color::LightYellow => "light-yellow".into(),
            Color::LightBlue => "light-blue".into(),
            Color::LightMagenta => "light-magenta".into(),
            Color::LightCyan => "light-cyan".into(),
            Color::LightGray => "light-gray".into(),
            Color::White => "white".into(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Indexed(index) => index.to_string(),
        }
    }
    let table = lua.create_table()?;
    if let Some(fg) = style.fg {
        table.raw_set("fg", color(fg))?;
    }
    if let Some(bg) = style.bg {
        table.raw_set("bg", color(bg))?;
    }
    let names = [
        (Modifier::BOLD, "bold"),
        (Modifier::DIM, "dim"),
        (Modifier::ITALIC, "italic"),
        (Modifier::SLOW_BLINK, "slow_blink"),
        (Modifier::RAPID_BLINK, "rapid_blink"),
        (Modifier::REVERSED, "reversed"),
        (Modifier::HIDDEN, "hidden"),
        (Modifier::CROSSED_OUT, "crossed_out"),
    ];
    let modifiers: Vec<&str> = names
        .iter()
        .filter(|(modifier, _)| style.add_modifier.contains(*modifier))
        .map(|(_, name)| *name)
        .collect();
    if !modifiers.is_empty() {
        table.raw_set("modifiers", modifiers)?;
    }
    if style.underline_style.is_some() || style.underline_color.is_some() {
        let underline = lua.create_table()?;
        if let Some(kind) = style.underline_style {
            let name = match kind {
                UnderlineStyle::Reset => "reset",
                UnderlineStyle::Line => "line",
                UnderlineStyle::Curl => "curl",
                UnderlineStyle::Dotted => "dotted",
                UnderlineStyle::Dashed => "dashed",
                UnderlineStyle::DoubleLine => "double_line",
            };
            underline.raw_set("style", name)?;
        }
        if let Some(underline_color) = style.underline_color {
            underline.raw_set("color", color(underline_color))?;
        }
        table.raw_set("underline", underline)?;
    }
    Ok(table)
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

        // written back the same
        let written: LuaTable = write_style(&lua, &expected).unwrap();
        let (_, again): (Option<String>, Style) = read_style(&lua, &written).unwrap();
        assert_eq!(again, expected);
        let fg: String = written.raw_get("fg").unwrap();
        assert_eq!(fg, "red");
        let bg: String = written.raw_get("bg").unwrap();
        assert_eq!(bg, "#102030");
        let indexed: LuaTable = write_style(&lua, &Style::default().fg(Color::Indexed(8))).unwrap();
        let fg: String = indexed.raw_get("fg").unwrap();
        assert_eq!(fg, "8");
        let none: LuaTable = write_style(&lua, &Style::default()).unwrap();
        assert_eq!(none.pairs::<LuaValue, LuaValue>().count(), 0);

        // colors only (the direct path) read like the rest
        for code in [
            "{ fg = 'light-blue' }",
            "{ fg = '#102030', bg = '8' }",
            "{ bg = 'default', style = 'comment' }",
        ] {
            let colors: LuaTable = lua.load(code).eval().unwrap();
            let (_, direct): (Option<String>, Style) = read_style(&lua, &colors).unwrap();
            let with_bold: LuaTable = lua.load(code).eval().unwrap();
            with_bold.raw_set("modifiers", vec!["bold"]).unwrap();
            let (_, general): (Option<String>, Style) = read_style(&lua, &with_bold).unwrap();
            let general = general.remove_modifier(Modifier::BOLD);
            assert_eq!(direct.fg, general.fg, "{code}");
            assert_eq!(direct.bg, general.bg, "{code}");
        }
        let direct: LuaTable = lua.load("{ fg = 'light-blue' }").eval().unwrap();
        let (_, light_blue): (Option<String>, Style) = read_style(&lua, &direct).unwrap();
        assert_eq!(light_blue.fg, Some(Color::LightBlue));

        let bad: LuaTable = lua.load("{ fg = 'not-a-color' }").eval().unwrap();
        let error = read_style(&lua, &bad).unwrap_err().to_string();
        assert!(error.contains("Invalid style"), "{error}");
    }
}
