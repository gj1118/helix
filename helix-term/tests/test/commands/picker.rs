use super::*;

use helix_term::application::Application;
use helix_view::quicklist::{Entry, Position, Target};

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

/// An app with the files of `files` in the quicklist, whose picker (`<space>q`) previews them.
fn app_with_quicklist(files: &[&std::path::Path]) -> anyhow::Result<Application> {
    let mut app = AppBuilder::new().build()?;
    for file in files {
        app.editor.quicklist.push(Entry::new(
            Target::Path(file.to_path_buf()),
            Position::Lines { start: 0, end: 0 },
            0,
            "",
        ));
    }
    Ok(app)
}

/// Files too big to read on the UI thread are read in the background, then shown.
#[tokio::test(flavor = "multi_thread")]
async fn big_files_are_previewed_once_read() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let big = dir.path().join("big.txt");
    let mut content = String::from("the first line of a big file\n");
    while content.len() < 1024 * 1024 {
        content.push_str("filler filler filler filler filler filler filler filler\n");
    }
    std::fs::write(&big, content)?;

    let mut app = app_with_quicklist(&[&big])?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(
                        screen.contains("the first line of a big file"),
                        "no preview:\n{screen}"
                    );
                    assert!(!screen.contains("<Loading…>"), "{screen}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

/// The preview of a file without a language has no highlighting to wait for: showing it
/// must not keep asking for it, which kept the editor busy (and never idle) for as long as
/// the preview was shown.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_text_preview_lets_the_editor_idle() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plain = dir.path().join("notes-without-a-language");
    std::fs::write(&plain, "some plain words\n")?;

    let mut app = app_with_quicklist(&[&plain])?;
    let run = test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(screen.contains("some plain words"), "no preview:\n{screen}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .map_err(|_| anyhow::anyhow!("the editor never became idle"))?
}

/// A command run from the command palette can open a picker.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_palette_opens_pickers() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>?buffer_picker<ret>"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(screen.contains("Buffers"), "no buffer picker:\n{screen}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

/// Each typable command is in the command palette once, not once per alias.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_palette_lists_commands_once() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                // `:write-quit` has the aliases `wq` and `x`
                Some("<space>?:write-quit"),
                Some(&|app| {
                    let screen = screen(app);
                    // the rows of the list, not the prompt (with its `matched/total` count)
                    let rows = screen
                        .lines()
                        .filter(|row| {
                            let mut words =
                                row.split(['▏', '>', ' ']).filter(|word| !word.is_empty());
                            words.next() == Some(":write-quit")
                                && words.next().is_some_and(|word| !word.contains('/'))
                        })
                        .count();
                    assert_eq!(rows, 1, "{screen}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

/// A huge count does not walk around the buffers that many times.
#[tokio::test(flavor = "multi_thread")]
async fn goto_next_buffer_with_a_huge_count() -> anyhow::Result<()> {
    let first = tempfile::NamedTempFile::new()?;
    let second = tempfile::NamedTempFile::new()?;
    let mut app = AppBuilder::new().with_file(first.path(), None).build()?;
    let open = format!(":o {}<ret>", second.path().display());
    test_key_sequences(
        &mut app,
        vec![
            (Some(&open), None),
            (
                // an odd count: the other buffer
                Some("99999999gn"),
                Some(&|app| {
                    let path = helix_view::doc!(app.editor)
                        .path()
                        .map(|path| path.to_path_buf());
                    let expected = helix_stdx::path::canonicalize(first.path());
                    assert_eq!(path, Some(expected));
                }),
            ),
        ],
        false,
    )
    .await
}

/// `:vsplit` (and `:hsplit`) on a directory opens a file picker whose `Enter` opens the file in
/// a split, as the command says, not in place of the current buffer.
#[tokio::test(flavor = "multi_thread")]
async fn vsplit_on_a_directory_opens_the_picked_file_in_a_split() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("only-file.txt");
    std::fs::write(&file, "picked\n")?;

    let mut app = AppBuilder::new().build()?;
    let vsplit = format!(":vsplit {}<ret>", dir.path().display());
    test_key_sequences(
        &mut app,
        vec![
            (Some(&vsplit), None),
            (
                Some("<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.tree.views().count(), 2);
                    let path = helix_view::doc!(app.editor)
                        .path()
                        .map(|path| path.to_path_buf());
                    assert_eq!(path, Some(helix_stdx::path::canonicalize(&file)));
                }),
            ),
            // close the split, so that the closing `:q!` quits
            (Some(":q!<ret>"), None),
        ],
        false,
    )
    .await
}
