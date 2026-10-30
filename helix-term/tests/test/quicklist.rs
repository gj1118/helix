use super::*;

use helix_view::quicklist::Target;

/// The focused file's name and the line of its cursor.
fn at(app: &helix_term::application::Application) -> String {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let name = doc
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "[scratch]".to_owned());
    format!("{name}:{}", doc.cursor_line(view.id))
}

fn two_files() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "a0\na1\na2\na3\na4\n")?;
    std::fs::write(&b, "b0\nb1\nb2\nb3\nb4\n")?;
    Ok((dir, a, b))
}

#[tokio::test(flavor = "multi_thread")]
async fn added_locations_are_walked_in_order_and_wrap() -> anyhow::Result<()> {
    let (_dir, a, _) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    test_key_sequences(
        &mut app,
        vec![
            // entries at lines 1, 3 and 0
            (
                Some("j<A-q>jj<A-q>gg<A-q>"),
                Some(&|app| {
                    assert_eq!(app.editor.quicklist.len(), 3);
                    assert_eq!(app.editor.quicklist.current(), None);
                }),
            ),
            // the first visit goes to the first entry
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:3"))),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:0"))),
            // wraps at the end
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            (Some("[q"), Some(&|app| assert_eq!(at(app), "a.txt:0"))),
            (Some("[q"), Some(&|app| assert_eq!(at(app), "a.txt:3"))),
            // counts
            (Some("2]q"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            // quicklist jumps are on the jumplist
            (
                Some("gg2jj<C-o>"),
                Some(&|app| assert_eq!(at(app), "a.txt:1")),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn entries_in_other_files_are_visited_and_in_file_navigation_skips_them() -> anyhow::Result<()>
{
    let (_dir, a, b) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    let open_b = format!(":o {}<ret>", b.display());
    let open_a = format!(":o {}<ret>", a.display());
    test_key_sequences(
        &mut app,
        vec![
            (Some("j<A-q>"), None), // a:1
            (Some(open_b.as_str()), None),
            (Some("jj<A-q>"), None), // b:2
            (Some(open_a.as_str()), None),
            (Some("gg4j<A-q>"), None), // a:4
            (Some("gg"), Some(&|app| assert_eq!(at(app), "a.txt:0"))),
            // all entries, across files
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "b.txt:2"))),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:4"))),
            // only entries of the current file
            (Some("[l"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            (Some("]l"), Some(&|app| assert_eq!(at(app), "a.txt:4"))),
            (Some("]l"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn unsaved_buffers_can_be_collected() -> anyhow::Result<()> {
    let (_dir, a, _) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":new<ret>iscratch<esc><A-q>"),
                Some(&|app| {
                    assert!(matches!(
                        app.editor.quicklist.entries()[0].target,
                        Target::Doc(_)
                    ));
                }),
            ),
            (
                Some(":bp<ret>"),
                Some(&|app| assert_eq!(at(app), "a.txt:0")),
            ),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "[scratch]:0"))),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_q_copies_the_matched_picker_entries() -> anyhow::Result<()> {
    let (_dir, a, b) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    let open_b = format!(":o {}<ret>", b.display());
    test_key_sequences(
        &mut app,
        vec![
            (Some(open_b.as_str()), None),
            // the buffer picker lists both files
            (
                Some("<space>b<C-q>"),
                Some(&|app| {
                    assert_eq!(app.editor.quicklist.len(), 2);
                }),
            ),
            // the picker is still open
            (
                None,
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("a.txt") && rows.contains("b.txt"), "{rows}");
                }),
            ),
            (Some("<esc>"), None),
            // filtering narrows what is copied
            (
                Some("<space>ba.txt<C-q><esc>"),
                Some(&|app| {
                    assert_eq!(app.editor.quicklist.len(), 1);
                    assert_eq!(
                        app.editor.quicklist.entries()[0].target,
                        Target::Path(
                            app.editor
                                .documents
                                .values()
                                .find(|d| d.path().is_some_and(|p| p.ends_with("a.txt")))
                                .unwrap()
                                .path()
                                .unwrap()
                                .to_owned()
                        )
                    );
                }),
            ),
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:0"))),
            // a picker without matches clears the list
            (
                Some("<space>bnomatchatall<C-q><esc>"),
                Some(&|app| {
                    assert!(app.editor.quicklist.is_empty());
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_shows_entries_and_removes_the_highlighted_one() -> anyhow::Result<()> {
    let (_dir, a, _) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("<A-q>j<A-q>jj<A-q>gg"), None), // lines 0, 1, 3
            (Some("]q]q"), Some(&|app| assert_eq!(at(app), "a.txt:1"))),
            (
                Some("<space>q"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(
                        rows.contains("a.txt:1") && rows.contains("a.txt:4"),
                        "{rows}"
                    );
                    let marked = rows
                        .lines()
                        .find(|row| row.contains("a.txt:2"))
                        .unwrap_or_default();
                    assert!(marked.contains(" * "), "current entry is marked:\n{rows}");
                    let other = rows
                        .lines()
                        .find(|row| row.contains("a.txt:4"))
                        .unwrap_or_default();
                    assert!(!other.contains(" * "), "{rows}");
                }),
            ),
            // remove the current entry (the second row); the picker stays open
            (Some("<C-n><C-x>"), None),
            (
                None,
                Some(&|app| {
                    assert_eq!(app.editor.quicklist.len(), 2);
                    let rows = app.rendered_rows().join("\n");
                    assert!(
                        rows.contains("Quicklist"),
                        "picker should stay open:\n{rows}"
                    );
                    assert!(!rows.contains("a.txt:2"), "{rows}");
                }),
            ),
            (Some("<esc>"), None),
            // the cursor stayed where it was and `]q` visits the entry that followed the
            // removed one
            (Some("]q"), Some(&|app| assert_eq!(at(app), "a.txt:3"))),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_selection_opens_the_entry() -> anyhow::Result<()> {
    let (_dir, a, b) = two_files()?;
    let mut app = helpers::AppBuilder::new().with_file(&a, None).build()?;
    let open_b = format!(":o {}<ret>", b.display());
    test_key_sequences(
        &mut app,
        vec![
            (Some("jj<A-q>"), None), // b? no: a:2
            (Some(open_b.as_str()), None),
            (
                Some("<space>q<ret>"),
                Some(&|app| {
                    assert_eq!(at(app), "a.txt:2");
                    assert_eq!(app.editor.quicklist.current(), Some(0));
                }),
            ),
        ],
        false,
    )
    .await
}
