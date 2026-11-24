//! What the fuzzer (`fuzz.rs`) found: each test is the few steps of a run that panicked.

use super::*;

use helix_core::Selection;
use helix_term::application::Application;
use helix_view::current_ref;

const TEN_LINES: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

/// The line of the cursor of the focused window.
fn cursor_line(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    let text = doc.text().slice(..);
    text.char_to_line(doc.selection(view.id).primary().cursor(text))
}

/// `:goto 9x` previews line 9 while `9` is typed, then fails: the cursor goes back where it
/// was, and nothing of the preview is left for a later `:goto` to put back. It used to stay
/// noted, and aborting a `:goto` in another, shorter buffer set that selection there: a
/// selection past the end of the text, and a panic.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_goto_leaves_no_selection_to_restore_in_another_buffer() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(TEN_LINES)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    helpers::send_keys(&mut app, ":goto 9").await?;
    assert_eq!(cursor_line(&app), 8_usize, "the preview");
    helpers::send_keys(&mut app, "x<ret>").await?;
    assert_eq!(cursor_line(&app), 0_usize, "back where it was");
    let noted: Option<Selection> = app.editor.last_selection.clone();
    assert_eq!(noted, None);

    // a line that is no number at all notes nothing either
    helpers::send_keys(&mut app, "8gg:goto x<ret>").await?;
    assert_eq!(cursor_line(&app), 7_usize);
    assert_eq!(app.editor.last_selection, None);

    // an empty buffer, and a `:goto` given up there
    helpers::send_keys(&mut app, ":new<ret>:goto 1<backspace><esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().len_chars(),
        1_usize,
        "the empty buffer's line ending"
    );
    assert_eq!(doc.selection(view.id), &Selection::single(0, 1));
    helpers::quit(app).await
}

/// A selection noted by a `:goto` preview that no longer fits the text is not put back.
#[tokio::test(flavor = "multi_thread")]
async fn a_goto_preview_is_not_restored_past_the_end_of_the_text() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(TEN_LINES)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    helpers::send_keys(&mut app, ":new<ret>").await?;
    // (as a preview in the ten-line buffer would have left it, before the fix above)
    app.editor.last_selection = Some(Selection::single(40, 41));
    helpers::send_keys(&mut app, ":goto <esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.selection(view.id), &Selection::single(0, 1));
    assert_eq!(app.editor.last_selection, None);
    helpers::quit(app).await
}

/// `:update` writes a modified buffer: it reads the `--no-code-actions` flag, which its
/// signature did not have (a failed assertion in debug builds, and a flag refused in release).
#[tokio::test(flavor = "multi_thread")]
async fn update_writes_a_modified_buffer_and_takes_the_write_flags() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    helpers::send_keys(&mut app, "ix<esc>:update<ret>").await?;
    helpers::run_event_loop_until_idle(&mut app).await;
    assert_eq!(std::fs::read_to_string(file.path())?, "xone\n");

    helpers::send_keys(
        &mut app,
        "iy<esc>:update --no-code-actions --no-format<ret>",
    )
    .await?;
    helpers::run_event_loop_until_idle(&mut app).await;
    assert_eq!(std::fs::read_to_string(file.path())?, "xyone\n");
    let (_, doc) = current_ref!(app.editor);
    assert!(!doc.is_modified());
    helpers::quit(app).await
}

/// A terminal too small for any row of text (and back): folds, soft wrap and the commands
/// that scroll by the height of the view have nothing to draw or to count with, and say so
/// without panicking.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_without_rows_draws_and_scrolls() -> anyhow::Result<()> {
    let text: String = (0..60)
        .map(|n| format!("fn item_{n}() {{\n    let x = {n};\n}}\n"))
        .collect();
    let file = helpers::temp_file_with_contents(&text)?;
    let mut config = helpers::test_config();
    config.editor.soft_wrap.enable = Some(true);
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;
    // a fold above the view's first line, and one in it
    helpers::send_keys(&mut app, "gg:fold --indent<ret>31gg:fold --indent<ret>").await?;
    let folds: usize = {
        let (view, doc) = current_ref!(app.editor);
        doc.fold_container(view.id).map_or(0, |folds| folds.len())
    };
    assert_eq!(folds, 2_usize);

    for (width, height) in [(5, 2), (1, 1), (2, 1), (10, 3), (3, 30), (90, 4)] {
        app.resize_terminal(width, height).await;
        // half pages and pages, both ways, and to the ends
        helpers::send_keys(&mut app, "<C-u><C-d><C-b><C-f>ggGzzztzb<C-u>").await?;
        let (view, doc) = current_ref!(app.editor);
        let rows: usize = view.inner_height(doc);
        assert!(rows <= height as usize, "{width}x{height}: {rows} rows");
    }
    app.resize_terminal(80, 24).await;
    helpers::send_keys(&mut app, "gg").await?;
    assert_eq!(cursor_line(&app), 0_usize);
    let first: String = app.rendered_rows().into_iter().next().unwrap_or_default();
    assert!(first.contains("fn item_0"), "{first}");
    helpers::quit(app).await
}
