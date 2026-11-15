//! Very long lines: drawn and moved in from where they are looked at, not walked from their
//! start (the plain text skipped without soft wrap, blocks of a long line with it). What is
//! drawn must be the text, where the cursor is.
use super::*;

use helix_core::doc_formatter::SOFT_WRAP_BLOCK;
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

/// The char under the cursor on the screen, and the one at the cursor in the document (a
/// space at the end of the file).
fn cursor_cell_and_char(app: &Application) -> (String, char) {
    let (position, _) = app.editor.cursor();
    let position = position.expect("a cursor on the screen");
    let shown = app
        .rendered_cell(position.col as u16, position.row as u16)
        .symbol
        .to_string();
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let at = if cursor < text.len_chars() {
        text.char(cursor)
    } else {
        ' '
    };
    (shown, at)
}

fn app_with(text: &str, soft_wrap: bool) -> anyhow::Result<(Application, tempfile::NamedTempFile)> {
    let file = helpers::temp_file_with_contents(text)?;
    let mut config = helpers::test_config();
    config.editor.soft_wrap.enable = Some(soft_wrap);
    let app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    Ok((app, file))
}

/// Without soft wrap: a long line scrolled far right, past a tab and wide and combining chars:
/// its text is drawn at the right columns, the cursor on its char.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_line_scrolled_right_is_drawn_where_it_is() -> anyhow::Result<()> {
    let line = format!(
        "{}\tmid é 名 {}MARK{}\n",
        "a".repeat(4000),
        "b".repeat(3000),
        "c".repeat(3000)
    );
    let (mut app, _file) = app_with(&format!("{line}second\n"), false)?;

    // the match selected, the cursor on its last char
    send_keys(&mut app, "/MARK<ret>").await?;
    let (shown, at): (String, char) = cursor_cell_and_char(&app);
    assert_eq!((shown.as_str(), at), ("K", 'K'));
    let rows = app.rendered_rows();
    assert!(
        rows.iter().any(|row| row.contains("bbbMARKccc")),
        "{rows:#?}"
    );

    // moving on the line, and to its end and back
    for keys in ["5l", "20h", "gl", "gh", "/é<ret>", "/名<ret>"] {
        send_keys(&mut app, keys).await?;
        let (shown, at): (String, char) = cursor_cell_and_char(&app);
        assert_eq!(shown, at.to_string(), "after {keys}");
    }
    Ok(())
}

/// With soft wrap: a line of words several blocks long: deep in it, across the start of its
/// blocks up and down, the cursor is on its char.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_soft_wrapped_line_is_drawn_where_it_is() -> anyhow::Result<()> {
    let words: Vec<String> = (0..SOFT_WRAP_BLOCK)
        .map(|i| {
            if i == SOFT_WRAP_BLOCK / 2 {
                "MARK".to_string()
            } else {
                format!("w{}", i % 97)
            }
        })
        .collect();
    let line = words.join(" ");
    assert!(line.len() > SOFT_WRAP_BLOCK * 3);
    let (mut app, _file) = app_with(&format!("{line}\nsecond\n"), true)?;

    send_keys(&mut app, "/MARK<ret>").await?;
    let (shown, at): (String, char) = cursor_cell_and_char(&app);
    assert_eq!((shown.as_str(), at), ("K", 'K'));
    let rows = app.rendered_rows();
    assert!(rows.iter().any(|row| row.contains("MARK")), "{rows:#?}");

    // up and down over the visual lines, past blocks' starts, to the end and the start
    for keys in ["300k", "700j", "1000k", "3l", "gl", "gg", "G"] {
        send_keys(&mut app, keys).await?;
        let (shown, at): (String, char) = cursor_cell_and_char(&app);
        if !at.is_whitespace() {
            assert_eq!(shown, at.to_string(), "after {keys}");
        }
    }
    Ok(())
}
