use super::*;
use helix_term::application::Application;
use termina::event::{MouseButton, MouseEventKind};
use tokio_stream::wrappers::UnboundedReceiverStream;

async fn mouse(
    app: &mut Application,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Ok(termina::Event::Mouse(termina::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: termina::event::Modifiers::empty(),
    })))?;
    app.event_loop_until_idle(&mut UnboundedReceiverStream::new(rx))
        .await;
    Ok(())
}

async fn click(app: &mut Application, column: u16, row: u16) -> anyhow::Result<()> {
    mouse(app, MouseEventKind::Down(MouseButton::Left), column, row).await?;
    mouse(app, MouseEventKind::Up(MouseButton::Left), column, row).await
}

fn status(app: &Application) -> String {
    app.editor
        .get_status()
        .map(|(s, _)| s.to_string())
        .unwrap_or_default()
}

/// The text of the tests: `epsilon` at columns 14 to 20 of row 1, after the line numbers.
async fn app() -> anyhow::Result<(Application, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.txt");
    std::fs::write(
        &path,
        "alpha beta gamma\ndelta epsilon zeta\neta theta iota\n",
    )?;
    let config = helix_term::config::Config::load(
        Ok(&"[editor]\nmouse = true\n".to_owned()),
        Err(helix_term::config::ConfigLoadError::default()),
    )
    .unwrap();
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config)
        .build()?;
    app.resize_terminal(60, 12).await;
    send_keys(&mut app, "").await?;
    app.editor.notifications.dismiss_all();
    assert!(app.rendered_rows()[1].contains("delta epsilon zeta"));
    Ok((app, dir))
}

fn selected(app: &Application) -> String {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.selection(view.id)
        .primary()
        .fragment(doc.text().slice(..))
        .to_string()
}

fn yanked(app: &Application) -> bool {
    status(app).contains("yanked")
}

/// A click that moves the pointer a cell on the way (a trackpad) is a click: it puts the
/// cursor where the button went down, selects nothing more and yanks nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_click_that_moves_a_cell_does_not_yank() -> anyhow::Result<()> {
    let (mut app, _dir) = app().await?;
    for (drag_column, drag_row) in [(15, 1), (13, 1), (14, 1)] {
        send_keys(&mut app, "gg").await?;
        app.editor.clear_status();
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 14, 1).await?;
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            drag_column,
            drag_row,
        )
        .await?;
        mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            drag_column,
            drag_row,
        )
        .await?;
        assert_eq!(selected(&app), "e", "drag to {drag_column}");
        assert!(!yanked(&app), "{}", status(&app));
    }
    Ok(())
}

/// A drag selects from where the button went down, and the release yanks the selection,
/// as before: across the line, and down a row (one row is enough).
#[tokio::test(flavor = "multi_thread")]
async fn a_drag_selects_and_yanks() -> anyhow::Result<()> {
    let (mut app, _dir) = app().await?;
    app.editor.clear_status();
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 14, 1).await?;
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 15, 1).await?;
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 20, 1).await?;
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 20, 1).await?;
    assert_eq!(selected(&app), "epsilon");
    assert!(yanked(&app), "{}", status(&app));

    app.editor.clear_status();
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 14, 1).await?;
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 14, 2).await?;
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 14, 2).await?;
    assert_eq!(selected(&app), "epsilon zeta\neta the");
    assert!(yanked(&app), "{}", status(&app));
    Ok(())
}

/// A click yanks nothing, whatever was selected before it: in the text, or anywhere else (the
/// status line), where the release used to yank a selection of more than a character.
#[tokio::test(flavor = "multi_thread")]
async fn a_click_yanks_nothing() -> anyhow::Result<()> {
    let (mut app, _dir) = app().await?;
    for (column, row) in [(14, 1), (14, 10), (40, 6)] {
        send_keys(&mut app, "ggw").await?;
        assert_eq!(selected(&app), "alpha ");
        app.editor.clear_status();
        click(&mut app, column, row).await?;
        assert!(!yanked(&app), "click at {column}, {row}: {}", status(&app));
    }
    // in the text: the cursor is put there
    click(&mut app, 14, 1).await?;
    assert_eq!(selected(&app), "e");
    Ok(())
}
