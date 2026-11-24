use super::*;
use helix_term::{config::Config, keymap};
use helix_view::{editor, persistence};
use std::{fs::File, io::Read};
use tempfile::{NamedTempFile, TempPath};

fn init_persistence_files() -> anyhow::Result<(TempPath, TempPath, TempPath, TempPath)> {
    let command_file = NamedTempFile::new()?;
    let command_path = command_file.into_temp_path();
    helix_loader::initialize_command_histfile(Some(command_path.to_path_buf()));

    let search_file = NamedTempFile::new()?;
    let search_path = search_file.into_temp_path();
    helix_loader::initialize_search_histfile(Some(search_path.to_path_buf()));

    let file_file = NamedTempFile::new()?;
    let file_path = file_file.into_temp_path();
    helix_loader::initialize_file_histfile(Some(file_path.to_path_buf()));

    let clipboard_file = NamedTempFile::new()?;
    let clipboard_path = clipboard_file.into_temp_path();
    helix_loader::initialize_clipboard_file(Some(clipboard_path.to_path_buf()));

    Ok((command_path, search_path, file_path, clipboard_path))
}

fn config_with_persistence() -> Config {
    let mut editor_config = editor::Config::default();
    editor_config.persistence.old_files = true;
    editor_config.persistence.commands = true;
    editor_config.persistence.search = true;
    editor_config.persistence.clipboard = true;
    editor_config.persistence.search_trim = 3;

    Config {
        theme: None,
        keys: keymap::default(),
        editor: editor_config,
        plugins: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_persistence() -> anyhow::Result<()> {
    let (_, search_histfile_path, _, _) = init_persistence_files()?;
    let mut file = tempfile::NamedTempFile::new()?;

    // Session 1:
    // open a new file,
    // add a newline, then a,
    // write-quit
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_persistence())
            .with_file_history()
            .with_file(file.path(), None)
            .build()?,
        Some("oa<esc>:wq<ret>"),
        None,
        true,
    )
    .await?;

    // Sanity check contents of file after first session
    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("\na\n"))?;

    // Session 2:
    // open same file,
    // add newline, then b,
    // copy the line ("b\n")
    // search for "a"
    // go back down to b
    // use last command (write-quit)
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_persistence())
            .with_file_history()
            .with_file(file.path(), None)
            .build()?,
        Some("ob<esc>xy/a<ret>j:<up><ret>"),
        None,
        true,
    )
    .await?;

    // This verifies both that the file position was persisted (since the b is inserted after the
    // a), and the command history (":<up>" resolves to the ":wq" from session 1)
    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("\na\nb\n"))?;

    // Session 3:
    // open same file,
    // paste
    // use last search ("/a")
    // append a
    // search for "1", "2", and "3" in sequence.
    // use last command (write-quit)
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_persistence())
            .with_file_history()
            .with_file(file.path(), None)
            .build()?,
        Some("p/<up><ret>aa<esc>/1<ret>/2<ret>/3<ret>:<up><ret>"),
        None,
        true,
    )
    .await?;

    // This verifies search history was persisted ("/<up>" resolves to "/a" from session 2), and
    // the clipboard was persisted (paste pastes the "b\n" copied in session 2)
    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("\naa\nb\nb\n"))?;

    // Session 4:
    // open same file
    // use last command (write-quit)
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_persistence())
            .with_file_history()
            .with_file(file.path(), None)
            .build()?,
        Some(":<up><ret>"),
        None,
        true,
    )
    .await?;

    // NOTE: This time we check the search history file, instead of the edited file
    let mut search_histfile = File::open(search_histfile_path)?;
    let mut search_histfile_contents = String::new();
    search_histfile.read_to_string(&mut search_histfile_contents)?;
    // This verifies that trimming the persistent state files is working correctly, because
    // session 3 sent more searches (4: "/a", "/1", "/2", "/3") than the trim limit (3), so when
    // session 4 starts, it should perform a trim, removing the oldest entry ("/a") while leaving
    // the other 3 intact.
    // Persistence data is encoded using bincode: each entry is a little-endian u64 length
    // followed by its UTF-8 bytes.
    let mut expected_contents = Vec::new();
    for entry in ["1", "2", "3"] {
        expected_contents.extend_from_slice(&1_u64.to_le_bytes());
        expected_contents.extend_from_slice(entry.as_bytes());
    }
    assert_eq!(search_histfile_contents.into_bytes(), expected_contents);

    Ok(())
}

/// A config with persistent undo on, its histories kept in `dir`.
fn config_with_undo(dir: &std::path::Path) -> Config {
    let mut editor_config = editor::Config::default();
    // no file history: the place a file opens at stays the same whatever ran before
    editor_config.persistence.old_files = false;
    editor_config.persistence.undo = true;
    editor_config.persistence.undo_dir = Some(dir.to_path_buf());
    Config {
        theme: None,
        keys: keymap::default(),
        editor: editor_config,
        plugins: None,
    }
}

fn buffer_text(app: &helix_term::application::Application) -> String {
    helix_view::current_ref!(app.editor).1.text().to_string()
}

/// Edits made and saved before the file was closed can be undone after it is opened again,
/// in order and back to the first version.
#[tokio::test(flavor = "multi_thread")]
async fn undo_history_survives_closing_the_file() -> anyhow::Result<()> {
    let undo_dir = tempfile::tempdir()?;
    let mut file = helpers::temp_file_with_contents("one\n")?;

    // Session 1: two edits, each saved, then quit.
    let mut app = helpers::AppBuilder::new()
        .with_config(config_with_undo(undo_dir.path()))
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("otwo<esc>:w<ret>"), None),
            (Some("othree<esc>:wq<ret>"), None),
        ],
        true,
    )
    .await?;
    helpers::assert_file_has_content(&mut file, "one\ntwo\nthree\n")?;
    let undo_files = std::fs::read_dir(undo_dir.path())?.count();
    assert_eq!(undo_files, 1_usize, "one undo file for the file");

    // Session 2: the history is back, and the buffer is not modified.
    let mut app = helpers::AppBuilder::new()
        .with_config(config_with_undo(undo_dir.path()))
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    let doc = helix_view::doc!(app.editor);
                    assert!(!doc.is_modified(), "a restored history is not a change");
                }),
            ),
            (
                Some("u"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\n".to_string())),
            ),
            (
                Some("u"),
                Some(&|app| {
                    assert_eq!(buffer_text(app), "one\n".to_string());
                    assert!(helix_view::doc!(app.editor).is_modified());
                }),
            ),
            // past the first version there is nothing more; U redoes
            (
                Some("uU"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\n".to_string())),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// A file changed by another program since it was saved gets no history: undo would
/// otherwise apply edits made to a different text.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_changed_elsewhere_gets_no_undo_history() -> anyhow::Result<()> {
    let undo_dir = tempfile::tempdir()?;
    let file = helpers::temp_file_with_contents("one\n")?;
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_undo(undo_dir.path()))
            .with_file(file.path(), None)
            .build()?,
        Some("otwo<esc>:wq<ret>"),
        None,
        true,
    )
    .await?;

    std::fs::write(file.path(), "something else\n")?;
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config_with_undo(undo_dir.path()))
            .with_file(file.path(), None)
            .build()?,
        Some("u"),
        Some(&|app| {
            assert_eq!(buffer_text(app), "something else\n".to_string());
            assert!(!helix_view::doc!(app.editor).is_modified());
        }),
        false,
    )
    .await?;
    Ok(())
}

/// With persistent undo off (the default), nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn no_undo_file_without_the_setting() -> anyhow::Result<()> {
    let undo_dir = tempfile::tempdir()?;
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut config = config_with_undo(undo_dir.path());
    config.editor.persistence.undo = false;
    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_config(config)
            .with_file(file.path(), None)
            .build()?,
        Some("otwo<esc>:wq<ret>"),
        None,
        true,
    )
    .await?;
    assert_eq!(std::fs::read_dir(undo_dir.path())?.count(), 0_usize);
    Ok(())
}

/// Sets the file history to a new file, when no test set it yet (it is set once for all the
/// tests: each opens files of its own). The file is made by the first place written.
fn init_file_history() -> anyhow::Result<()> {
    let path = NamedTempFile::new()?.into_temp_path();
    helix_loader::initialize_file_histfile(Some(path.to_path_buf()));
    path.close()?;
    Ok(())
}

/// The line and column of the cursor of the focused window.
fn cursor_of(app: &helix_term::application::Application) -> (usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let line = text.char_to_line(cursor);
    (line, cursor - text.line_to_char(line))
}

const SIX_LINES: &str = "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\n";

/// With the default config, a file opens where its cursor was when Helix quit, its window's
/// scroll too; a place given on the command line (`hx file:2`) wins.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_opens_where_its_cursor_was_last_time() -> anyhow::Result<()> {
    init_file_history()?;
    let old_files: bool = editor::Config::default().persistence.old_files;
    assert!(old_files, "on by default");
    let file = helpers::temp_file_with_contents(SIX_LINES)?;

    // Session 1: the cursor on line 4, column 2, then quit
    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    helpers::send_keys(&mut app, "4jll").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let scroll: helix_view::view::ViewPosition = doc.view_offset(view.id);
    helpers::quit(app).await?;

    // Session 2: there again
    let app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert_eq!(cursor_of(&app), (4_usize, 2_usize));
    let (view, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(doc.view_offset(view.id), scroll);
    helpers::quit(app).await?;

    // Session 3: a place given wins
    let app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), Some(helix_core::Position::new(1, 0)))
        .build()?;
    assert_eq!(cursor_of(&app), (1_usize, 0_usize));
    helpers::quit(app).await?;

    // Session 4: the place of session 3 is the last one now
    let app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert_eq!(cursor_of(&app), (1_usize, 0_usize));
    helpers::quit(app).await
}

/// Helix stopped without `:q` (a signal: its windows never closed) still notes where the
/// cursor was, as it exits.
#[tokio::test(flavor = "multi_thread")]
async fn the_place_is_kept_when_helix_exits_without_quitting() -> anyhow::Result<()> {
    init_file_history()?;
    let file = helpers::temp_file_with_contents(SIX_LINES)?;

    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    helpers::send_keys(&mut app, "5j").await?;
    let errors = app.close().await;
    assert!(errors.is_empty(), "{errors:?}");

    let app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert_eq!(cursor_of(&app), (5_usize, 0_usize));
    helpers::quit(app).await
}

/// Each file of `hx a b` opens at its own last place: the file shown is not given the place of
/// another one loaded after it.
#[tokio::test(flavor = "multi_thread")]
async fn files_opened_together_each_open_at_their_place() -> anyhow::Result<()> {
    init_file_history()?;
    let shown = helpers::temp_file_with_contents(SIX_LINES)?;
    let loaded = helpers::temp_file_with_contents(SIX_LINES)?;

    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(loaded.path(), None)
        .build()?;
    helpers::send_keys(&mut app, "3j").await?;
    helpers::quit(app).await?;

    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(shown.path(), None)
        .with_file(loaded.path(), None)
        .build()?;
    let (_, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        doc.path().map(std::path::Path::to_path_buf),
        Some(helix_stdx::path::canonicalize(shown.path()))
    );
    assert_eq!(cursor_of(&app), (0_usize, 0_usize), "never moved in it");
    helpers::send_keys(&mut app, &format!(":o {}<ret>", loaded.path().display())).await?;
    assert_eq!(cursor_of(&app), (3_usize, 0_usize));
    helpers::quit(app).await
}

/// A window closed, then its buffer: the file history gets the file once (the same place).
#[tokio::test(flavor = "multi_thread")]
async fn a_place_is_written_once() -> anyhow::Result<()> {
    init_file_history()?;
    let file = helpers::temp_file_with_contents(SIX_LINES)?;
    let path = helix_stdx::path::canonicalize(file.path());

    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    // the file in a second window too, where the cursor moves, which then closes
    helpers::send_keys(&mut app, ":vsplit<ret>2j:q<ret>").await?;
    helpers::send_keys(&mut app, ":bc<ret>").await?;
    helpers::quit(app).await?;

    let places: Vec<persistence::FileHistoryEntry> = persistence::read_file_history()
        .into_iter()
        .filter(|entry| entry.path == path)
        .collect();
    assert_eq!(places.len(), 1_usize, "{places:?}");
    let text = helix_core::Rope::from(SIX_LINES);
    let cursor = places[0].selection.primary().cursor(text.slice(..));
    assert_eq!(text.char_to_line(cursor), 2_usize);
    Ok(())
}

/// `remember-cursor-position = false` with `old-files` on: no place is written, and a file opens
/// at its start after a restart even with a place from an earlier session.
#[tokio::test(flavor = "multi_thread")]
async fn without_remember_cursor_position_no_place_is_kept() -> anyhow::Result<()> {
    init_file_history()?;
    let file = helpers::temp_file_with_contents(SIX_LINES)?;
    let path = helix_stdx::path::canonicalize(file.path());
    let off = || {
        let mut config = helpers::test_config();
        config.editor.remember_cursor_position = false;
        config
    };

    // off: nothing written
    let mut app = helpers::AppBuilder::new()
        .with_config(off())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert!(app.editor.config().persistence.old_files);
    helpers::send_keys(&mut app, "4j").await?;
    helpers::quit(app).await?;
    let places = persistence::read_file_history()
        .into_iter()
        .filter(|entry| entry.path == path)
        .count();
    assert_eq!(places, 0_usize);

    // on: written
    let mut app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    helpers::send_keys(&mut app, "2j").await?;
    helpers::quit(app).await?;

    // off again: the place written is not used
    let app = helpers::AppBuilder::new()
        .with_config(off())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert_eq!(cursor_of(&app), (0_usize, 0_usize));
    helpers::quit(app).await?;

    // on: it is
    let app = helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file_history()
        .with_file(file.path(), None)
        .build()?;
    assert_eq!(cursor_of(&app), (2_usize, 0_usize));
    helpers::quit(app).await
}
