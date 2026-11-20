//! The `pile` plugin of `contrib/plugins`: the chunk and line_num parts of hlchunk.nvim.
use super::*;

use helix_term::application::Application;
use helix_view::graphics::Color;

async fn send_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent};

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut rx).await;
    Ok(())
}

/// Lines 3 to 8 are the `if` block; line 7 (`b";`) is the end of a string, not indented.
const RUST: &str = "fn main() {
    for item in list {
        if item.ready {
            item.run();

            let s = \"a
b\";
        }
    }
}
";

const VIOLET: Color = Color::Rgb(0x80, 0x6d, 0x9c);
const RED: Color = Color::Rgb(0xc2, 0x1f, 0x30);

fn file(text: &str, suffix: &str) -> anyhow::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new().suffix(suffix).tempfile()?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// An app on 80x24 showing `file`, with the plugin loaded from `contrib/plugins/pile` with
/// `config` (a TOML inline table) and the permissions it asks for.
async fn app(
    config: &str,
    file: &tempfile::NamedTempFile,
) -> anyhow::Result<(Application, tempfile::TempDir)> {
    let plugins = tempfile::tempdir()?;
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/../contrib/plugins/pile");
    let dir = plugins.path().join("pile");
    std::fs::create_dir(&dir)?;
    for name in ["init.lua", "plugin.toml"] {
        std::fs::copy(format!("{source}/{name}"), dir.join(name))?;
    }
    let mut app_config = helpers::test_config();
    app_config.editor.notifications.enable = false;
    app_config.editor.true_color = true;
    app_config.plugins = Some(toml::from_str(&format!(
        "plugin_dirs = [{:?}]\n[[plugins]]\nname = \"pile\"\n\
         permissions = [\"commands\", \"keymaps\", \"edit\"]\nconfig = {config}\n",
        plugins.path()
    ))?);
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(app_config)
        .build()?;
    assert!(app.plugins_enabled());
    app.resize_terminal(80, 24).await;
    send_keys(&mut app, "").await?;
    Ok((app, plugins))
}

/// The plugin's overlays in the focused window: `(line (1-based), col, text)`.
fn overlays(app: &Application) -> Vec<(usize, usize, String)> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.plugin_overlays
        .get(&view.id)
        .into_iter()
        .flatten()
        .filter(|o| o.plugin == "pile")
        .map(|o| (o.line + 1, o.col, o.text.clone()))
        .collect()
}

/// Where the first line of the test files starts in a row of the screen.
fn first_text(row: &str) -> Option<usize> {
    ["fn main", "a {", "package main"]
        .iter()
        .find_map(|start| row.find(start))
}

/// The screen's rows of the text, from the column of the text (after the gutter).
fn text_rows(app: &Application) -> Vec<String> {
    let rows = app.rendered_rows();
    let first = rows
        .iter()
        .find_map(|row| first_text(row).map(|_| row))
        .expect("the first line is shown");
    let at = first_text(first)
        .map(|byte| first[..byte].chars().count())
        .unwrap();
    rows.iter()
        .map(|row| {
            row.chars()
                .skip(at)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The column of the text on the screen.
fn text_column(app: &Application) -> u16 {
    let rows = app.rendered_rows();
    let byte = rows[0].find("fn main").expect("line 1 first");
    rows[0][..byte].chars().count() as u16
}

/// The chunk of the block around the cursor is drawn over the indentation: the corner and
/// the horizontal line before the first line's code, the vertical line beside the lines in
/// between (not beside one less indented than it, like the end of a string), the arrow before
/// the last line's code; in the chunk's colour.
#[tokio::test(flavor = "multi_thread")]
async fn the_chunk_of_the_block_is_drawn() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 0 } }", &rust).await?;
    send_keys(&mut app, "4gg").await?;
    let rows = text_rows(&app);
    let context = rows[..10].join("\n");
    assert_eq!(
        rows[2..8],
        [
            "    ╭───if item.ready {",
            "    │       item.run();",
            "    │",
            "    │       let s = \"a",
            "b\";",
            "    ╰──>}",
        ],
        "{context}"
    );
    let corner: Color = app.rendered_fg(text_column(&app) + 4, 2);
    assert_eq!(corner, VIOLET);
    // the numbers of the lines are left alone (line_num is off)
    let (view, doc) = helix_view::current_ref!(app.editor);
    assert!(!doc.plugin_line_numbers.contains_key(&view.id));

    // up into the `for` block: its chunk
    send_keys(&mut app, "2gg").await?;
    let rows = text_rows(&app);
    let context = rows[..10].join("\n");
    assert_eq!(rows[1], "╭───for item in list {", "{context}");
    assert_eq!(rows[2], "│       if item.ready {", "{context}");
    assert_eq!(rows[6], "b\";", "{context}");
    assert_eq!(rows[8], "╰──>}", "{context}");
    Ok(())
}

/// line_num colours the numbers of the block's lines, on its own: the chunk can be off.
#[tokio::test(flavor = "multi_thread")]
async fn line_num_colours_the_numbers_of_the_block() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (mut app, _plugins) = app(
        "{ chunk = { enable = false }, line_num = { enable = true } }",
        &rust,
    )
    .await?;
    send_keys(&mut app, "4gg").await?;
    assert!(overlays(&app).is_empty(), "no chunk");
    let rows = app.rendered_rows();
    let text = text_column(&app);
    // the fg of the last digit of the number of each row
    let number = |row: usize| -> Color {
        let digits = rows[row][..]
            .chars()
            .take(text as usize)
            .enumerate()
            .filter(|(_, c)| c.is_ascii_digit())
            .map(|(col, _)| col)
            .last()
            .expect("a line number");
        app.rendered_fg(digits as u16, row as u16)
    };
    let coloured: Vec<bool> = (0..10).map(|row| number(row) == VIOLET).collect();
    assert_eq!(
        coloured,
        [false, false, true, true, true, true, true, true, false, false],
        "lines 3 to 8:\n{}",
        rows[..10].join("\n")
    );
    Ok(())
}

/// A block with a syntax error is drawn in the error colour (unless `error_sign = false`).
#[tokio::test(flavor = "multi_thread")]
async fn a_block_with_an_error_is_red() -> anyhow::Result<()> {
    let text = "fn main() {\n    if ready {\n        let = ;\n    }\n}\n";
    let rust = file(text, ".rs")?;
    for (config, expected) in [
        ("{ chunk = { delay = 0 } }", RED),
        ("{ chunk = { delay = 0, error_sign = false } }", VIOLET),
    ] {
        let (mut app, _plugins) = app(config, &rust).await?;
        send_keys(&mut app, "3gg").await?;
        let rows = text_rows(&app);
        assert_eq!(
            rows[1],
            "╭───if ready {",
            "{config}:\n{}",
            rows[..5].join("\n")
        );
        let corner: Color = app.rendered_fg(text_column(&app), 1);
        assert_eq!(corner, expected, "{config}");
    }
    Ok(())
}

/// Another block is drawn once the cursor has rested `delay` ms, animated over `duration`
/// ms: until then the last one stays. An edit draws at once.
#[tokio::test(flavor = "multi_thread")]
async fn another_block_waits_for_the_cursor_to_rest() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    // a long delay: the new block is not drawn yet
    let (mut slow, _slow_plugins) = app("{ chunk = { delay = 60000 } }", &rust).await?;
    let function = overlays(&slow);
    assert!(
        function
            .iter()
            .all(|(_, col, text)| *col == 0 && text == "│")
            && function.len() == 7,
        "the function's chunk, drawn at once at the start: {function:?}"
    );
    send_keys(&mut slow, "4gg").await?;
    assert_eq!(overlays(&slow), function, "still the function's");
    // an edit: drawn at once (the `if` block, a line longer)
    send_keys(&mut slow, "o<esc>").await?;
    let edited = overlays(&slow);
    assert!(
        edited.contains(&(3, 4, "╭".to_string())) && edited.contains(&(9, 4, "╰".to_string())),
        "{edited:?}"
    );

    // a short delay and animation: drawn in the end
    let (mut fast, _fast_plugins) = app("{ chunk = { delay = 20, duration = 60 } }", &rust).await?;
    send_keys(&mut fast, "4gg").await?;
    let mut drawn = Vec::new();
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        send_keys(&mut fast, "").await?;
        drawn = overlays(&fast);
        if drawn.contains(&(8, 7, ">".to_string())) {
            break;
        }
    }
    assert!(drawn.contains(&(3, 4, "╭".to_string())), "{drawn:?}");
    assert!(
        drawn.contains(&(8, 7, ">".to_string())),
        "the arrow, last: {drawn:?}"
    );
    Ok(())
}

/// The commands switch each part, or both, on and off.
#[tokio::test(flavor = "multi_thread")]
async fn commands_switch_the_parts() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (mut app, _plugins) = app(
        "{ chunk = { delay = 0 }, line_num = { enable = true } }",
        &rust,
    )
    .await?;
    send_keys(&mut app, "4gg").await?;
    let shown = |app: &Application| -> (bool, bool) {
        let (view, doc) = helix_view::current_ref!(app.editor);
        (
            !overlays(app).is_empty(),
            doc.plugin_line_numbers.contains_key(&view.id),
        )
    };
    assert_eq!(shown(&app), (true, true));
    send_keys(&mut app, ":pile-toggle chunk<ret>").await?;
    assert_eq!(shown(&app), (false, true));
    send_keys(&mut app, ":pile-disable<ret>").await?;
    assert_eq!(shown(&app), (false, false));
    send_keys(&mut app, ":pile-enable line_num<ret>").await?;
    assert_eq!(shown(&app), (false, true));
    send_keys(&mut app, ":pile-enable<ret>").await?;
    assert_eq!(shown(&app), (true, true));
    Ok(())
}

/// Without a syntax tree (or with `use_treesitter = false`) the block is between the braces
/// around the cursor; `textobject` is a key selecting its lines.
#[tokio::test(flavor = "multi_thread")]
async fn braces_make_a_block_and_the_textobject_selects_it() -> anyhow::Result<()> {
    let plain = file("a {\n    b {\n        c\n    }\n}\n", ".txt")?;
    let (mut app, _plugins) =
        app("{ chunk = { delay = 0, textobject = \"A-c\" } }", &plain).await?;
    send_keys(&mut app, "3gg").await?;
    let rows = text_rows(&app);
    let context = rows[..5].join("\n");
    assert_eq!(rows[1..4], ["╭───b {", "│       c", "╰──>}"], "{context}");
    send_keys(&mut app, "<A-c>").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();
    let lines: (usize, usize) = (
        text.char_to_line(range.from()),
        text.char_to_line(range.to()),
    );
    assert_eq!(
        lines,
        (1, 4),
        "lines 2 to 4, and the line ending of the last"
    );
    Ok(())
}

/// A setting of the wrong type or an unknown one is reported, and its default used.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_settings_are_reported() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (app, _plugins) = app(
        "{ chunk = { delay = \"soon\", colour = 1 }, line_number = {} }",
        &rust,
    )
    .await?;
    let status = app
        .editor
        .get_status()
        .map(|(message, _)| message.to_string())
        .unwrap_or_default();
    for expected in [
        "chunk.delay should be a number",
        "unknown setting chunk.colour",
        "unknown setting line_number",
    ] {
        assert!(status.contains(expected), "{expected}: {status}");
    }
    // the defaults: the function's chunk, drawn at once at the start
    assert!(!overlays(&app).is_empty());
    Ok(())
}

/// A case of a switch is a block of its own: with the cursor in one, its chunk is drawn, not
/// the switch's (Go, indented with tabs).
#[tokio::test(flavor = "multi_thread")]
async fn a_case_of_a_switch_is_its_own_block() -> anyhow::Result<()> {
    let text = "package main\n\nfunc main() {\n\tswitch {\n\tcase a:\n\t\tone()\n\tcase b:\n\t\ttwo()\n\t\tthree()\n\tdefault:\n\t\tfour()\n\t}\n}\n";
    let go = file(text, ".go")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 0 } }", &go).await?;
    // in the second case
    send_keys(&mut app, "8gg").await?;
    let drawn = overlays(&app);
    let rows = text_rows(&app);
    let context = format!("{drawn:?}\n{}", rows[..13].join("\n"));
    assert!(
        drawn.iter().all(|(line, _, _)| (7..=9).contains(line)),
        "{context}"
    );
    assert!(drawn.contains(&(7, 0, "╭".to_string())), "{context}");
    assert!(drawn.contains(&(9, 7, ">".to_string())), "{context}");
    assert_eq!(rows[6], "╭───case b:", "{context}");
    assert_eq!(rows[8], "╰──────>three()", "{context}");

    // in `default:`, its case
    send_keys(&mut app, "11gg").await?;
    let drawn = overlays(&app);
    assert!(drawn.contains(&(10, 0, "╭".to_string())), "{drawn:?}");
    assert!(
        drawn.iter().all(|(line, _, _)| (10..=11).contains(line)),
        "{drawn:?}"
    );

    // on the `switch` line: the switch
    send_keys(&mut app, "4gg").await?;
    let drawn = overlays(&app);
    assert!(drawn.contains(&(12, 0, "╰".to_string())), "{drawn:?}");
    assert!(
        drawn.iter().all(|(line, _, _)| (4..=12).contains(line)),
        "{drawn:?}"
    );
    Ok(())
}
