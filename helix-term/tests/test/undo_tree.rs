use super::*;

use helix_term::application::Application;
use helix_term::ui::undo_tree::{UndoTree, ID};

fn buffer_text(app: &Application) -> String {
    helix_view::current_ref!(app.editor).1.text().to_string()
}

fn tree_is_open(app: &Application) -> bool {
    app.compositor.get_id::<UndoTree>(ID).is_some()
}

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

/// Two edits, an undo and a third edit: the second one is on a branch. Moving up the tree
/// shows each revision in the buffer, Esc goes back, Enter keeps the one under the cursor.
#[tokio::test(flavor = "multi_thread")]
async fn moving_through_the_undo_tree_shows_each_revision() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("otwo<esc>othree<esc>uofour<esc>"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\nfour\n".to_string())),
            ),
            (
                Some("<space>u"),
                Some(&|app| {
                    assert!(tree_is_open(app));
                    let tree = app.compositor.get_id::<UndoTree>(ID).unwrap();
                    assert_eq!(tree.selected_revision(), Some(3_usize));
                    let text = screen(app);
                    assert!(text.contains("Undo tree · 4 revisions"), "{text}");
                    assert!(text.contains("original"), "{text}");
                    // the abandoned branch is drawn indented
                    assert!(text.contains("├─○ #2 +1 line"), "{text}");
                    assert!(text.contains("● #3 +1 line"), "{text}");
                }),
            ),
            // up: the branch with "three", then the first edit
            (
                Some("k"),
                Some(&|app| {
                    assert_eq!(buffer_text(app), "one\ntwo\nthree\n".to_string());
                    assert!(screen(app).contains("● #2"), "{}", screen(app));
                }),
            ),
            (
                Some("k"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\n".to_string())),
            ),
            (
                Some("g"),
                Some(&|app| assert_eq!(buffer_text(app), "one\n".to_string())),
            ),
            // Esc: back to where the tree opened, and closed
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(!tree_is_open(app));
                    assert_eq!(buffer_text(app), "one\ntwo\nfour\n".to_string());
                }),
            ),
            // Enter keeps the revision under the cursor; the history keeps every branch
            (
                Some(":undo-tree<ret>k<ret>"),
                Some(&|app| {
                    assert!(!tree_is_open(app));
                    assert_eq!(buffer_text(app), "one\ntwo\nthree\n".to_string());
                }),
            ),
            (
                Some("u"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\n".to_string())),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// The edit just made is in the tree, and the first row is the text before any edit.
#[tokio::test(flavor = "multi_thread")]
async fn the_tree_has_the_last_edit_and_the_original() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("ihello<esc>:undo-tree<ret>"), None),
            (
                None,
                Some(&|app| {
                    let tree = app.compositor.get_id::<UndoTree>(ID).unwrap();
                    assert_eq!(tree.selected_revision(), Some(1_usize));
                    assert!(screen(app).contains("Undo tree · 2 revisions"));
                }),
            ),
            (
                Some("k<ret>"),
                Some(&|app| assert_eq!(buffer_text(app), "".to_string())),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
