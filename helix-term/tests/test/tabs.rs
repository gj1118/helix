use super::*;

use helix_term::application::Application;

fn tabs(app: &Application) -> (usize, usize) {
    (app.editor.tree.tab_count(), app.editor.tree.active_tab())
}

fn shown_name(app: &Application) -> String {
    app.editor.tab_label(app.editor.tree.active_tab())
}

fn windows(app: &Application) -> usize {
    app.editor.tree.views().count()
}

/// Tab pages: each has its own windows; the tab line shows them while there are several;
/// closing a tab's last window closes the tab.
#[tokio::test(flavor = "multi_thread")]
async fn tab_pages_have_their_own_windows() -> anyhow::Result<()> {
    let a = helpers::temp_file_with_contents("first\n")?;
    let b = helpers::temp_file_with_contents("second\n")?;
    let a_name = a.path().file_name().unwrap().to_string_lossy().into_owned();
    let b_name = b.path().file_name().unwrap().to_string_lossy().into_owned();
    let mut app = helpers::AppBuilder::new()
        .with_file(a.path(), None)
        .build()?;
    let open_b = format!(":tab-new {}<ret>", b.path().display());
    let (a1, b1, a2, b2) = (
        a_name.clone(),
        b_name.clone(),
        a_name.clone(),
        b_name.clone(),
    );
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&open_b),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 1_usize));
                    assert_eq!(shown_name(app), b1);
                    assert_eq!(windows(app), 1_usize);
                    let top = app.rendered_rows()[0].clone();
                    assert!(top.contains(&format!(" 1 {a1} ")), "{top}");
                    // (the rows are trimmed at the right)
                    assert!(top.contains(&format!(" 2 {b1}")), "{top}");
                }),
            ),
            // back to the first tab, split it: the second tab is untouched
            (
                Some("<C-w>[:vsplit<ret>"),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 0_usize));
                    assert_eq!(shown_name(app), a2);
                    assert_eq!(windows(app), 2_usize);
                    assert_eq!(app.editor.tree.tab_views(1).len(), 1_usize);
                    assert!(app.rendered_rows()[0].contains(" +1 "));
                }),
            ),
            // the second tab's only window closes: the tab goes, the tab line too
            (
                Some("<C-w>]:q<ret>"),
                Some(&|app| {
                    assert_eq!(tabs(app), (1_usize, 0_usize));
                    assert_eq!(windows(app), 2_usize);
                    let top = app.rendered_rows()[0].clone();
                    assert!(!top.contains(&format!(" 2 {b2}")), "{top}");
                }),
            ),
            // the last tab cannot be closed
            (
                Some(":tab-close<ret>"),
                Some(&|app| {
                    assert_eq!(tabs(app), (1_usize, 0_usize));
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(
                        status.as_deref(),
                        Some("'tab-close': This is the only tab page")
                    );
                }),
            ),
            // a scratch tab, the current buffer in a tab, then only the shown one is kept
            (
                Some(":tab-new<ret><C-w>T"),
                Some(&|app| {
                    assert_eq!(tabs(app), (3_usize, 2_usize));
                    assert!(
                        app.rendered_rows()[0].contains(" 2 [scratch] "),
                        "{}",
                        app.rendered_rows()[0]
                    );
                }),
            ),
            (
                Some(":tab-move 1<ret>"),
                Some(&|app| assert_eq!(tabs(app), (3_usize, 0_usize))),
            ),
            (
                Some(":tab-next 3<ret>"),
                Some(&|app| assert_eq!(tabs(app), (3_usize, 2_usize))),
            ),
            (
                Some(":tab-only<ret>"),
                Some(&|app| {
                    assert_eq!(tabs(app), (1_usize, 0_usize));
                    assert_eq!(app.editor.tree.all_views().count(), windows(app));
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// A buffer closed while a tab not shown has it in a window: that window goes (and its tab
/// with it); `:quit-all` closes the windows of every tab.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_buffer_reaches_the_tabs_not_shown() -> anyhow::Result<()> {
    let a = helpers::temp_file_with_contents("first\n")?;
    let b = helpers::temp_file_with_contents("second\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(a.path(), None)
        .build()?;
    let open_b = format!(":tab-new {}<ret>", b.path().display());
    let close_b = format!(":buffer-close {}<ret>", b.path().display());
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&open_b),
                Some(&|app| assert_eq!(tabs(app), (2_usize, 1_usize))),
            ),
            (
                Some("<C-w>["),
                Some(&|app| assert_eq!(tabs(app), (2_usize, 0_usize))),
            ),
            (
                Some(&close_b),
                Some(&|app| {
                    assert_eq!(tabs(app), (1_usize, 0_usize));
                    assert_eq!(app.editor.documents().count(), 1_usize);
                }),
            ),
            (
                Some(&open_b),
                Some(&|app| assert_eq!(tabs(app), (2_usize, 1_usize))),
            ),
            (Some(":quit-all<ret>"), None),
        ],
        true,
    )
    .await?;
    Ok(())
}
