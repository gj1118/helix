//! Side-by-side diffs (`:diff`, `:diff-git`, `hx --diff`).
use super::*;

use helix_core::diff_hunks::HunkKind;
use helix_term::application::Application;
use helix_view::diff_view::DiffStyles;
use helix_view::editor::GutterType;

const OLD: &str = "fn main() {\n    let total = price * count;\n    println!(\"{total}\");\n    removed();\n}\n\nfn other() {}\n";
const NEW: &str = "fn main() {\n    let total = cost * count + tax;\n    println!(\"{total}\");\n}\n\nfn added() {}\nfn more() {}\n\nfn other() {}\n";

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

/// The app on 100x14 with `NEW` open and `:diff` of `OLD` against it.
async fn app_with_diff(
    config: Config,
) -> anyhow::Result<(
    Application,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
)> {
    let old = helpers::temp_file_with_contents(OLD)?;
    let new = helpers::temp_file_with_contents(NEW)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(new.path(), None)
        .with_config(config)
        .build()?;
    app.resize_terminal(100, 14).await;
    send_keys(&mut app, &format!(":diff {}<ret>", old.path().display())).await?;
    Ok((app, old, new))
}

/// The side shown in the focused view, and the line of its cursor.
fn cursor(app: &Application) -> (&'static str, usize) {
    let view = app.editor.tree.focus;
    let session = app.editor.diff_session(view).expect("in a diff");
    let side = if view == session.left_view {
        "left"
    } else {
        "right"
    };
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    (
        side,
        text.char_to_line(doc.selection(view.id).primary().cursor(text)),
    )
}

/// The line of the cursor of the other side.
fn other_cursor(app: &Application) -> usize {
    let session = app.editor.diff_session(app.editor.tree.focus).unwrap();
    let other = if app.editor.tree.focus == session.left_view {
        session.right_view
    } else {
        session.left_view
    };
    let view = app.editor.tree.get(other);
    let doc = app.editor.document(view.doc).unwrap();
    let text = doc.text().slice(..);
    text.char_to_line(doc.selection(other).primary().cursor(text))
}

/// The column where `needle` starts on `row`.
fn column(app: &Application, row: u16, needle: &str) -> u16 {
    let rows = app.rendered_rows();
    let line = &rows[row as usize];
    let byte = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not on row {row}: {rows:#?}"));
    line[..byte].chars().count() as u16
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_shows_both_versions_side_by_side() -> anyhow::Result<()> {
    let (app, old, _new) = app_with_diff(helpers::test_config()).await?;
    let session = app.editor.diff_sessions.first().expect("a diff");
    assert_eq!(app.editor.tree.focus, session.right_view);
    // the old version on the left, a read-only snapshot titled after its file
    let left = app.editor.document(session.left_doc).unwrap();
    assert_eq!(left.text().to_string(), OLD);
    assert!(left.readonly && left.path().is_none());
    assert_eq!(
        left.title.as_deref(),
        Some(format!("{} (old)", old.path().display()).as_str())
    );
    let left_view = app.editor.tree.get(session.left_view);
    let right_view = app.editor.tree.get(session.right_view);
    assert!(left_view.area.x < right_view.area.x);
    assert_eq!(right_view.gutters.layout[0], GutterType::DiffConnector);
    assert!(!left_view
        .gutters
        .layout
        .contains(&GutterType::DiffConnector));

    // the changes: a changed line, a removed one, two added
    let kinds: Vec<_> = session
        .hunks
        .iter()
        .map(|hunk| (hunk.left.clone(), hunk.right.clone(), hunk.kind()))
        .collect();
    assert_eq!(
        kinds,
        [
            (1..2, 1..2, HunkKind::Changed),
            (3..4, 3..3, HunkKind::Removed),
            (6..6, 5..8, HunkKind::Added),
        ]
    );

    let rows = app.rendered_rows();
    assert!(rows[1].contains("let total = price * count;"), "{rows:#?}");
    assert!(
        rows[1].contains("let total = cost * count + ta"),
        "{rows:#?}"
    );
    assert!(rows[3].contains("removed();"), "{rows:#?}");
    // the status line of the left side: its title
    assert!(rows[12].contains("(old)"), "{rows:#?}");

    // the backgrounds: the changed lines, the changed words, not the others
    let styles = DiffStyles::new(&app.editor.theme);
    let bg = |x: u16, y: u16| app.rendered_cell(x, y).bg;
    let price = column(&app, 1, "price");
    let cost = column(&app, 1, "cost");
    let total = column(&app, 1, "total");
    assert_eq!(bg(price, 1), styles.change_emphasis.bg.unwrap());
    assert_eq!(bg(cost, 1), styles.change_emphasis.bg.unwrap());
    assert_eq!(bg(total, 1), styles.change.bg.unwrap());
    assert_eq!(bg(column(&app, 3, "removed"), 3), styles.delete.bg.unwrap());
    assert_eq!(bg(column(&app, 5, "fn added"), 5), styles.add.bg.unwrap());
    let unchanged = bg(column(&app, 2, "println"), 2);
    assert!(
        ![
            styles.add,
            styles.delete,
            styles.change,
            styles.change_emphasis
        ]
        .iter()
        .any(|style| style.bg == Some(unchanged)),
        "{unchanged:?}"
    );

    // the connector, in front of the right side: the changed line's band across it
    let x = right_view.area.x;
    for column in 0..9 {
        assert_eq!(
            bg(x + column, 1),
            styles.change.bg.unwrap(),
            "column {column}"
        );
    }
    // the removed line: full on the left of the connector, a thin line on its right
    assert_eq!(bg(x, 3), styles.delete.bg.unwrap());
    assert_eq!(app.rendered_cell(x + 8, 3).symbol.as_str(), "▔");
    // the added lines: a thin line on the left, full on the right, over three rows
    assert_eq!(app.rendered_cell(x, 6).symbol.as_str(), "▔");
    for row in 5..8 {
        assert_eq!(bg(x + 8, row), styles.add.bg.unwrap(), "row {row}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_keys_move_between_the_changes_and_line_the_sides_up() -> anyhow::Result<()> {
    let (mut app, _old, _new) = app_with_diff(helpers::test_config()).await?;
    assert_eq!(cursor(&app), ("right", 0));
    // `]c`, the other side follows
    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 1));
    assert_eq!(other_cursor(&app), 1);
    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 3)); // where the removed line was
    assert_eq!(other_cursor(&app), 3);
    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 5));
    assert_eq!(other_cursor(&app), 6);
    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 5));
    let (status, _) = app.editor.get_status().unwrap();
    assert_eq!(status, "No more changes after this one");
    // `[c`, with a count
    send_keys(&mut app, "2[c").await?;
    assert_eq!(cursor(&app), ("right", 1));

    // `]s`: the other side to the cursor's line
    send_keys(&mut app, "6j]s").await?;
    assert_eq!(cursor(&app), ("right", 7));
    assert_eq!(other_cursor(&app), 6);
    // `[d` / `]d`: both sides to the top, the bottom
    send_keys(&mut app, "[d").await?;
    assert_eq!((cursor(&app), other_cursor(&app)), (("right", 0), 0));
    send_keys(&mut app, "]d").await?;
    assert_eq!((cursor(&app), other_cursor(&app)), (("right", 8), 6));

    // in the left view too
    send_keys(&mut app, "<C-w>h[d]c").await?;
    assert_eq!(cursor(&app), ("left", 1));
    assert_eq!(other_cursor(&app), 1);

    // keys that are not diff keys still work, those that start one too (`>` indents)
    send_keys(&mut app, "<C-w>lgg<gt>j").await?;
    let doc = helix_view::doc!(app.editor);
    assert!(doc.text().to_string().starts_with("    fn main() {\n"));
    assert_eq!(cursor(&app), ("right", 1));
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_are_applied_from_one_side_to_the_other() -> anyhow::Result<()> {
    let (mut app, _old, _new) = app_with_diff(helpers::test_config()).await?;
    // `>>` on the changed line: the right side gets the old line back
    send_keys(&mut app, "]c<gt><gt>").await?;
    fn right(app: &Application) -> String {
        helix_view::doc!(app.editor).text().to_string()
    }
    assert!(
        right(&app).contains("let total = price * count;\n"),
        "{}",
        right(&app)
    );
    assert_eq!(app.editor.diff_sessions[0].hunks.len(), 2);
    // the removed line, put back
    send_keys(&mut app, "]c<gt><gt>").await?;
    assert!(
        right(&app).contains("println!(\"{total}\");\n    removed();\n}"),
        "{}",
        right(&app)
    );
    // undo, as any edit
    send_keys(&mut app, "u").await?;
    assert!(!right(&app).contains("removed();"));

    // `>>` from the left side too: into the right side (the left one is read-only)
    send_keys(&mut app, "<C-w>hgg]c<gt><gt><C-w>l").await?;
    assert!(
        right(&app).contains("let total = price * count;\n"),
        "{}",
        right(&app)
    );
    // `<<` is no diff key: `<` (unindent) as usual, which the read-only left side refuses
    send_keys(&mut app, "<C-w>h<lt><lt><C-w>l").await?;
    let session = &app.editor.diff_sessions[0];
    let left: String = app
        .editor
        .document(session.left_doc)
        .unwrap()
        .text()
        .to_string();
    assert_eq!(left, OLD);
    // nothing to apply where nothing changed
    send_keys(&mut app, "gg<gt><gt>").await?;
    let (status, _) = app.editor.get_status().unwrap();
    assert_eq!(status, "No change under the cursor");
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_closes_with_q_or_when_a_side_goes() -> anyhow::Result<()> {
    let (mut app, _old, new) = app_with_diff(helpers::test_config()).await?;
    let session = app.editor.diff_sessions[0].clone();
    // `q`: the snapshot goes, the right side stays as it was
    send_keys(&mut app, "q").await?;
    assert!(app.editor.diff_sessions.is_empty());
    assert!(!app.editor.tree.contains(session.left_view));
    assert!(app.editor.document(session.left_doc).is_none());
    let view = app.editor.tree.get(session.right_view);
    assert!(!view.gutters.layout.contains(&GutterType::DiffConnector));
    assert_eq!(
        helix_view::doc!(app.editor).path().map(|p| p.to_path_buf()),
        Some(helix_stdx::path::canonicalize(new.path()))
    );
    // `q` is a key of the diff views only: outside of them it replays a macro (none here)
    send_keys(&mut app, "q").await?;
    assert!(app.editor.diff_sessions.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);

    // closing the left view ends the diff too
    send_keys(
        &mut app,
        &format!(":diff {}<ret><C-w>h:q<ret>", _old.path().display()),
    )
    .await?;
    app.editor.refresh_diff_sessions();
    assert!(app.editor.diff_sessions.is_empty());
    let view = helix_view::view!(app.editor);
    assert!(!view.gutters.layout.contains(&GutterType::DiffConnector));
    // `:diff-close`
    send_keys(
        &mut app,
        &format!(":diff {}<ret>:diff-close<ret>", _old.path().display()),
    )
    .await?;
    assert!(app.editor.diff_sessions.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    Ok(())
}

/// `[editor.diff.keys]`: other keys for the actions, `""` for none.
#[tokio::test(flavor = "multi_thread")]
async fn diff_keys_can_be_changed() -> anyhow::Result<()> {
    let mut config = helpers::test_config();
    config.editor.diff = toml::from_str("[keys]\nclose = \"Q\"\nnext-change = \"\"")?;
    let (mut app, _old, _new) = app_with_diff(config).await?;
    // `]c` is Helix's again: the next comment (none in a text without syntax): no jump
    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 0));
    send_keys(&mut app, "Q").await?;
    assert!(app.editor.diff_sessions.is_empty());
    Ok(())
}

/// `hx --diff <old> <new>`
#[tokio::test(flavor = "multi_thread")]
async fn hx_diff_opens_a_diff_of_two_files() -> anyhow::Result<()> {
    let old = helpers::temp_file_with_contents(OLD)?;
    let new = helpers::temp_file_with_contents(NEW)?;
    let mut app = helpers::AppBuilder::new()
        .with_args(|args| {
            args.diff = true;
            args.files.insert(old.path().to_path_buf(), None);
            args.files.insert(new.path().to_path_buf(), None);
        })
        .build()?;
    app.resize_terminal(100, 14).await;
    send_keys(&mut app, "]c").await?;
    let session = app.editor.diff_sessions.first().expect("a diff");
    assert_eq!(session.hunks.len(), 3);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(cursor(&app), ("right", 1));
    assert_eq!(helix_view::doc!(app.editor).text().to_string(), NEW);

    // one file: an error
    let result = helix_term::args::Args::default();
    drop(result);
    let only_one = helpers::AppBuilder::new()
        .with_args(|args| {
            args.diff = true;
            args.files.insert(old.path().to_path_buf(), None);
        })
        .build();
    assert!(only_one
        .err()
        .unwrap()
        .to_string()
        .contains("--diff takes two files"));
    Ok(())
}

/// Runs the editor until `done` holds (true), or ten seconds pass (false): for background jobs
/// (`git show` of `:diff-git`), slower on some machines than an idle editor.
async fn until(app: &mut Application, mut done: impl FnMut(&Application) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // the stream stays open (`_keys`): an ended one stops the event loop
    let (_keys, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    while std::time::Instant::now() < deadline {
        if done(app) {
            return true;
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            app.event_loop_until_idle(&mut rx),
        )
        .await;
    }
    done(app)
}

/// The title of the old version shown, if a diff is.
fn left_title(app: &Application) -> Option<String> {
    let session = app.editor.diff_sessions.first()?;
    app.editor.document(session.left_doc)?.title.clone()
}

/// Waits for the diff of `:diff-git` titled `title`; fails with the status line otherwise.
async fn wait_for_diff(app: &mut Application, title: &str) {
    let shown = until(app, |app| left_title(app).as_deref() == Some(title)).await;
    assert!(
        shown,
        "no diff {title:?} (shown: {:?}); status: {:?}",
        left_title(app),
        app.editor.get_status()
    );
}

fn git(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    anyhow::ensure!(status.status.success(), "git {args:?}: {status:?}");
    Ok(())
}

/// `:diff-git`: the file against the index (what `git diff` shows), `HEAD` or another
/// revision; a file git doesn't have yet against nothing.
#[tokio::test(flavor = "multi_thread")]
async fn diff_git_compares_with_the_index_or_a_revision() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let root = repo.path();
    git(root, &["init", "-q"])?;
    let file = root.join("main.rs");
    std::fs::write(&file, "one\ntwo\n")?;
    git(root, &["add", "main.rs"])?;
    git(root, &["commit", "-qm", "first"])?;
    std::fs::write(&file, "one\ntwo staged\n")?;
    git(root, &["add", "main.rs"])?;
    std::fs::write(&file, "one\ntwo staged\nthree\n")?;

    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;
    app.resize_terminal(100, 14).await;
    let left = |app: &Application| -> (String, String) {
        let session = app.editor.diff_sessions.first().expect("a diff");
        let doc = app.editor.document(session.left_doc).unwrap();
        (doc.text().to_string(), doc.title.clone().unwrap())
    };
    send_keys(&mut app, ":diff-git<ret>").await?;
    wait_for_diff(&mut app, "index:main.rs").await;
    assert_eq!(
        left(&app),
        ("one\ntwo staged\n".to_string(), "index:main.rs".to_string())
    );
    assert_eq!(app.editor.diff_sessions[0].hunks.len(), 1);
    // another diff in the same window replaces it
    send_keys(&mut app, ":diff-git HEAD<ret>").await?;
    wait_for_diff(&mut app, "HEAD:main.rs").await;
    assert_eq!(app.editor.diff_sessions.len(), 1);
    assert_eq!(
        left(&app),
        ("one\ntwo\n".to_string(), "HEAD:main.rs".to_string())
    );
    assert_eq!(app.editor.tree.views().count(), 2);

    // a new file
    let added = root.join("new.rs");
    std::fs::write(&added, "fresh\n")?;
    send_keys(
        &mut app,
        &format!("q:o {}<ret>:diff-git<ret>", added.display()),
    )
    .await?;
    wait_for_diff(&mut app, "index:new.rs").await;
    assert_eq!(left(&app), (String::new(), "index:new.rs".to_string()));
    let session = &app.editor.diff_sessions[0];
    assert_eq!(session.hunks[0].kind(), HunkKind::Added);

    // a buffer without file: an error
    send_keys(&mut app, "q:new<ret>:diff-git<ret>").await?;
    let (status, _) = app.editor.get_status().unwrap();
    assert!(status.contains("no file"), "{status}");
    Ok(())
}

/// With `hx --diff` (git difftool), `:q` on the new version closes the diff and quits, so that
/// git goes on to the next file.
#[tokio::test(flavor = "multi_thread")]
async fn quitting_the_right_side_of_hx_diff_quits() -> anyhow::Result<()> {
    let old = helpers::temp_file_with_contents(OLD)?;
    let new = helpers::temp_file_with_contents(NEW)?;
    let mut app = helpers::AppBuilder::new()
        .with_args(|args| {
            args.diff = true;
            args.files.insert(old.path().to_path_buf(), None);
            args.files.insert(new.path().to_path_buf(), None);
        })
        .build()?;
    helpers::test_key_sequence(&mut app, Some(":q<ret>"), None, true).await?;
    Ok(())
}

/// `:diff-git` on a file opened through a symlinked folder (as on macOS, where temporary
/// folders are under `/var`, a link to `/private/var`, and git names the repository by its
/// real path).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn diff_git_works_through_symlinks() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let root = repo.path();
    git(root, &["init", "-q"])?;
    std::fs::write(root.join("main.rs"), "one\n")?;
    git(root, &["add", "main.rs"])?;
    git(root, &["commit", "-qm", "first"])?;
    std::fs::write(root.join("main.rs"), "one\ntwo\n")?;
    let links = tempfile::tempdir()?;
    let link = links.path().join("project");
    std::os::unix::fs::symlink(root, &link)?;

    let mut app = helpers::AppBuilder::new()
        .with_file(link.join("main.rs"), None)
        .build()?;
    send_keys(&mut app, ":diff-git<ret>").await?;
    wait_for_diff(&mut app, "index:main.rs").await;
    let session = &app.editor.diff_sessions[0];
    let left = app.editor.document(session.left_doc).unwrap();
    assert_eq!(left.text().to_string(), "one\n");
    Ok(())
}

/// 200 lines, `line 0` to `line 199`, and the same with 20 lines (`new 0` to `new 19`) put in
/// after `line 49`.
fn long_versions() -> (String, String) {
    let old: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
    let mut new = old.clone();
    new.splice(50..50, (0..20).map(|i| format!("new {i}")));
    (old.join("\n") + "\n", new.join("\n") + "\n")
}

async fn app_with_long_diff(
    config: Config,
) -> anyhow::Result<(
    Application,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
)> {
    let (old_text, new_text) = long_versions();
    let old = helpers::temp_file_with_contents(old_text)?;
    let new = helpers::temp_file_with_contents(new_text)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(new.path(), None)
        .with_config(config)
        .build()?;
    app.resize_terminal(100, 14).await;
    send_keys(&mut app, &format!(":diff {}<ret>", old.path().display())).await?;
    Ok((app, old, new))
}

/// The first line shown by the left (or right) side of the diff.
fn top_line(app: &Application, left: bool) -> usize {
    let session = app.editor.diff_sessions.first().expect("a diff");
    let view = if left {
        session.left_view
    } else {
        session.right_view
    };
    let doc = app.editor.document(app.editor.tree.get(view).doc).unwrap();
    let text = doc.text().slice(..);
    text.char_to_line(doc.view_offset(view).anchor)
}

/// The number of times `text` (a whole word: `line 7` is not in `line 70`) is on `row`.
fn times_on_row(app: &Application, row: usize, text: &str) -> usize {
    let rows = app.rendered_rows();
    rows[row]
        .match_indices(text)
        .filter(|(at, _)| {
            !rows[row][at + text.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
        .count()
}

/// The row where the left side shows `line {n}`, which must be the row where the right side
/// shows it too.
fn row_of_both(app: &Application, n: usize) -> usize {
    let text = format!("line {n}");
    let rows = app.rendered_rows();
    (0..12)
        .find(|row| times_on_row(app, *row, &text) == 2)
        .unwrap_or_else(|| panic!("{text:?} not on the same row of both sides: {rows:#?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sides_of_a_diff_scroll_together() -> anyhow::Result<()> {
    let (mut app, _old, _new) = app_with_long_diff(helpers::test_config()).await?;
    let session = app.editor.diff_sessions.first().cloned().expect("a diff");

    // down the right side, past the added lines: the left one follows, the cursor's line
    // (`line 99`, the right's 120th) level with its match
    send_keys(&mut app, "120gg").await?;
    assert_eq!(cursor(&app), ("right", 119));
    let top: usize = top_line(&app, true);
    assert!(top > 80, "the left side scrolled: {top}");
    let row: usize = row_of_both(&app, 99);
    assert!(row < 12);

    // up the left side: the right one follows
    send_keys(&mut app, "<C-w>h20gg").await?;
    assert_eq!(cursor(&app), ("left", 19));
    let right_top: usize = top_line(&app, false);
    assert!(right_top < 20, "the right side scrolled back: {right_top}");
    row_of_both(&app, 19);

    // a side scrolled without the focus (the mouse wheel, a plugin): the focused one follows,
    // its cursor brought into the window
    {
        let doc = app.editor.documents.get_mut(&session.right_doc).unwrap();
        let anchor = doc.text().line_to_char(150);
        doc.set_view_offset(
            session.right_view,
            helix_view::view::ViewPosition {
                anchor,
                horizontal_offset: 0,
                vertical_offset: 0,
            },
        );
    }
    send_keys(&mut app, "<esc>").await?;
    let left_top: usize = top_line(&app, true);
    assert!((125..135).contains(&left_top), "{left_top}");
    let (side, line): (&str, usize) = cursor(&app);
    assert_eq!(side, "left");
    assert!(
        (left_top..left_top + 12).contains(&line),
        "{line} in the window from {left_top}"
    );
    row_of_both(&app, 135);
    // and keys don't scroll it back
    send_keys(&mut app, "l").await?;
    assert_eq!(top_line(&app, true), left_top);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scroll_sync_can_be_turned_off() -> anyhow::Result<()> {
    let mut config = helpers::test_config();
    config.editor.diff.scroll_sync = false;
    let (mut app, _old, _new) = app_with_long_diff(config).await?;
    send_keys(&mut app, "120gg").await?;
    assert_eq!(cursor(&app), ("right", 119));
    let left_top: usize = top_line(&app, true);
    assert_eq!(left_top, 0);
    // `]s` still lines them up
    send_keys(&mut app, "]s").await?;
    row_of_both(&app, 99);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mouse_wheel_scrolls_both_sides() -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyModifiers as Modifiers, MouseEvent, MouseEventKind};
    #[cfg(not(windows))]
    use termina::event::{Event, Modifiers, MouseEvent, MouseEventKind};

    let (mut app, _old, _new) = app_with_long_diff(helpers::test_config()).await?;
    let session = app.editor.diff_sessions.first().cloned().expect("a diff");
    let wheel = |kind: MouseEventKind, column: u16, times: usize| -> Vec<Event> {
        (0..times)
            .map(|_| {
                Event::Mouse(MouseEvent {
                    kind,
                    column,
                    row: 5,
                    modifiers: Modifiers::empty(),
                })
            })
            .collect()
    };
    async fn send(app: &mut Application, events: Vec<Event>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for event in events {
            tx.send(Ok(event)).unwrap();
        }
        let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        app.event_loop_until_idle(&mut rx).await;
    }

    // the wheel over the left side (not focused): the focused right side follows
    send(&mut app, wheel(MouseEventKind::ScrollDown, 10, 10)).await;
    assert_eq!(app.editor.tree.focus, session.right_view);
    let left_top: usize = top_line(&app, true);
    assert_eq!(left_top, 30);
    row_of_both(&app, 35);

    // over the right side, past the added lines and far down a long file: the left side
    // follows (and a frame stays cheap with many changes out of the window)
    send(&mut app, wheel(MouseEventKind::ScrollDown, 80, 40)).await;
    let right_top: usize = top_line(&app, false);
    assert_eq!(right_top, 150);
    row_of_both(&app, 135);
    Ok(())
}

/// The changed words are worked out for the changes shown: one far down the file gets its
/// emphasis once it is in view.
#[tokio::test(flavor = "multi_thread")]
async fn changes_far_down_get_their_words_emphasized() -> anyhow::Result<()> {
    let old: String = (0..400)
        .map(|i| format!("let value_{i} = price;\n"))
        .collect();
    let new = old.replace("let value_350 = price;", "let value_350 = cost;");
    let old_file = helpers::temp_file_with_contents(old)?;
    let new_file = helpers::temp_file_with_contents(new)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(new_file.path(), None)
        .with_config(helpers::test_config())
        .build()?;
    app.resize_terminal(100, 14).await;
    send_keys(
        &mut app,
        &format!(":diff {}<ret>", old_file.path().display()),
    )
    .await?;
    let session = app.editor.diff_sessions.first().cloned().expect("a diff");
    // not shown yet: not worked out
    let none: &[std::ops::Range<usize>] = session.words(0, helix_core::diff_hunks::Side::Right);
    assert!(none.is_empty());

    send_keys(&mut app, "]c").await?;
    assert_eq!(cursor(&app), ("right", 350));
    let session = app.editor.diff_sessions.first().cloned().expect("a diff");
    let words: Vec<std::ops::Range<usize>> = session
        .words(0, helix_core::diff_hunks::Side::Right)
        .to_vec();
    assert_eq!(words.len(), 1);
    let styles = DiffStyles::new(&app.editor.theme);
    let row = (0..12u16)
        .find(|row| app.rendered_rows()[*row as usize].contains("value_350 = cost"))
        .expect("the change in view");
    let cost = column(&app, row, "cost");
    assert_eq!(
        app.rendered_cell(cost, row).bg,
        styles.change_emphasis.bg.unwrap()
    );
    let price = column(&app, row, "price");
    assert_eq!(
        app.rendered_cell(price, row).bg,
        styles.change_emphasis.bg.unwrap()
    );
    Ok(())
}

/// `:diff-buffers` snapshots a buffer without copying it: the snapshot shares its rope.
#[tokio::test(flavor = "multi_thread")]
async fn a_buffer_snapshot_shares_the_text() -> anyhow::Result<()> {
    let old = helpers::temp_file_with_contents("one\ntwo\n")?;
    let new = helpers::temp_file_with_contents("one\nthree\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(old.path(), None)
        .with_config(helpers::test_config())
        .build()?;
    let old_doc = helix_view::doc!(app.editor).id();
    send_keys(&mut app, &format!(":open {}<ret>", new.path().display())).await?;
    send_keys(
        &mut app,
        &format!(":diff-buffers {}<ret>", old.path().display()),
    )
    .await?;
    let session = app.editor.diff_sessions.first().cloned().expect("a diff");
    let snapshot = app.editor.document(session.left_doc).unwrap().text();
    let buffer = app.editor.document(old_doc).unwrap().text();
    let shared: bool = snapshot.is_instance(buffer);
    assert!(shared);
    assert_eq!(snapshot.to_string(), "one\ntwo\n");
    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// The old version (the left side) can't be changed: typing, deleting, pasting are refused,
/// with a word on why, and insert mode is left at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_old_version_is_read_only() -> anyhow::Result<()> {
    let (mut app, _old, _new) = app_with_diff(helpers::test_config()).await?;
    let left_text = |app: &Application| -> String {
        let session = &app.editor.diff_sessions[0];
        app.editor
            .document(session.left_doc)
            .unwrap()
            .text()
            .to_string()
    };
    send_keys(&mut app, "<C-w>hihello").await?;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    let (status, severity) = app.editor.get_status().expect("a message");
    assert_eq!(status, "The old version is read-only");
    assert_eq!(*severity, helix_view::editor::Severity::Error);
    // `hello` was typed as normal-mode keys: nothing they do changes the text
    assert_eq!(left_text(&app), OLD);
    for keys in ["dd", "xd", "yp", "o", "<gt>j", "~", "<C-a>"] {
        send_keys(&mut app, &format!("<esc>gg{keys}<esc>")).await?;
        let text: String = left_text(&app);
        assert_eq!(text, OLD, "after {keys}");
    }
    // the right side is edited as usual
    send_keys(&mut app, "<C-w>lggiok <esc>").await?;
    let right: String = helix_view::doc!(app.editor).text().to_string();
    assert!(right.starts_with("ok fn main()"), "{right}");
    Ok(())
}

/// Typing in the right side: after a pause (the idle timeout), still in insert mode, the
/// diff is worked out again, with the new lines as a change.
#[tokio::test(flavor = "multi_thread")]
async fn the_diff_follows_typing() -> anyhow::Result<()> {
    let (mut app, _old, _new) = app_with_diff(helpers::test_config()).await?;
    let kinds =
        |app: &Application| -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>, HunkKind)> {
            app.editor.diff_sessions[0]
                .hunks
                .iter()
                .map(|hunk| (hunk.left.clone(), hunk.right.clone(), hunk.kind()))
                .collect()
        };
    send_keys(&mut app, "ggOuse std::fmt;").await?;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Insert);
    assert_eq!(
        kinds(&app),
        [
            (0..0, 0..1, HunkKind::Added),
            (1..2, 2..3, HunkKind::Changed),
            (3..4, 4..4, HunkKind::Removed),
            (6..6, 6..9, HunkKind::Added),
        ]
    );
    // out of insert mode, after more edits in normal mode: worked out at once
    send_keys(&mut app, "<esc>jdd").await?;
    assert_eq!(
        kinds(&app),
        [
            // the new line in place of `fn main() {`, next to the changed one: one change
            (0..2, 0..2, HunkKind::Changed),
            (3..4, 3..3, HunkKind::Removed),
            (6..6, 5..8, HunkKind::Added),
        ]
    );
    Ok(())
}
