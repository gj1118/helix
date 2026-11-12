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

    // `<<` on the added lines: into the left side (the snapshot)
    send_keys(&mut app, "]c<lt><lt>").await?;
    let session = &app.editor.diff_sessions[0];
    let left = app
        .editor
        .document(session.left_doc)
        .unwrap()
        .text()
        .to_string();
    assert!(left.contains("fn added() {}\nfn more() {}\n"), "{left}");
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
    assert_eq!(
        left(&app),
        ("one\ntwo staged\n".to_string(), "index:main.rs".to_string())
    );
    assert_eq!(app.editor.diff_sessions[0].hunks.len(), 1);
    // another diff in the same window replaces it
    send_keys(&mut app, ":diff-git HEAD<ret>").await?;
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
