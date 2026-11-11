//! The command palette (`Space ?`).

use super::*;
use helix_term::application::Application;

async fn send(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent};

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.editor.reset_idle_timer();
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// The text of the screen, without the trailing spaces of its rows.
fn screen(app: &Application) -> Vec<String> {
    app.rendered_rows()
        .iter()
        .map(|row| row.trim_end().to_string())
        .collect()
}

/// Typable commands are listed with their aliases (searching `quickfix` finds `:copen`), and
/// the quickfix panel has a command of its own.
#[tokio::test(flavor = "multi_thread")]
async fn commands_are_found_by_their_aliases() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    app.resize_terminal(140, 30).await;
    send(&mut app, "<space>?quickfix").await?;
    let rows = screen(&app);
    let found = |text: &str| rows.iter().any(|row| row.contains(text));
    let (copen, cclose, focus): (bool, bool, bool) = (
        found(":copen, :quickfix-open"),
        found(":cclose, :quickfix-close"),
        found("quickfix_focus"),
    );
    assert_eq!((copen, cclose, focus), (true, true, true), "{rows:#?}");
    Ok(())
}

/// The selected command shows all of its description, wrapped over as many lines as it
/// takes; the others keep one line, cut at the end (not at the start).
#[tokio::test(flavor = "multi_thread")]
async fn the_selected_description_is_shown_in_full() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    app.resize_terminal(100, 30).await;
    send(&mut app, "<space>?:copen").await?;
    let rows = screen(&app);
    let doc = "Show the quickfix panel: focus it, or show the last list it showed again, or the quicklist.";
    let start = rows
        .iter()
        .position(|row| row.contains(":copen, :quickfix-open"))
        .expect("`:copen` is listed");
    // where the description starts, in its row (in characters: the border is not ASCII)
    let chars = |row: &str| row.chars().collect::<Vec<char>>();
    let first = chars(&rows[start]);
    let column = (0..first.len())
        .find(|&i| {
            first[i..]
                .iter()
                .collect::<String>()
                .starts_with("Show the quickfix")
        })
        .expect("the description starts with its first words");
    // its lines, one below the other in the same column (up to the border), make it whole
    let mut lines = Vec::new();
    for row in &rows[start..] {
        let text: String = chars(row)
            .get(column..)
            .unwrap_or(&[])
            .iter()
            .take_while(|c| **c != '▕')
            .collect();
        let text = text.trim().to_string();
        if text.is_empty() {
            break;
        }
        let done = text.ends_with("quicklist.");
        lines.push(text);
        if done {
            break;
        }
    }
    assert!(lines.len() > 1, "a long description wraps: {rows:#?}");
    let joined: String = lines.join(" ");
    assert_eq!(joined, doc);
    // the line after it is the next command, not a cut description
    assert!(!rows.iter().any(|row| row.contains('…')), "{rows:#?}");
    Ok(())
}

/// `quickfix_focus` runs from the palette like `:copen`: with an empty quicklist, it says so.
#[tokio::test(flavor = "multi_thread")]
async fn the_quickfix_panel_is_focused_from_the_palette() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    send(&mut app, "<space>?quickfix_focus<ret>").await?;
    let status: Option<String> = app.editor.get_status().map(|(s, _)| s.to_string());
    assert_eq!(
        status.as_deref(),
        Some("The quicklist is empty, add locations with Alt-q or Ctrl-q in a picker")
    );
    Ok(())
}
