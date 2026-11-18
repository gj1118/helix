use std::fs;

use helix_term::application::Application;
use helix_view::graphics::Color;

use super::*;

/// The background of `ui.cursor.match` in the theme of the test.
const MATCH_BG: Color = Color::Rgb(0x00, 0xc0, 0x00);

/// A theme of its own: the default theme depends on the terminal (true colors or 16 colors),
/// and so would `ui.cursor.match`.
fn theme() -> anyhow::Result<helix_view::Theme> {
    Ok(toml::from_str(
        r##""ui.background" = { bg = "#101010" }
"ui.text" = "#d0d0d0"
"ui.selection" = { bg = "#303030" }
"ui.cursor" = { fg = "#101010", bg = "#4080c0" }
"ui.cursor.match" = { fg = "#ffffff", bg = "#00c000" }"##,
    )?)
}

/// Whether the cell of the first `ch` of the row showing `line` has the background of
/// `ui.cursor.match`.
fn matched(app: &Application, line: &str, ch: char) -> bool {
    let rows = app.rendered_rows();
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.contains(line))
        .unwrap_or_else(|| panic!("no {line:?}:\n{}", rows.join("\n")));
    // (the line starts after the gutters, whose cells are one wide each)
    let x = row.chars().position(|c| c == ch).expect("the character");
    app.rendered_cell(x as u16, y as u16).bg == MATCH_BG
}

/// Both brackets of a pair get `ui.cursor.match` with a bar cursor (insert mode here): the
/// bar does not mark the one under it, as a block cursor does. With a block cursor only the
/// other one does, as before.
#[tokio::test(flavor = "multi_thread")]
async fn both_brackets_are_highlighted_without_a_block_cursor() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::with_suffix(".rs")?;
    fs::write(file.path(), "fn first() {}\n")?;
    let mut config = helpers::test_config();
    config.editor.cursor_shape =
        toml::from_str("normal = \"block\"\ninsert = \"bar\"\nselect = \"underline\"\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    app.editor.set_theme(theme()?)?;
    test_key_sequences(
        &mut app,
        vec![
            // the block cursor on `)`: the `(` is highlighted, the `)` has the cursor
            (
                Some("9l"),
                Some(&|app| {
                    assert!(matched(app, "fn first()", '('));
                    assert!(!matched(app, "fn first()", ')'));
                }),
            ),
            // the bar cursor before `)` (insert mode): both are
            (
                Some("i"),
                Some(&|app| {
                    assert!(matched(app, "fn first()", '('));
                    assert!(matched(app, "fn first()", ')'));
                    // not the rest of the line
                    assert!(!matched(app, "fn first()", 'f'));
                }),
            ),
            // no bracket under the cursor: none
            (
                Some("<esc>gh"),
                Some(&|app| {
                    assert!(!matched(app, "fn first()", '('));
                    assert!(!matched(app, "fn first()", ')'));
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
