//! The recent file picker (`Space O`, `[editor.recent-picker]`).

use super::*;
use helix_term::application::Application;
use std::path::{Path, PathBuf};

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
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// The test config, the recent files kept in `recents`.
fn config(recents: &Path, enable: bool) -> Config {
    let mut config = helpers::test_config();
    config.editor.notifications.enable = false;
    config.editor.recent_picker.recents_path = recents.to_path_buf();
    config.editor.recent_picker.enable = enable;
    config
}

/// The recent files written (after writing what is left, as on exit).
fn written(app: &Application, recents: &Path) -> Vec<PathBuf> {
    app.editor.flush_recent_files();
    std::fs::read_to_string(recents)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn canonical(file: &tempfile::NamedTempFile) -> PathBuf {
    helix_stdx::path::canonicalize(file.path())
}

/// The rows of the screen once the picker lists `expected` (its files come from a thread).
async fn rows_listing(app: &mut Application, expected: &str) -> Vec<String> {
    for _ in 0..100 {
        let rows = app.rendered_rows();
        if rows.iter().any(|row| row.contains(expected)) {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let size = app.compositor.size();
        app.resize_terminal(size.width, size.height).await;
    }
    app.rendered_rows()
}

/// The files opened and saved are written, most recent first, each once, to `recents-path`.
#[tokio::test(flavor = "multi_thread")]
async fn opened_and_saved_files_are_recorded() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let recents = dir.path().join("recents.txt");
    let (a, b) = (
        helpers::temp_file_with_contents("a\n")?,
        helpers::temp_file_with_contents("b\n")?,
    );
    let mut app = helpers::AppBuilder::new()
        .with_file(a.path(), None)
        .with_config(config(&recents, true))
        .build()?;

    send(&mut app, &format!(":open {}<ret>", b.path().display())).await?;
    send(&mut app, &format!(":open {}<ret>", a.path().display())).await?;
    assert_eq!(written(&app, &recents), [canonical(&a), canonical(&b)]);

    // saved under a new name: the newest
    let saved = dir.path().join("saved.txt");
    send(&mut app, &format!(":w {}<ret>", saved.display())).await?;
    let files = written(&app, &recents);
    assert_eq!(files.len(), 3, "{files:?}");
    assert_eq!(files[0], helix_stdx::path::canonicalize(&saved));
    assert_eq!(files[1..], [canonical(&a), canonical(&b)]);

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// `Space O` lists the recent files, most recent first, without the current buffer's file
/// nor the files that no longer exist; Enter opens the one selected (the previous file).
#[tokio::test(flavor = "multi_thread")]
async fn the_picker_lists_the_recent_files() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let recents = dir.path().join("recents.txt");
    let (a, b) = (
        helpers::temp_file_with_contents("a\n")?,
        helpers::temp_file_with_contents("b\n")?,
    );
    // another editor's files: one still there, one gone
    let theirs = helpers::temp_file_with_contents("theirs\n")?;
    let gone = dir.path().join("gone.txt");
    std::fs::write(
        &recents,
        format!("{}\n{}\n", canonical(&theirs).display(), gone.display()),
    )?;

    let mut app = helpers::AppBuilder::new()
        .with_file(a.path(), None)
        .with_config(config(&recents, true))
        .build()?;
    app.resize_terminal(200, 30).await;
    send(&mut app, &format!(":open {}<ret>", b.path().display())).await?;

    send(&mut app, "<space>O").await?;
    let name = |file: &tempfile::NamedTempFile| {
        file.path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    let rows = rows_listing(&mut app, &name(&theirs)).await;
    let position = |text: &str| rows.iter().position(|row| row.contains(text));
    assert!(
        rows.iter().any(|row| row.contains("Recent Files")),
        "{rows:#?}"
    );
    // `a` (the previous file) first, then `theirs`; not `b` (current), not the gone file
    let first = position(&name(&a)).unwrap_or_else(|| panic!("{rows:#?}"));
    let second = position(&name(&theirs)).unwrap();
    assert!(first < second, "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("2/2")), "{rows:#?}");
    assert!(position("gone.txt").is_none(), "{rows:#?}");

    send(&mut app, "<ret>").await?;
    let path = helix_view::doc!(app.editor).path().map(Path::to_path_buf);
    assert_eq!(path, Some(canonical(&a)));

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// With `enable = false`, nothing is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_recorded_when_off() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let recents = dir.path().join("recents.txt");
    let a = helpers::temp_file_with_contents("a\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(a.path(), None)
        .with_config(config(&recents, false))
        .build()?;
    send(&mut app, ":w<ret>").await?;
    assert_eq!(written(&app, &recents), Vec::<PathBuf>::new());
    assert!(!recents.exists());
    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

#[test]
fn the_defaults() {
    let config = helix_view::editor::Config::default().recent_picker;
    assert!(config.enable);
    assert_eq!(config.max_show_entries, 50);
    assert_eq!(config.max_save_entries, 100);
    assert_eq!(config.recents_path, helix_loader::default_recents_file());
}
