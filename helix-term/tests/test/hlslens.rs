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

/// While the search is typed the lenses follow it (`incsearch`); Esc brings back those of the
/// last search, or none.
#[tokio::test(flavor = "multi_thread")]
async fn lenses_follow_the_search_as_it_is_typed() -> anyhow::Result<()> {
    let (mut app, _plugins, _file) = app("{}").await?;
    // nothing searched yet: the lenses come and go with the prompt
    send_keys(&mut app, "/fo").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );
    send_keys(&mut app, "o g").await?;
    assert_eq!(lenses(&app), [lens(2, "[1/1]", NEAR)]);
    // an invalid pattern while typing: no lenses, no error
    send_keys(&mut app, "<backspace><backspace><backspace><backspace>(").await?;
    assert_eq!(lenses(&app), []);
    send_keys(&mut app, "<esc>").await?;
    assert_eq!(lenses(&app), []);
    assert_eq!(highlights(&app), []);

    // after a search: Esc shows its lenses again
    send_keys(&mut app, "/foo<ret>/gam").await?;
    assert_eq!(lenses(&app), [lens(2, "[1/1]", NEAR)]);
    send_keys(&mut app, "<esc>").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2]", OTHER)]
    );
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// `per_line = "all"`: a lens for each match of a line; `"count"`: one, and how many more.
#[tokio::test(flavor = "multi_thread")]
async fn several_matches_on_a_line() -> anyhow::Result<()> {
    let (mut app, _plugins, _file) = app("{ per_line = \"all\" }").await?;
    send_keys(&mut app, "/foo<ret>n").await?;
    assert_eq!(
        lenses(&app),
        [
            lens(0, "[1]", OTHER),
            lens(2, "[2/4]", NEAR),
            lens(2, "[3]", OTHER)
        ]
    );
    assert!(
        app.rendered_rows()
            .iter()
            .any(|row| row.contains("foo gamma foo [2/4][3]")),
        "{:#?}",
        app.rendered_rows()
    );
    send_keys(&mut app, ":q!<ret>").await?;

    let (mut app, _plugins, _file) = self::app("{ per_line = \"count\" }").await?;
    send_keys(&mut app, "/foo<ret>").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1/4]", NEAR), lens(2, "[2] +1", OTHER)]
    );
    send_keys(&mut app, "nn").await?;
    assert_eq!(
        lenses(&app),
        [lens(0, "[1]", OTHER), lens(2, "[3/4] +1", NEAR)]
    );
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// A match on a line longer than the window: its lens is at the right edge of the window, on
/// the match's row (not dropped below it, not out of view).
#[tokio::test(flavor = "multi_thread")]
async fn lenses_of_long_lines_stay_in_view() -> anyhow::Result<()> {
    let (mut app, _plugins, _file) = app("{}").await?;
    send_keys(
        &mut app,
        &format!("ggA {}<esc>gg/foo<ret>", "y".repeat(100)),
    )
    .await?;
    let rows = app.rendered_rows();
    assert!(rows[0].ends_with("[1/4]"), "{rows:#?}");
    assert_eq!(rows[0].chars().count(), 80, "{rows:#?}");
    assert!(rows[1].contains("line 1"), "{rows:#?}");
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}
