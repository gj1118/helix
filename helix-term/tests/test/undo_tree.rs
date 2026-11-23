use super::*;

use helix_term::application::Application;

fn buffer_text(app: &Application) -> String {
    helix_view::current_ref!(app.editor).1.text().to_string()
}

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

/// The text rows of the picker's preview (the frame above the list, the screen being taller
/// than wide).
fn preview_rows(app: &Application) -> Vec<String> {
    let rows = app.rendered_rows();
    // the first frame's top border, then its rows up to its bottom border
    let top = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.trim_start().starts_with('🭽'))
        .map(|(i, _)| i)
        .next();
    let Some(top) = top else {
        return Vec::new();
    };
    rows[top + 1..]
        .iter()
        .take_while(|row| !row.trim_start().starts_with('🭼'))
        .map(|row| {
            row.trim()
                .trim_start_matches('▏')
                .trim_end_matches('▕')
                .trim()
                .to_string()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

fn tree_is_open(app: &Application) -> bool {
    screen(app).contains("Undo tree ·")
}

/// Two edits, an undo and a third edit: the second one is on a branch. The picker lists every
/// revision in the order of the tree, the cursor on the one shown; the preview shows the text
/// of the revision under the cursor, without changing the buffer; Esc changes nothing, Enter
/// turns the buffer into that revision.
#[tokio::test(flavor = "multi_thread")]
async fn the_undo_tree_picker_previews_and_jumps_to_revisions() -> anyhow::Result<()> {
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
                    let text = screen(app);
                    assert!(text.contains("Undo tree · 4 revisions"), "{text}");
                    assert!(text.contains("original"), "{text}");
                    // the abandoned branch is drawn indented; ● the revision shown
                    assert!(text.contains("├─○"), "{text}");
                    let shown = text
                        .lines()
                        .find(|row| row.contains("●"))
                        .unwrap_or_default();
                    assert!(shown.contains("#3 +1 line"), "{text}");
                    assert!(text.contains("original saved"), "{text}");
                }),
            ),
            // up: the branch with "three", previewed; the buffer stays as it is
            (
                Some("<up>"),
                Some(&|app| {
                    // the preview: the revision's whole text, a row per line
                    let rows = preview_rows(app);
                    assert_eq!(
                        rows,
                        vec!["one".to_string(), "two".to_string(), "three".to_string()]
                    );
                    assert_eq!(buffer_text(app), "one\ntwo\nfour\n".to_string());
                }),
            ),
            // Esc: closed, nothing changed
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(!tree_is_open(app));
                    assert_eq!(buffer_text(app), "one\ntwo\nfour\n".to_string());
                }),
            ),
            // Enter turns the buffer into the revision under the cursor; the history keeps
            // every branch (the command opens the picker once the key handling is done)
            (
                Some(":undo-tree<ret>"),
                Some(&|app| assert!(tree_is_open(app))),
            ),
            (
                Some("<up><ret>"),
                Some(&|app| {
                    assert!(!tree_is_open(app));
                    assert_eq!(buffer_text(app), "one\ntwo\nthree\n".to_string());
                }),
            ),
            (
                Some("u"),
                Some(&|app| assert_eq!(buffer_text(app), "one\ntwo\n".to_string())),
            ),
            // typing filters the revisions by what they changed
            (Some("<space>u"), Some(&|app| assert!(tree_is_open(app)))),
            (
                Some("original<ret>"),
                Some(&|app| assert_eq!(buffer_text(app), "one\n".to_string())),
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
            (Some("ihello<esc>"), None),
            (Some(":undo-tree<ret>"), None),
            (
                None,
                Some(&|app| {
                    let text = screen(app);
                    assert!(text.contains("Undo tree · 2 revisions"), "{text}");
                    let shown = text
                        .lines()
                        .find(|row| row.contains("●"))
                        .unwrap_or_default();
                    assert!(shown.contains("#1 +5"), "{text}");
                }),
            ),
            (
                Some("<up><ret>"),
                Some(&|app| assert_eq!(buffer_text(app), "".to_string())),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// With the quickfix panel on, the undo tree is in the panel: `j`/`k` move, Enter jumps.
#[tokio::test(flavor = "multi_thread")]
async fn the_undo_tree_goes_to_the_quickfix_panel_when_it_is_on() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut config = helpers::test_config();
    config.editor.quickfix.enable = true;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("otwo<esc>othree<esc>"), None),
            (
                Some("<space>u"),
                Some(&|app| {
                    let text = screen(app);
                    assert!(text.contains("#2 +1 line"), "{text}");
                    assert!(helix_term::ui::quickfix::focused(&app.editor));
                }),
            ),
            // the preview takes the windows' place above the panel: revision #1's text
            (
                Some("k"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let top: Vec<&str> = rows.iter().take(3).map(|row| row.trim()).collect();
                    assert_eq!(top, vec!["one", "two", ""], "{}", screen(app));
                }),
            ),
            (
                Some("k<ret>"),
                Some(&|app| assert_eq!(buffer_text(app), "one\n".to_string())),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
