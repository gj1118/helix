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
    app_with(config, file, |_| {}).await
}

/// [`app`], with `change` made to the editor's settings.
async fn app_with(
    config: &str,
    file: &tempfile::NamedTempFile,
    change: impl FnOnce(&mut Config),
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
    app_config.plugins = Some(plugins_section(plugins.path(), config)?);
    change(&mut app_config);
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(app_config)
        .build()?;
    assert!(app.plugins_enabled());
    app.resize_terminal(80, 24).await;
    send_keys(&mut app, "").await?;
    Ok((app, plugins))
}

/// The `[plugins]` section loading the plugin from `dir` with `config` (a TOML inline table)
/// and the permissions it asks for.
fn plugins_section(dir: &std::path::Path, config: &str) -> anyhow::Result<toml::Value> {
    Ok(toml::from_str(&format!(
        "plugin_dirs = [{dir:?}]\n[[plugins]]\nname = \"pile\"\n\
         permissions = [\"commands\", \"keymaps\", \"edit\"]\nconfig = {config}\n"
    ))?)
}

/// The plugin settings of that section, as `:config-reload` reads them.
fn settings(dir: &std::path::Path, config: &str) -> anyhow::Result<helix_plugin::PluginConfig> {
    let config = Config {
        plugins: Some(plugins_section(dir, config)?),
        ..Default::default()
    };
    Ok(config.plugin_config())
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
        // not in the gradient's colours
        ("{ chunk = { delay = 0, gradient = true } }", RED),
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
            // beside the 7 lines between its first and last, and the rows its first line
            // would be wrapped onto
            && function.len() == 8,
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

/// Every frame of the animation is the whole editor drawn again: there are `fps` of them a
/// second at most (here 3 for the 12 cells of the `if` block; one every 16 ms would be 37).
#[tokio::test(flavor = "multi_thread")]
async fn the_animation_draws_no_more_frames_than_its_fps() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let frames_of = |config: &'static str| {
        let rust = &rust;
        async move {
            let (mut app, _plugins) = app(config, rust).await?;
            let function = overlays(&app);
            let before = app.frames_drawn();
            // (the editor is not idle before the animation is over)
            send_keys(&mut app, "4gg").await?;
            let mut polls = 0;
            while overlays(&app) == function || overlays(&app).len() < 12 {
                polls += 1;
                assert!(polls < 300, "the animation does not end");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                send_keys(&mut app, "").await?;
            }
            let done = overlays(&app);
            assert!(done.contains(&(3, 4, "╭".to_string())), "{done:?}");
            assert!(done.contains(&(8, 7, ">".to_string())), "{done:?}");
            anyhow::Ok(app.frames_drawn() - before)
        }
    };
    // what the keys and the cursor resting draw, without an animation
    let plain = frames_of("{ chunk = { delay = 20, duration = 0 } }").await?;
    let slow = frames_of("{ chunk = { delay = 20, duration = 600, fps = 5 } }").await?;
    let fast = frames_of("{ chunk = { delay = 20, duration = 600, fps = 20 } }").await?;
    // (the first frame also takes the last block away; one more at most: a firing of the
    // timer a moment early)
    assert!(
        (plain + 1..=plain + 3 + 1).contains(&slow),
        "3 frames at 5 a second: {slow} frames, {plain} without an animation"
    );
    assert!(
        (slow..=plain + 12 + 1).contains(&fast),
        "12 frames at 20 a second: {fast} frames, {plain} without an animation"
    );
    Ok(())
}

/// The cells kept above and below the screen (for a scroll) are not animated: a frame that
/// shows nothing new would be the editor drawn again for nothing. They are there from the
/// start, with the first frame of what is on screen.
#[tokio::test(flavor = "multi_thread")]
async fn the_animation_is_of_the_cells_on_screen() -> anyhow::Result<()> {
    let mut text = String::from("fn first() {\n    one();\n}\n\nfn tall() {\n");
    for _ in 0..200 {
        text.push_str("    line();\n");
    }
    text.push_str("}\n");
    let rust = file(&text, ".rs")?;
    // (so slow that only the first frame is drawn while the test looks)
    let (mut app, _plugins) = app("{ chunk = { delay = 20, duration = 600000 } }", &rust).await?;
    let first = overlays(&app);
    assert!(first.iter().all(|(line, _, _)| *line <= 3), "{first:?}");
    send_keys(&mut app, "100gg").await?;
    let mut drawn = Vec::new();
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        send_keys(&mut app, "").await?;
        drawn = overlays(&app);
        if drawn != first {
            break;
        }
    }
    let (top, bottom) = {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let text = doc.text().slice(..);
        let top = text.char_to_line(doc.view_offset(view.id).anchor) + 1;
        (top, top + usize::from(view.inner_area(doc).height))
    };
    assert!(
        top > 60 && bottom < 140,
        "the view is inside the block: {top} to {bottom}"
    );
    let above = drawn.iter().filter(|(line, _, _)| *line < top).count();
    let below = drawn.iter().filter(|(line, _, _)| *line > bottom).count();
    let on_screen = drawn.len() - above - below;
    assert!(
        above >= 10,
        "the lines above the screen, at once: {drawn:?}"
    );
    assert!(
        below >= 10,
        "the lines below the screen, at once: {drawn:?}"
    );
    assert_eq!(
        on_screen, 1,
        "those on screen come with the animation, the first at once: {drawn:?}"
    );
    assert!(drawn.iter().all(|(_, col, text)| *col == 0 && text == "│"));
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
    send_keys(&mut app, ":pile-toggle line_num<ret>:pile-status<ret>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(message, _)| message.to_string())
        .unwrap_or_default();
    assert_eq!(
        status,
        "pile: chunk on, line_num off; the block: lines 3 to 8"
    );
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
        "{ chunk = { delay = \"soon\", colour = 1, style = { thickness = 9, double = \"yes\", line_color = \"yellowish\" } }, \
         line_num = { style = { thickness = 2 } }, line_number = {} }",
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
        "chunk.style.thickness should be 1 to 5",
        "chunk.style.double should be true or false",
        "chunk.style.line_color should be a colour",
        "line_num.style.thickness is for the chunk's line",
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

/// The line goes down the rows a long line of the block is wrapped onto (soft wrap): no gap.
#[tokio::test(flavor = "multi_thread")]
async fn the_chunk_goes_down_wrapped_rows() -> anyhow::Result<()> {
    let text = format!(
        "fn main() {{\n    if ready {{\n        run(\"{}\");\n        done();\n    }}\n}}\n",
        "word ".repeat(16)
    );
    let rust = file(&text, ".rs")?;
    let (mut app, _plugins) = app_with("{ chunk = { delay = 0 } }", &rust, |config| {
        config.editor.soft_wrap.enable = Some(true)
    })
    .await?;
    app.resize_terminal(50, 20).await;
    send_keys(&mut app, "3gg").await?;
    let rows = text_rows(&app);
    let context = rows[..9].join("\n");
    assert_eq!(rows[1], "╭───if ready {", "{context}");
    assert!(rows[2].starts_with("│       run(\"word"), "{context}");
    // the rows `run(...)` is wrapped onto, then `done();`, all beside the line
    let wrapped: Vec<&String> = rows[3..]
        .iter()
        .take_while(|row| !row.contains("done"))
        .collect();
    assert!(!wrapped.is_empty(), "the line is wrapped: {context}");
    for row in &wrapped {
        assert!(row.starts_with("│       "), "{row:?}: {context}");
    }
    let done = 3 + wrapped.len();
    assert!(rows[done].starts_with("│       done();"), "{context}");
    assert_eq!(rows[done + 1], "╰──>}", "{context}");
    Ok(())
}

/// `:config-reload` puts new plugin settings in place (`Application::apply_plugin_settings`,
/// which it calls with the `[plugins]` section read again): the plugins are loaded again with
/// them. The same settings leave them alone; switching plugins off takes a restart.
#[tokio::test(flavor = "multi_thread")]
async fn new_settings_load_the_plugin_again() -> anyhow::Result<()> {
    use helix_plugin::ConfigUpdate;
    let rust = file(RUST, ".rs")?;
    let (mut app, plugins) = app("{ chunk = { delay = 0 } }", &rust).await?;
    send_keys(&mut app, "4gg").await?;
    let shown = |app: &Application| -> (bool, bool) {
        let (view, doc) = helix_view::current_ref!(app.editor);
        (
            !overlays(app).is_empty(),
            doc.plugin_line_numbers.contains_key(&view.id),
        )
    };
    assert_eq!(shown(&app), (true, false));

    // the same settings: nothing to do
    let same = settings(plugins.path(), "{ chunk = { delay = 0 } }")?;
    let update: ConfigUpdate = app.apply_plugin_settings(same)?;
    assert_eq!(update, ConfigUpdate::Unchanged);

    // chunk off, line_num on
    let new = settings(
        plugins.path(),
        "{ chunk = { enable = false }, line_num = { enable = true } }",
    )?;
    let update: ConfigUpdate = app.apply_plugin_settings(new)?;
    assert_eq!(update, ConfigUpdate::Reloaded);
    send_keys(&mut app, "j").await?;
    assert_eq!(shown(&app), (false, true));

    // plugins switched off: at the next start
    let mut off = settings(plugins.path(), "{}")?;
    off.enabled = false;
    let update: ConfigUpdate = app.apply_plugin_settings(off)?;
    assert_eq!(update, ConfigUpdate::NeedsRestart);
    assert_eq!(shown(&app), (false, true), "still loaded");
    Ok(())
}

/// The colours: the user's styles, else the theme's `pile.*` keys, else hlchunk's. A theme
/// change takes its keys at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_colours_come_from_the_settings_or_the_theme() -> anyhow::Result<()> {
    use helix_view::graphics::Modifier;
    let rust = file(RUST, ".rs")?;
    let number_cell = |app: &Application, row: u16| -> (Color, bool) {
        let rows = app.rendered_rows();
        let text = text_column(app) as usize;
        let col = rows[row as usize]
            .chars()
            .take(text)
            .enumerate()
            .filter(|(_, c)| c.is_ascii_digit())
            .map(|(col, _)| col)
            .last()
            .expect("a line number") as u16;
        let cell = app.rendered_cell(col, row);
        (cell.fg, cell.modifier.contains(Modifier::BOLD))
    };

    // no style set: hlchunk's with the test theme, the theme's with simplelight
    let (mut plain, _plain_plugins) = app(
        "{ chunk = { delay = 0 }, line_num = { enable = true } }",
        &rust,
    )
    .await?;
    send_keys(&mut plain, "4gg").await?;
    let number: (Color, bool) = number_cell(&plain, 2);
    assert_eq!(number.0, VIOLET);
    send_keys(&mut plain, ":theme simplelight<ret>").await?;
    let ink = Color::Rgb(0x00, 0x01, 0x00);
    assert_eq!(
        number_cell(&plain, 2),
        (ink, true),
        "simplelight's pile.line_num"
    );
    assert_eq!(
        number_cell(&plain, 9).0,
        Color::Rgb(0x40, 0x41, 0x3e),
        "out of the block"
    );
    let corner: Color = plain.rendered_fg(text_column(&plain) + 4, 2);
    assert_eq!(
        corner,
        Color::Rgb(0x6d, 0x6e, 0x6b),
        "simplelight's pile.chunk"
    );

    // the user's, over the theme's
    let (mut mine, _mine_plugins) = app(
        "{ chunk = { delay = 0, style = { fg = \"#112233\" } }, \
         line_num = { enable = true, style = \"#5f87af\" } }",
        &rust,
    )
    .await?;
    send_keys(&mut mine, ":theme simplelight<ret>4gg").await?;
    assert_eq!(number_cell(&mine, 2).0, Color::Rgb(0x5f, 0x87, 0xaf));
    let corner: Color = mine.rendered_fg(text_column(&mine) + 4, 2);
    assert_eq!(corner, Color::Rgb(0x11, 0x22, 0x33));

    // the numbers' style written like the line's: line_color, and modifiers
    let (mut numbers, _numbers_plugins) = app(
        "{ chunk = { delay = 0 }, \
         line_num = { enable = true, style = { line_color = \"#96adc9\", modifiers = [\"bold\"] } } }",
        &rust,
    )
    .await?;
    send_keys(&mut numbers, ":theme simplelight<ret>4gg").await?;
    let (fg, bold): (Color, bool) = number_cell(&numbers, 2);
    assert_eq!(fg, Color::Rgb(0x96, 0xad, 0xc9));
    assert!(bold);
    let (outside, _): (Color, bool) = number_cell(&numbers, 0);
    assert_ne!(outside, Color::Rgb(0x96, 0xad, 0xc9), "out of the block");
    Ok(())
}

/// `gradient`: the line in the colours of `[editor.gradient-borders]`, from the first line's
/// code (its start colour), down, to the arrow (its end colour), through its middle colour
/// when it has one; `:set` changes them at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_line_can_take_the_gradient_colours() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    // the colours of the overlays: by (line, col)
    let colour = |app: &Application, line: usize, col: usize| -> Option<Color> {
        let (view, doc) = helix_view::current_ref!(app.editor);
        doc.plugin_overlays
            .get(&view.id)?
            .iter()
            .find(|o| o.line + 1 == line && o.col == col && o.rows.on(true))
            .and_then(|o| o.style.fg)
    };
    for (middle, halfway) in [
        ("", Color::Rgb(139, 0, 115)),
        ("#00ff00", Color::Rgb(23, 231, 0)),
    ] {
        let (mut app, _plugins) = app_with(
            "{ chunk = { delay = 0, gradient = true } }",
            &rust,
            |config| {
                config.editor.gradient_borders.start_color = "#ff0000".into();
                config.editor.gradient_borders.middle_color = middle.into();
                config.editor.gradient_borders.end_color = "#0000ff".into();
            },
        )
        .await?;
        send_keys(&mut app, "4gg").await?;
        // the `if` block, lines 3 to 8, the line in column 4: 4 cells along line 3, 4 lines down
        // (5 to 7, and 4), 4 cells along line 8: 12 places
        assert_eq!(
            colour(&app, 3, 7),
            Some(Color::Rgb(255, 0, 0)),
            "{middle:?}: the start"
        );
        assert_eq!(
            colour(&app, 5, 4),
            Some(halfway),
            "{middle:?}: place 5 of 12"
        );
        assert_eq!(
            colour(&app, 8, 7),
            Some(Color::Rgb(0, 0, 255)),
            "{middle:?}: the arrow"
        );
        // as drawn
        let start: Color = app.rendered_fg(text_column(&app) + 7, 2);
        assert_eq!(start, Color::Rgb(255, 0, 0));
        if middle.is_empty() {
            send_keys(&mut app, ":set gradient-borders.start-color #00ff00<ret>").await?;
            assert_eq!(
                colour(&app, 3, 7),
                Some(Color::Rgb(0, 255, 0)),
                "the new start"
            );
            assert_eq!(colour(&app, 5, 4), Some(Color::Rgb(0, 139, 115)));
        }
    }
    Ok(())
}

/// The line's style: `line_color` (or `lineColor`), `thickness`, 1 to 5, a single line from
/// thin to full blocks (thin and rounded, heavy, half blocks, half and full blocks, full
/// blocks), and `double`, a double line whatever the thickness; `chars` over the thickness's;
/// `gradient` over `line_color`, not over the thickness.
#[tokio::test(flavor = "multi_thread")]
async fn the_line_has_a_colour_and_a_thickness() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let yellow = Color::Rgb(0xff, 0xc9, 0x00);
    for (style, top, middle, bottom, colour) in [
        (
            "{ line_color = \"#ffc900\", thickness = 3 }",
            "▗▄▄▄if",
            "▐   ",
            "▝▀▀>}",
            yellow,
        ),
        (
            "{ line_color = \"#ffc900\", double = true }",
            "╔═══if",
            "║   ",
            "╚══>}",
            yellow,
        ),
        // double whatever the thickness; double = false: the thickness's single line
        (
            "{ thickness = 4, double = true }",
            "╔═══if",
            "║   ",
            "╚══>}",
            VIOLET,
        ),
        (
            "{ thickness = 2, double = false }",
            "┏━━━if",
            "┃   ",
            "┗━━>}",
            VIOLET,
        ),
        (
            "{ lineColor = \"#ffc900\", thickness = 2 }",
            "┏━━━if",
            "┃   ",
            "┗━━>}",
            yellow,
        ),
        ("{ thickness = 4 }", "█▄▄▄if", "█   ", "█▀▀>}", VIOLET),
        (
            "{ thickness = 5, fg = \"#ffc900\" }",
            "████if",
            "█   ",
            "███>}",
            yellow,
        ),
        (
            "{ line_color = \"#ffc900\" }",
            "╭───if",
            "│   ",
            "╰──>}",
            yellow,
        ),
    ] {
        let config = format!("{{ chunk = {{ delay = 0, style = {style} }} }}");
        let (mut app, _plugins) = app(&config, &rust).await?;
        send_keys(&mut app, "4gg").await?;
        let rows = text_rows(&app);
        let context = format!("{style}:\n{}", rows[..9].join("\n"));
        // from the line's column (4)
        let from_the_line = |row: usize| -> String { rows[row].chars().skip(4).collect() };
        assert!(from_the_line(2).starts_with(top), "{context}");
        assert!(from_the_line(3).starts_with(middle), "{context}");
        assert!(from_the_line(7).starts_with(bottom), "{context}");
        let corner: Color = app.rendered_fg(text_column(&app) + 4, 2);
        assert_eq!(corner, colour, "{context}");
    }

    // chars over the thickness's
    let (mut chars, _chars_plugins) = app(
        "{ chunk = { delay = 0, style = { thickness = 2 }, chars = { vertical_line = \"┆\" } } }",
        &rust,
    )
    .await?;
    send_keys(&mut chars, "4gg").await?;
    let rows = text_rows(&chars);
    assert_eq!(
        rows[2],
        "    ┏━━━if item.ready {",
        "{}",
        rows[..9].join("\n")
    );
    assert_eq!(rows[3], "    ┆       item.run();");

    // the gradient over line_color, the double line kept
    let (mut gradient, _gradient_plugins) = app_with(
        "{ chunk = { delay = 0, gradient = true, style = { line_color = \"#ffc900\", double = true } } }",
        &rust,
        |config| {
            config.editor.gradient_borders.start_color = "#ff0000".into();
            config.editor.gradient_borders.end_color = "#0000ff".into();
        },
    )
    .await?;
    send_keys(&mut gradient, "4gg").await?;
    let rows = text_rows(&gradient);
    assert_eq!(
        rows[2],
        "    ╔═══if item.ready {",
        "{}",
        rows[..9].join("\n")
    );
    let start: Color = gradient.rendered_fg(text_column(&gradient) + 7, 2);
    assert_eq!(
        start,
        Color::Rgb(255, 0, 0),
        "the gradient's start, not line_color"
    );

    // beside a block with a syntax error: the error colour, style's thickness and double (or
    // its own)
    let broken = file(
        "fn main() {\n    if ready {\n        let = ;\n    }\n}\n",
        ".rs",
    )?;
    for (config, top) in [
        ("{ chunk = { delay = 0, style = { thickness = 3 } } }", "▗▄▄▄if ready {"),
        ("{ chunk = { delay = 0, style = { double = true } } }", "╔═══if ready {"),
        (
            "{ chunk = { delay = 0, style = { thickness = 3 }, error_style = { fg = \"#c21f30\", thickness = 2 } } }",
            "┏━━━if ready {",
        ),
        (
            "{ chunk = { delay = 0, style = { double = true }, error_style = { fg = \"#c21f30\", double = false } } }",
            "╭───if ready {",
        ),
        (
            "{ chunk = { delay = 0, style = { thickness = 2 }, error_style = { fg = \"#c21f30\", double = true } } }",
            "╔═══if ready {",
        ),
    ] {
        let (mut app, _plugins) = app(config, &broken).await?;
        send_keys(&mut app, "3gg").await?;
        let rows = text_rows(&app);
        assert_eq!(rows[1], top, "{config}:\n{}", rows[..5].join("\n"));
        let corner: Color = app.rendered_fg(text_column(&app), 1);
        assert_eq!(corner, RED, "{config}");
    }
    Ok(())
}

/// The status line's message.
fn status(app: &Application) -> String {
    app.editor
        .get_status()
        .map(|(message, _)| message.to_string())
        .unwrap_or_default()
}

/// The braces of a block are found however far apart (the text is read in batches growing
/// from 64 lines to 512), past nested blocks, with wide characters before them, `\r\n` line
/// endings, and the cursor on a brace.
#[tokio::test(flavor = "multi_thread")]
async fn braces_are_found_however_far_apart() -> anyhow::Result<()> {
    let mut lines = vec!["a {".to_string(), "  é ü { inner".to_string()];
    lines.extend(std::iter::repeat_n("    x".to_string(), 700));
    // the inner block closes on line 703, where another opens
    lines.extend(["  } { same line", "    y", "  }", "}", "after"].map(String::from));
    let text = lines.join("\r\n") + "\r\n";
    let plain = file(&text, ".txt")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 0 } }", &plain).await?;
    for (keys, block) in [
        ("400gg", "lines 2 to 703"),
        ("704gg", "lines 703 to 705"),
        // on the closing brace of the outer block, the inner ones skipped going back
        ("706gg", "lines 1 to 706"),
        // on the brace opening the inner block: its block
        ("2ggf{", "lines 2 to 703"),
        ("1gggl", "lines 1 to 706"),
    ] {
        send_keys(&mut app, &format!("{keys}:pile-status<ret>")).await?;
        assert_eq!(
            status(&app),
            format!("pile: chunk on, line_num off; the block: {block}"),
            "{keys}"
        );
    }
    send_keys(&mut app, "707gg:pile-status<ret>").await?;
    assert_eq!(
        status(&app),
        "pile: chunk on, line_num off; no block around the cursor"
    );
    assert!(overlays(&app).is_empty());
    Ok(())
}

/// Edits keep the chunk right: typing after a line's indentation changes none of its cells,
/// typing in the indentation or on a blank line may, and a change of whole lines (replacing
/// them with the same text) moves what is drawn on them, which is drawn again.
#[tokio::test(flavor = "multi_thread")]
async fn the_chunk_stays_right_through_edits() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 0 } }", &rust).await?;
    send_keys(&mut app, "4gg").await?;
    let drawn = overlays(&app);
    assert!(drawn.contains(&(5, 4, "│".to_string())), "{drawn:?}");

    send_keys(&mut app, "Aabc<esc>").await?;
    assert_eq!(overlays(&app), drawn, "typed after the indentation");
    send_keys(&mut app, "ghi  <esc>").await?;
    let indented = overlays(&app);
    assert_eq!(
        indented, drawn,
        "line 4 indented more: still beside the line"
    );

    // lines 4 and 5 replaced by themselves in one change: the overlays on them moved
    send_keys(&mut app, "4ggxxyR").await?;
    let (_, doc) = helix_view::current_ref!(app.editor);
    assert!(
        doc.text().to_string().contains("item.run();abc\n\n"),
        "the same text"
    );
    assert_eq!(overlays(&app), drawn, "drawn again where they were");

    // the blank line 5 typed on (not indented): no line beside it any more
    send_keys(&mut app, "5ggiz<esc>").await?;
    let typed = overlays(&app);
    assert!(!typed.iter().any(|(line, ..)| *line == 5), "{typed:?}");
    assert!(typed.contains(&(6, 4, "│".to_string())), "{typed:?}");
    Ok(())
}

/// In another window (a split) the chunk is drawn at once, even with a long delay; the same
/// cells as in the first window are sent to the new one.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_window_draws_its_chunk_at_once() -> anyhow::Result<()> {
    let rust = file(RUST, ".rs")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 60000 } }", &rust).await?;
    let first = overlays(&app);
    assert!(!first.is_empty(), "drawn at the start");
    let first_view = app.editor.tree.focus;
    send_keys(&mut app, ":vsplit<ret>").await?;
    assert_ne!(app.editor.tree.focus, first_view);
    assert_eq!(overlays(&app), first, "the new window's");
    Ok(())
}

/// A scroll taking the cursor into another block (`C-d`) is a move: the new block waits for
/// the cursor to rest. An edit then draws it at once.
#[tokio::test(flavor = "multi_thread")]
async fn scrolling_into_another_block_waits_like_a_move() -> anyhow::Result<()> {
    let body = "    let x = 1;\n".repeat(30);
    let text = format!("fn a() {{\n{body}}}\nfn b() {{\n{body}}}\n");
    let rust = file(&text, ".rs")?;
    let (mut app, _plugins) = app("{ chunk = { delay = 60000 } }", &rust).await?;
    let a = overlays(&app);
    assert!(
        a.contains(&(2, 0, "│".to_string())),
        "a's, at the start: {a:?}"
    );
    send_keys(&mut app, "<C-d><C-d><C-d><C-d>").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let line = doc.text().char_to_line(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    assert!(line + 1 > 33, "in b: line {}", line + 1);
    assert_eq!(overlays(&app), a, "still a's");
    send_keys(&mut app, "o<esc>").await?;
    let b = overlays(&app);
    assert!(
        b.contains(&(40, 0, "│".to_string())) && !b.contains(&(2, 0, "│".to_string())),
        "{b:?}"
    );
    Ok(())
}
