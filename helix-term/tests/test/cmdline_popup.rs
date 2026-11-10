//! The noice.nvim-like command line popup (`[editor.cmdline] style = "popup"`) and the
//! notifications drawn with it.
#![cfg(not(windows))]

use super::*;
use helix_term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;

fn config(toml: &str) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(toml).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// The command line as a popup, without the notification of the opened file.
fn popup_config() -> Config {
    config("[editor.cmdline]\nstyle = \"popup\"\n[editor.notifications]\nenable = false\n")
}

/// Sends `keys` to the application and runs it until it is idle.
async fn send_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    let mut stream = UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// The screen position of the cursor, as the terminal gets it.
fn cursor(app: &Application) -> (u16, u16) {
    let area = app.compositor.size();
    let (position, _) = app.compositor.cursor(area, &app.editor);
    let position = position.expect("a cursor");
    (position.col as u16, position.row as u16)
}

/// Typed past the width of the popup, the input scrolls: the cursor stays right after the
/// last character typed (it was a column further right, past what was drawn), and moving to
/// the start of the line brings it back to the first one.
#[tokio::test(flavor = "multi_thread")]
async fn the_cursor_follows_the_input_scrolled_in_the_popup() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(popup_config())
        .build()?;
    app.resize_terminal(50, 20).await;

    // short: right after the text
    send_keys(&mut app, ":abc").await?;
    let (x, y) = cursor(&app);
    assert_eq!(app.rendered_cell(x - 1, y).symbol.as_str(), "c");
    assert_eq!(app.rendered_cell(x, y).symbol.as_str(), " ");

    // long: scrolled, with an ellipsis at the start
    let tail = "0123456789".repeat(6) + "XYZ";
    send_keys(&mut app, &tail).await?;
    let (x, y) = cursor(&app);
    assert_eq!(app.rendered_cell(x - 1, y).symbol.as_str(), "Z");
    assert_eq!(app.rendered_cell(x, y).symbol.as_str(), " ");
    let row = &app.rendered_rows()[y as usize];
    assert!(row.contains('…'), "{row:?}");
    assert!(row.contains("XYZ"), "{row:?}");

    // at the start: on the first character
    send_keys(&mut app, "<home>").await?;
    let (x, y) = cursor(&app);
    assert_eq!(app.rendered_cell(x, y).symbol.as_str(), "a");

    send_keys(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}

/// The documentation of the command typed is shown below the popup, like above the bottom
/// command line.
#[tokio::test(flavor = "multi_thread")]
async fn the_popup_shows_the_documentation_of_the_command() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(popup_config())
        .build()?;
    app.resize_terminal(100, 30).await;

    send_keys(&mut app, ":write-quit-all ").await?;
    let rows = app.rendered_rows();
    let popup_row = rows
        .iter()
        .position(|row| row.contains("write-quit-all"))
        .expect("the popup");
    let doc_row = rows
        .iter()
        .position(|row| row.contains("Write changes from all buffers"))
        .unwrap_or_else(|| panic!("no documentation: {rows:#?}"));
    assert!(doc_row > popup_row);
    let aliases = rows.iter().any(|row| row.contains("Aliases: wqa, xa"));
    assert!(aliases, "{rows:#?}");

    // nothing for what is not a command
    send_keys(&mut app, "<C-u>no-such-command").await?;
    let rows = app.rendered_rows();
    assert!(!rows.iter().any(|row| row.contains("Write changes")));

    send_keys(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}

/// With nothing typed, the popup suggests the last entry of the prompt's history (in the
/// inactive text's color), as the bottom command line does.
#[tokio::test(flavor = "multi_thread")]
async fn the_popup_suggests_the_last_entry_of_the_history() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one needle two\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(popup_config())
        .build()?;
    app.resize_terminal(80, 20).await;

    send_keys(&mut app, "/needle<ret>").await?;
    send_keys(&mut app, "/").await?;
    let rows = app.rendered_rows();
    let y = rows
        .iter()
        .position(|row| row.contains("needle") && !row.contains("one needle"))
        .unwrap_or_else(|| panic!("no suggestion: {rows:#?}"));
    let x = rows[y].chars().take_while(|c| *c != 'n').count() as u16;
    let inactive = app.editor.theme.get("ui.text.inactive");
    assert_eq!(Some(app.rendered_fg(x, y as u16)), inactive.fg);

    send_keys(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}

/// The search popup highlights its regex, as the bottom command line does.
#[tokio::test(flavor = "multi_thread")]
async fn the_search_popup_highlights_the_regex() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(popup_config())
        .build()?;
    app.resize_terminal(80, 20).await;

    send_keys(&mut app, "/ab[0-9]+").await?;
    let rows = app.rendered_rows();
    let y = rows
        .iter()
        .position(|row| row.contains("ab[0-9]+"))
        .expect("the input") as u16;
    let x = rows[y as usize].find("ab[0-9]+").unwrap() as u16;
    // (columns are characters on this row of ASCII after the icon)
    let x = rows[y as usize][..x as usize].chars().count() as u16;
    // the letters are plain text, some of the rest is not (the digits of the class, ...)
    let fgs: Vec<_> = (x..x + 8).map(|x| app.rendered_fg(x, y)).collect();
    let plain = app.rendered_fg(x, y);
    assert_eq!(app.rendered_fg(x + 1, y), plain);
    assert!(
        fgs.iter().any(|fg| *fg != plain),
        "not highlighted: {fgs:?} {:?}",
        rows[y as usize]
    );

    send_keys(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}

/// A long notification shows its first line whole (it was cut by the width of the icon), and
/// a message longer than the card ends in an ellipsis.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_notification_is_shown_whole_or_ends_in_an_ellipsis() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            "[editor.notifications]\nmax-width = 30\nmax-height = 6\nshadow.enable = false\n",
        ))
        .build()?;
    app.resize_terminal(80, 20).await;

    app.editor
        .notify_error("the quick brown fox jumps over the lazy dog again and again and again");
    send_keys(&mut app, "<esc>").await?;
    let rows = app.rendered_rows();
    let first = rows
        .iter()
        .find(|row| row.contains("the quick brown fox"))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    // (the cell hidden by the wide emoji keeps what the test terminal had there)
    assert!(first.contains('❌'), "{first:?}");
    let text: String = rows.join("\n");
    assert!(text.contains("jumps over the lazy dog"), "{rows:#?}");
    assert!(text.contains('…'), "{rows:#?}");

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// With `enable-history` off, `:notifications-history` says so instead of showing it.
#[tokio::test(flavor = "multi_thread")]
async fn the_history_can_be_turned_off() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config("[editor.notifications]\nenable-history = false\n"))
        .build()?;
    app.editor.notify_info("something happened");
    send_keys(&mut app, ":notifications-history<ret>").await?;
    let (status, _) = app.editor.get_status().expect("a status");
    assert!(status.contains("history is off"), "{status:?}");
    assert!(!app
        .rendered_rows()
        .iter()
        .any(|row| row.contains("Notification History")));
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}
