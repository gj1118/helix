use std::fs;

use helix_term::application::Application;

use super::*;

/// Whether the cell of the first `ch` of the row showing `line` has the style of
/// `ui.cursor.match` (its foreground and underline).
fn matched(app: &Application, line: &str, ch: char) -> bool {
    let rows = app.rendered_rows();
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.contains(line))
        .unwrap_or_else(|| panic!("no {line:?}:\n{}", rows.join("\n")));
    // (the line starts after the gutters, whose cells are one wide each)
    let x = row.chars().position(|c| c == ch).expect("the character");
    let cell = app.rendered_cell(x as u16, y as u16);
    let style = app.editor.theme.get("ui.cursor.match");
    let fg = style.fg.expect("a foreground for ui.cursor.match");
    let underline = style
        .underline_style
        .expect("an underline for ui.cursor.match");
    cell.fg == fg && cell.underline_style == underline
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
