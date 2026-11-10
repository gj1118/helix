use super::*;

use helix_term::application::Application;

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

/// The test config with `set-title` on and the given `title-format` (the default one for
/// `None`).
fn config(format: Option<&str>) -> Config {
    let mut config = helpers::test_config();
    config.editor.set_title = true;
    if let Some(format) = format {
        config.editor.title_format = format.to_string();
    }
    config
}

/// The terminal's title, and the titles saved on its title stack.
fn title(app: &Application) -> (String, Vec<String>) {
    let (title, stack) = app.terminal_title();
    (title.to_string(), stack.to_vec())
}

fn buffer_name(app: &Application) -> String {
    helix_view::doc!(app.editor).display_name().into_owned()
}

#[test]
fn the_title_is_off_by_default() {
    let config = helix_view::editor::Config::default();
    assert!(!config.set_title);
    assert_eq!(
        config.title_format,
        "hx %{buffer_name}:%{cursor_line}:%{cursor_column}"
    );
}

/// Off (the default), Helix leaves the terminal's title alone.
#[tokio::test(flavor = "multi_thread")]
async fn without_set_title_the_terminal_keeps_its_title() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\ntwo\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    send_keys(&mut app, "jl").await?;
    assert_eq!(title(&app), ("terminal".to_string(), Vec::new()));
    assert!(app.titles_set().is_empty());
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// On, the title follows the buffer and the cursor; it is only sent when it changes, and the
/// terminal's own title is saved on its title stack.
#[tokio::test(flavor = "multi_thread")]
async fn the_title_follows_the_buffer_and_the_cursor() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\ntwo\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config(None))
        .build()?;
    send_keys(&mut app, "<esc>").await?;
    let name = buffer_name(&app);
    assert_eq!(
        title(&app),
        (format!("hx {name}:1:1"), vec!["terminal".to_string()])
    );

    send_keys(&mut app, "jl").await?;
    assert_eq!(title(&app).0, format!("hx {name}:2:2"));
    let sent = app.titles_set().len();
    // nothing changes: nothing is sent
    send_keys(&mut app, "<esc><esc>").await?;
    assert_eq!(app.titles_set().len(), sent);
    // (and every title sent differs from the one before)
    assert!(app.titles_set().windows(2).all(|pair| pair[0] != pair[1]));

    // another buffer
    send_keys(&mut app, ":new<ret>").await?;
    assert_eq!(title(&app).0, "hx [scratch]:1:1");
    assert_eq!(title(&app).1, ["terminal"]);

    send_keys(&mut app, ":q!<ret>:q!<ret>").await?;
    Ok(())
}

/// The terminal gets its own title back when Helix gives it back (exit, `Ctrl-z`) and when
/// `set-title` is turned off; resumed or turned on again, Helix sets its title again.
#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_gets_its_title_back() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config(Some("editing %{buffer_name}")))
        .build()?;
    send_keys(&mut app, "<esc>").await?;
    let editing = format!("editing {}", buffer_name(&app));
    let set = (editing.clone(), vec!["terminal".to_string()]);
    let own = ("terminal".to_string(), Vec::new());
    assert_eq!(title(&app), set);

    // suspended, then resumed
    app.suspend_and_resume(false).await?;
    assert_eq!(title(&app), own);
    app.suspend_and_resume(true).await?;
    assert_eq!(title(&app), set);

    // turned off and on again
    send_keys(&mut app, ":set set-title false<ret>").await?;
    assert_eq!(title(&app), own);
    send_keys(&mut app, ":set set-title true<ret>").await?;
    assert_eq!(title(&app), set);

    // on exit
    app.suspend_and_resume(false).await?;
    assert_eq!(title(&app), own);
    Ok(())
}

/// A shell command in the format is not run (the title would run it for every frame), and
/// control characters (an escape sequence in a file name, say) are left out of the title.
#[tokio::test(flavor = "multi_thread")]
async fn shell_commands_are_not_run_and_control_characters_are_left_out() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("ran");
    let format = format!("%sh{{touch {}}}", marker.display());
    let mut app = helpers::AppBuilder::new()
        .with_config(config(Some(&format)))
        .build()?;
    send_keys(&mut app, "jk").await?;
    assert_eq!(title(&app).0, "hx");
    assert!(!marker.exists());
    send_keys(&mut app, ":q!<ret>").await?;

    // ESC ] 2 ; ... BEL would set another title
    let mut app = helpers::AppBuilder::new()
        .with_config(config(Some("a%u{1b}]2;evil%u{7}b %{cursor_line}")))
        .build()?;
    send_keys(&mut app, "<esc>").await?;
    let (shown, stack) = title(&app);
    assert_eq!(shown, "a]2;evilb 1");
    assert_eq!(stack, ["terminal"]);
    assert!(app
        .titles_set()
        .iter()
        .all(|title| !title.chars().any(char::is_control)));
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}
