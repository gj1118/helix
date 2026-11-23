//! A buffer opens where its cursor was last: after `:buffer-close`, in a window new to it, and
//! (`persistence.rs`) after a restart.

use super::*;

use helix_term::application::Application;
use helix_view::{current_ref, editor::Action, DocumentId, ViewId};
use std::path::Path;

/// Six lines, `line 0` to `line 5`.
const SIX_LINES: &str = "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\n";

/// The line and column of the cursor of the focused window.
fn cursor_of(app: &Application) -> (usize, usize) {
    let (view, doc) = current_ref!(app.editor);
    cursor_in(app, view.id, doc.id())
}

/// The line and column of the cursor of `doc` in the window `view`.
fn cursor_in(app: &Application, view: ViewId, doc: DocumentId) -> (usize, usize) {
    let doc = app.editor.document(doc).expect("the document is open");
    let text = doc.text().slice(..);
    let cursor = doc.selection(view).primary().cursor(text);
    let line = text.char_to_line(cursor);
    (line, cursor - text.line_to_char(line))
}

/// The path of the document of the focused window.
fn current_path(app: &Application) -> Option<std::path::PathBuf> {
    let (_, doc) = current_ref!(app.editor);
    doc.path().map(Path::to_path_buf)
}

fn open_keys(path: &Path) -> String {
    format!(":o {}<ret>", path.display())
}

fn app_with(path: &Path) -> anyhow::Result<Application> {
    helpers::AppBuilder::new()
        .with_config(helpers::test_config())
        .with_file(path, None)
        .build()
}

/// `:buffer-close`, then the file opened again: the cursor is where it was, without the file
/// history (`old-files` is off in the tests).
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_buffer_opens_where_its_cursor_was() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(SIX_LINES)?;
    let path = helix_stdx::path::canonicalize(file.path());
    let mut app = app_with(&path)?;
    let old_files: bool = app.editor.config().persistence.old_files;
    assert!(!old_files, "no file history in the tests");

    helpers::send_keys(&mut app, "3jll").await?;
    assert_eq!(cursor_of(&app), (3_usize, 2_usize));
    helpers::send_keys(&mut app, ":bc<ret>").await?;
    assert_eq!(current_path(&app), None, "a new buffer instead");

    helpers::send_keys(&mut app, &open_keys(&path)).await?;
    assert_eq!(current_path(&app), Some(path.clone()));
    assert_eq!(cursor_of(&app), (3_usize, 2_usize));
    helpers::quit(app).await
}

/// A window showing a buffer for the first time starts where the cursor was last: where it is
/// in the other window (still open), then where it was in a window closed since. Each window
/// keeps its own place.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_new_to_a_buffer_starts_where_its_cursor_was_last() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(SIX_LINES)?;
    let path = helix_stdx::path::canonicalize(file.path());
    let mut app = app_with(&path)?;
    let first = app.editor.tree.focus;
    let doc_id = current_ref!(app.editor).1.id();

    helpers::send_keys(&mut app, "3jl").await?;
    // a new window, with a new buffer, then the file in it
    helpers::send_keys(&mut app, ":vnew<ret>").await?;
    assert_eq!(current_path(&app), None);
    helpers::send_keys(&mut app, &open_keys(&path)).await?;
    let second = app.editor.tree.focus;
    assert_ne!(second, first, "another window");
    assert_eq!(
        cursor_of(&app),
        (3_usize, 1_usize),
        "where it is in the first"
    );

    // the cursor moves up there, and the window closes: the next one starts on line 2
    helpers::send_keys(&mut app, "kl:q<ret>").await?;
    assert_eq!(app.editor.tree.focus, first);
    assert_eq!(
        cursor_of(&app),
        (3_usize, 1_usize),
        "the first kept its place"
    );
    helpers::send_keys(&mut app, ":vnew<ret>").await?;
    helpers::send_keys(&mut app, &open_keys(&path)).await?;
    let third = app.editor.tree.focus;
    assert_ne!(third, first);
    assert_eq!(
        cursor_of(&app),
        (2_usize, 2_usize),
        "where it was in the closed one"
    );
    assert_eq!(cursor_in(&app, first, doc_id), (3_usize, 1_usize));
    helpers::quit(app).await
}

/// A buffer loaded in the background (as `hx b a` loads `a`) starts where its cursor was last
/// in the window that shows it later; the buffer shown keeps its own place.
#[tokio::test(flavor = "multi_thread")]
async fn a_buffer_loaded_in_the_background_starts_where_its_cursor_was() -> anyhow::Result<()> {
    let a = helpers::temp_file_with_contents(SIX_LINES)?;
    let b = helpers::temp_file_with_contents(SIX_LINES)?;
    let a_path = helix_stdx::path::canonicalize(a.path());
    let b_path = helix_stdx::path::canonicalize(b.path());
    let mut app = app_with(&a_path)?;

    helpers::send_keys(&mut app, "4j:bc<ret>").await?;
    helpers::send_keys(&mut app, &open_keys(&b_path)).await?;
    assert_eq!(current_path(&app), Some(b_path.clone()));
    assert_eq!(
        app.editor.document_by_path(&a_path).map(|doc| doc.id()),
        None
    );
    assert_eq!(cursor_of(&app), (0_usize, 0_usize));

    let loaded: DocumentId = app.editor.open(&a_path, Action::Load)?;
    assert_eq!(current_path(&app), Some(b_path.clone()), "still showing b");
    assert_eq!(cursor_of(&app), (0_usize, 0_usize), "b keeps its place");
    helpers::send_keys(&mut app, &open_keys(&a_path)).await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), loaded);
    assert_eq!(cursor_of(&app), (4_usize, 0_usize));
    helpers::quit(app).await
}

/// A commit message (`old-files-exclusions`) opens at its start every time.
#[tokio::test(flavor = "multi_thread")]
async fn an_excluded_file_opens_at_its_start() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = helix_stdx::path::canonicalize(dir.path().join("COMMIT_EDITMSG"));
    std::fs::write(&path, SIX_LINES)?;
    let mut app = app_with(&path)?;

    helpers::send_keys(&mut app, "3j:bc<ret>").await?;
    helpers::send_keys(&mut app, &open_keys(&path)).await?;
    assert_eq!(current_path(&app), Some(path.clone()));
    assert_eq!(cursor_of(&app), (0_usize, 0_usize));
    helpers::quit(app).await
}

/// A file made shorter since it was closed (by another program), so that its last place is past
/// its end, opens at its start.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_shortened_since_opens_at_its_start() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(SIX_LINES)?;
    let path = helix_stdx::path::canonicalize(file.path());
    let mut app = app_with(&path)?;

    helpers::send_keys(&mut app, "5j:bc<ret>").await?;
    std::fs::write(&path, "short\n")?;
    helpers::send_keys(&mut app, &open_keys(&path)).await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "short\n");
    assert_eq!(cursor_of(&app), (0_usize, 0_usize));
    helpers::quit(app).await
}
