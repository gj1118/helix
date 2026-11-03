use super::*;

use helix_term::application::Application;

/// The tests never load plugins: the default plugin directories are the user's own, and
/// every test would otherwise run with (and load again) whatever plugins are installed.
#[tokio::test(flavor = "multi_thread")]
async fn tests_do_not_load_the_users_plugins() -> anyhow::Result<()> {
    let app = helpers::AppBuilder::new().build()?;
    assert!(!app.plugins_enabled());
    Ok(())
}

/// Sends `keys` to the application and runs its event loop until it is idle.
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

/// What the probe plugin recorded in register `z`.
fn recorded(app: &Application) -> Vec<String> {
    app.editor
        .registers
        .read('z', &app.editor)
        .and_then(|mut values| values.next().map(|value| value.into_owned()))
        .map(|events| events.split(',').map(String::from).collect())
        .unwrap_or_default()
}

/// A test's own `[plugins]` table loads plugins (from the given directory only), hands them
/// their configuration and fires the editor events.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_get_their_config_and_the_editor_events() -> anyhow::Result<()> {
    let plugins = tempfile::tempdir()?;
    let probe = plugins.path().join("probe");
    std::fs::create_dir(&probe)?;
    std::fs::write(
        probe.join("init.lua"),
        r#"
local seen = {}
local function record(name)
    table.insert(seen, name)
    helix.editor.set_register("z", { table.concat(seen, ",") })
end
for _, name in ipairs({ "ready", "buffer_changed", "buffer_pre_save", "buffer_post_save", "buffer_close" }) do
    helix.on(name, function() record(name) end)
end
helix.on("ready", function()
    record("greeting=" .. tostring((helix.get_config() or {}).greeting))
end)
-- runs before the text is written: the edit is saved too
helix.on("buffer_pre_save", function()
    helix.buffer.get_current():insert(1, 0, "PRE ")
end)
"#,
    )?;

    let mut config = helpers::test_config();
    config.plugins = Some(toml::from_str(&format!(
        r#"
        plugin_dirs = [{:?}]
        [[plugins]]
        name = "probe"
        config = {{ greeting = "hi" }}
        "#,
        plugins.path()
    ))?);

    let mut file = helpers::temp_file_with_contents("abc\n")?;
    let other = helpers::temp_file_with_contents("other\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    assert!(app.plugins_enabled());
    assert_eq!(recorded(&app), ["ready", "greeting=hi"]);

    // A macro run 20 times edits the buffer 20 times before the next frame: one event.
    send_keys(&mut app, "Qix<esc>Q").await?;
    let changes = |app: &Application| {
        recorded(app)
            .iter()
            .filter(|event| *event == "buffer_changed")
            .count()
    };
    let before = changes(&app);
    assert!(before >= 1);
    send_keys(&mut app, "20q").await?;
    // (the replay ends with a redraw request: draw a frame for sure)
    send_keys(&mut app, "<esc>").await?;
    assert_eq!(changes(&app), before + 1);

    send_keys(&mut app, ":w<ret>").await?;
    helpers::assert_file_has_content(
        &mut file,
        &LineFeedHandling::Native.apply(&format!("PRE {}abc\n", "x".repeat(21))),
    )?;
    let events = recorded(&app);
    let pre = events.iter().position(|e| e == "buffer_pre_save");
    let post = events.iter().position(|e| e == "buffer_post_save");
    assert!(pre.is_some() && pre < post, "{events:?}");

    send_keys(
        &mut app,
        &format!(":o {}<ret>:bc!<ret>", other.path().to_string_lossy()),
    )
    .await?;
    assert_eq!(
        recorded(&app).last().map(String::as_str),
        Some("buffer_close")
    );

    send_keys(&mut app, ":qa!<ret>").await?;
    Ok(())
}
