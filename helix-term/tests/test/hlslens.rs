//! The `hlslens` plugin of `contrib/plugins`: search lenses, like nvim-hlslens.
use super::*;

use helix_term::application::Application;

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

/// `foo` on lines 0 (once), 2 (twice) and 20: four matches, the last one out of view.
fn text() -> String {
    let mut lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
    lines[0] = "alpha foo".into();
    lines[2] = "foo gamma foo".into();
    lines[20] = "far foo".into();
    lines.join("\n") + "\n"
}

/// An app on 80x12 with the plugin loaded from `contrib/plugins/hlslens`, with `config`
/// (a TOML inline table) and the `commands` permission.
async fn app(
    config: &str,
) -> anyhow::Result<(Application, tempfile::TempDir, tempfile::NamedTempFile)> {
    let plugins = tempfile::tempdir()?;
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/../contrib/plugins/hlslens");
    let dir = plugins.path().join("hlslens");
    std::fs::create_dir(&dir)?;
    for name in ["init.lua", "plugin.toml"] {
        std::fs::copy(format!("{source}/{name}"), dir.join(name))?;
    }
    let mut app_config = helpers::test_config();
    app_config.editor.notifications.enable = false;
    app_config.plugins = Some(toml::from_str(&format!(
        "plugin_dirs = [{:?}]\n[[plugins]]\nname = \"hlslens\"\npermissions = [\"commands\"]\nconfig = {config}\n",
        plugins.path()
    ))?);
    let file = helpers::temp_file_with_contents(text())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(app_config)
        .build()?;
    assert!(app.plugins_enabled());
    app.resize_terminal(80, 12).await;
    Ok((app, plugins, file))
}

/// The lenses of the focused window: `(line, text, style)`, by position.
fn lenses(app: &Application) -> Vec<(usize, String, String)> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    doc.plugin_annotations
        .get(&view.id)
        .map(|annots| {
            annots
                .iter()
                .filter(|a| a.plugin == "hlslens")
                .map(|a| {
                    (
                        text.char_to_line(a.char_idx),
                        a.text.clone(),
                        a.style.clone().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The highlighted matches: `(from, to, style)`.
fn highlights(app: &Application) -> Vec<(usize, usize, String)> {
    let doc = helix_view::doc!(app.editor);
    let mut found: Vec<(usize, usize, String)> = doc
        .plugin_highlights
        .iter()
        .filter(|h| h.plugin == "hlslens")
        .map(|h| (h.from, h.to, h.scope.clone()))
        .collect();
    found.sort();
    found
}

fn lens(line: usize, text: &str, style: &str) -> (usize, String, String) {
    (line, text.to_string(), style.to_string())
}

const NEAR: &str = "ui.cursor.match";
const OTHER: &str = "ui.virtual";

#[tokio::test(flavor = "multi_thread")]
async fn lenses_show_the_index_of_the_matches_in_view() -> anyhow::Result<()> {
    let (mut app, _plugins, _file) = app("{}").await?;
    // the char indices of the matches
    let line2 = "alpha foo\nline 1\n".chars().count();
    let first = 6;
    let (second, third) = (line2, line2 + 10);

    // `/foo`: the first match is the nearest; the line 20 one is out of view
    send_keys(&mut app, "/foo<ret>").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );
    assert_eq!(
        highlights(&app),
        [
            (first, first + 3, NEAR.to_string()),
            (second, second + 3, "ui.highlight".to_string()),
            (third, third + 3, "ui.highlight".to_string()),
        ]
    );
    // drawn after the end of its line, one column away
    let rows = app.rendered_rows();
    assert!(
        rows.iter().any(|row| row.contains("alpha foo [1/4]")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("foo gamma foo [2]")),
        "{rows:#?}"
    );

    // `n`: the nearest moves; one lens per line, the nearest match's
    send_keys(&mut app, "n").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1]", OTHER), lens(2, "[2/4]", NEAR)]
    );
    send_keys(&mut app, "n").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1]", OTHER), lens(2, "[3/4]", NEAR)]
    );
    assert!(highlights(&app).contains(&(third, third + 3, NEAR.to_string())));

    // to line 20: the view scrolls, the others are out of view
    send_keys(&mut app, "n").await?;
    assert_eq!(lenses(&app), [lens(20, "[4/4]", NEAR)]);

    // the cursor moves off the match: the nearest one follows it
    send_keys(&mut app, "gg").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );

    // `:hlslens-stop`, like `:nohlsearch`; the next `n` shows them again
    send_keys(&mut app, ":hlslens-stop<ret>").await?;
    assert_eq!(lenses(&app), []);
    assert_eq!(highlights(&app), []);
    send_keys(&mut app, "n").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );

    // a search left with Esc changes nothing
    send_keys(&mut app, "/line<esc>").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );

    // an edit: the matches are counted again
    send_keys(&mut app, "ggOfoo<esc>").await?;
    assert_eq!(
        lenses(&app),
        [
            lens(0, "[1/5]", NEAR),
            lens(1, "[2]", OTHER),
            lens(3, "[3]", OTHER)
        ]
    );

    // another search: `line 1` and `line 10` to `line 19`, only the first one in view
    send_keys(&mut app, "/line 1<ret>").await?;
    assert_eq!(lenses(&app), [lens(2, "[1/11]", NEAR)]);

    // turned off and on
    send_keys(&mut app, ":hlslens-toggle<ret>").await?;
    assert_eq!(lenses(&app), []);
    send_keys(&mut app, "n").await?;
    assert_eq!(lenses(&app), []);
    send_keys(&mut app, ":hlslens-toggle<ret>").await?;
    assert!(!lenses(&app).is_empty());

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// `nearest_only`: one lens; `calm_down`: they go once the cursor leaves the matches.
#[tokio::test(flavor = "multi_thread")]
async fn nearest_only_and_calm_down() -> anyhow::Result<()> {
    let (mut app, _plugins, _file) =
        app("{ nearest_only = true, calm_down = true, near_lens = \"{index} of {total}\" }")
            .await?;
    send_keys(&mut app, "/foo<ret>").await?;
    assert_eq!(lenses(&app), [lens(0, "1 of 4", NEAR)]);
    // the other matches are still highlighted
    assert_eq!(highlights(&app).len(), 3);
    send_keys(&mut app, "n").await?;
    assert_eq!(lenses(&app), [lens(2, "2 of 4", NEAR)]);
    // off the match
    send_keys(&mut app, "j").await?;
    assert_eq!(lenses(&app), []);
    assert_eq!(highlights(&app), []);
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}
