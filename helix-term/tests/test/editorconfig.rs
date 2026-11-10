//! `.editorconfig` files, end to end: read when a file opens, applied on `:w`, read again when
//! one is saved in Helix or the `editor-config` option changes.

use super::*;
use helix_core::{indent::IndentStyle, LineEnding};
use helix_term::application::Application;
use helix_view::Document;
use std::path::Path;

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
    // The loop returns on the idle timer, which it clears: armed again, a call without keys
    // (or one after the last returned idle) returns too instead of waiting for an event.
    app.editor.reset_idle_timer();
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

fn config(editor_config: bool) -> Config {
    let mut config = helpers::test_config();
    config.editor.notifications.enable = false;
    config.editor.editor_config = editor_config;
    config
}

/// A directory with `.editorconfig` (`editorconfig`) and the files of `files`.
fn project(editorconfig: &str, files: &[(&str, &str)]) -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join(".editorconfig"), editorconfig)?;
    for (name, contents) in files {
        std::fs::write(dir.path().join(name), contents)?;
    }
    Ok(dir)
}

/// The open document of `path`.
fn doc<'a>(app: &'a Application, path: &Path) -> &'a Document {
    let path = helix_stdx::path::canonicalize(path);
    app.editor
        .documents()
        .find(|doc| doc.path() == Some(&path))
        .unwrap_or_else(|| panic!("{path:?} is not open"))
}

/// Runs the event loop until the file written in the background holds `expected` (or a second).
async fn wait_for_write(app: &mut Application, path: &Path, expected: &[u8]) -> Vec<u8> {
    for _ in 0..100 {
        let bytes = std::fs::read(path).unwrap_or_default();
        if bytes == expected {
            return bytes;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        send(app, "").await.expect("no keys");
    }
    std::fs::read(path).unwrap_or_default()
}

/// Each property applies when the file opens; trimming and the final newline (in the line
/// ending of the file) on `:w`.
#[tokio::test(flavor = "multi_thread")]
async fn the_properties_apply_on_open_and_on_write() -> anyhow::Result<()> {
    let dir = project(
        "root = true\n\
         [*.txt]\n\
         indent_style = space\n\
         indent_size = 3\n\
         tab_width = 5\n\
         end_of_line = crlf\n\
         trim_trailing_whitespace = true\n\
         insert_final_newline = true\n\
         max_line_length = 60\n",
        &[("a.txt", "one  \r\ntwo\t")],
    )?;
    let path = dir.path().join("a.txt");
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config(true))
        .build()?;

    let doc = doc(&app, &path);
    let indent_style: IndentStyle = doc.indent_style;
    assert_eq!(indent_style, IndentStyle::Spaces(3));
    let indent_width: usize = doc.indent_width();
    assert_eq!(indent_width, 3);
    let tab_width: usize = doc.tab_width();
    assert_eq!(tab_width, 5);
    let line_ending: LineEnding = doc.line_ending;
    assert_eq!(line_ending, LineEnding::Crlf);
    let text_width: usize = doc.text_width();
    assert_eq!(text_width, 60);
    let (trim, final_newline): (bool, bool) =
        (doc.trim_trailing_whitespace(), doc.insert_final_newline());
    assert_eq!((trim, final_newline), (true, true));

    send(&mut app, ":w<ret>").await?;
    let written: Vec<u8> = wait_for_write(&mut app, &path, b"one\r\ntwo\r\n").await;
    assert_eq!(String::from_utf8(written)?, "one\r\ntwo\r\n");
    helpers::assert_status_not_error(&app.editor);
    Ok(())
}

/// `indent_size` without `indent_style`: the width of a file indented with spaces; a file
/// indented with tabs keeps them.
#[tokio::test(flavor = "multi_thread")]
async fn indent_size_alone_sets_the_width_of_the_spaces() -> anyhow::Result<()> {
    let dir = project(
        "root = true\n[*.txt]\nindent_size = 2\n",
        &[("spaces.txt", "a\n    b\n"), ("tabs.txt", "a\n\tb\n")],
    )?;
    let (spaces, tabs) = (dir.path().join("spaces.txt"), dir.path().join("tabs.txt"));
    let mut app = helpers::AppBuilder::new()
        .with_file(&spaces, None)
        .with_config(config(true))
        .build()?;
    send(&mut app, &format!(":open {}<ret>", tabs.display())).await?;

    let spaces_style: IndentStyle = doc(&app, &spaces).indent_style;
    assert_eq!(spaces_style, IndentStyle::Spaces(2));
    let tabs_style: IndentStyle = doc(&app, &tabs).indent_style;
    assert_eq!(tabs_style, IndentStyle::Tabs);
    // indent_size sets the tab width too
    let tab_width: usize = doc(&app, &tabs).tab_width();
    assert_eq!(tab_width, 2);
    Ok(())
}

/// `charset = utf-8-bom`: a new file starts with the byte order mark.
#[tokio::test(flavor = "multi_thread")]
async fn utf_8_bom_gives_a_new_file_the_mark() -> anyhow::Result<()> {
    let dir = project("root = true\n[*]\ncharset = utf-8-bom\n", &[])?;
    let path = dir.path().join("new.txt");
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config(true))
        .build()?;

    let encoding: &str = doc(&app, &path).encoding().name();
    assert_eq!(encoding, "UTF-8");
    send(&mut app, "ihi<esc>:w<ret>").await?;
    let expected: &[u8] = b"\xEF\xBB\xBFhi\n";
    let written: Vec<u8> = wait_for_write(&mut app, &path, expected).await;
    assert_eq!(written, expected);
    Ok(())
}

/// Number ranges match the numbers between, and a section that does not compile is skipped
/// alone (the sections after it still apply).
#[tokio::test(flavor = "multi_thread")]
async fn number_ranges_and_a_broken_section() -> anyhow::Result<()> {
    let dir = project(
        "root = true\n\
         [a{b]\n\
         indent_style = space\n\
         [part{1..3}.txt]\n\
         indent_style = tab\n\
         end_of_line = crlf\n",
        &[("part2.txt", "a\n  b\n"), ("part4.txt", "a\n  b\n")],
    )?;
    let (in_range, out_of_range) = (dir.path().join("part2.txt"), dir.path().join("part4.txt"));
    let mut app = helpers::AppBuilder::new()
        .with_file(&in_range, None)
        .with_config(config(true))
        .build()?;
    send(&mut app, &format!(":open {}<ret>", out_of_range.display())).await?;

    let in_range = doc(&app, &in_range);
    let (style, ending): (IndentStyle, LineEnding) = (in_range.indent_style, in_range.line_ending);
    assert_eq!((style, ending), (IndentStyle::Tabs, LineEnding::Crlf));
    let out_of_range = doc(&app, &out_of_range);
    let (style, ending): (IndentStyle, LineEnding) =
        (out_of_range.indent_style, out_of_range.line_ending);
    assert_eq!((style, ending), (IndentStyle::Spaces(2), LineEnding::LF));
    Ok(())
}

/// A `.editorconfig` saved in Helix applies to the open files under its directory right away
/// (and not to the others).
#[tokio::test(flavor = "multi_thread")]
async fn saving_an_editorconfig_applies_it_to_the_open_files() -> anyhow::Result<()> {
    let dir = project(
        "root = true\n[*.txt]\nindent_style = tab\n",
        &[("a.txt", "a\n")],
    )?;
    let elsewhere = project("root = true\n", &[("b.txt", "b\n")])?;
    let (a, b) = (dir.path().join("a.txt"), elsewhere.path().join("b.txt"));
    let editorconfig = dir.path().join(".editorconfig");
    let mut app = helpers::AppBuilder::new()
        .with_file(&a, None)
        .with_config(config(true))
        .build()?;
    send(&mut app, &format!(":open {}<ret>", b.display())).await?;
    let before: IndentStyle = doc(&app, &a).indent_style;
    assert_eq!(before, IndentStyle::Tabs);

    send(&mut app, &format!(":open {}<ret>", editorconfig.display())).await?;
    send(
        &mut app,
        ":set auto-pairs false<ret>%croot = true<ret>[*]<ret>indent_style = space<ret>\
         indent_size = 2<ret>end_of_line = crlf<esc>:w<ret>",
    )
    .await?;
    let expected = "root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\n";
    let written = wait_for_write(&mut app, &editorconfig, expected.as_bytes()).await;

    assert_eq!(String::from_utf8(written)?, expected);
    // the save event, which reads the file again
    send(&mut app, "").await?;

    let a = doc(&app, &a);
    let (style, ending): (IndentStyle, LineEnding) = (a.indent_style, a.line_ending);
    assert_eq!((style, ending), (IndentStyle::Spaces(2), LineEnding::Crlf));
    // b.txt is under another directory: nothing changes
    let b = doc(&app, &b);
    let (style, ending): (IndentStyle, LineEnding) = (b.indent_style, b.line_ending);
    assert_eq!((style, ending), (IndentStyle::Tabs, LineEnding::LF));
    Ok(())
}

/// `editor-config = false` ignores the files; `:set editor-config` switches the open files over.
#[tokio::test(flavor = "multi_thread")]
async fn the_editor_config_option_turns_it_off_and_on() -> anyhow::Result<()> {
    let dir = project(
        "root = true\n[*.txt]\nindent_style = tab\nend_of_line = crlf\n",
        &[("a.txt", "a\n  b\n")],
    )?;
    let path = dir.path().join("a.txt");

    let app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config(false))
        .build()?;
    let off = doc(&app, &path);
    let (style, ending): (IndentStyle, LineEnding) = (off.indent_style, off.line_ending);
    assert_eq!((style, ending), (IndentStyle::Spaces(2), LineEnding::LF));
    let trim: bool = off.trim_trailing_whitespace();
    assert!(!trim);
    drop(app);

    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config(true))
        .build()?;
    let on = doc(&app, &path);
    let (style, ending): (IndentStyle, LineEnding) = (on.indent_style, on.line_ending);
    assert_eq!((style, ending), (IndentStyle::Tabs, LineEnding::Crlf));

    send(&mut app, ":set editor-config false<ret>").await?;
    let off = doc(&app, &path);
    let (style, ending): (IndentStyle, LineEnding) = (off.indent_style, off.line_ending);
    assert_eq!((style, ending), (IndentStyle::Spaces(2), LineEnding::LF));

    send(&mut app, ":set editor-config true<ret>").await?;
    let on = doc(&app, &path);
    let (style, ending): (IndentStyle, LineEnding) = (on.indent_style, on.line_ending);
    assert_eq!((style, ending), (IndentStyle::Tabs, LineEnding::Crlf));
    Ok(())
}
