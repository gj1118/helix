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

/// The file names of the buffers the bufferline lists.
fn listed(app: &Application) -> Vec<String> {
    app.editor
        .bufferline_documents()
        .map(|doc| {
            doc.path()
                .and_then(|path| path.file_name())
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        })
        .collect()
}

fn bufferline_config(scope: &str) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(&format!(
        "[editor.bufferline]\nrender-mode = \"always\"\nscope = \"{scope}\"\n"
    ))
    .unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// Each tab page's bufferline lists its own buffers (`scope = "tab"`, the default): two
/// files opened in the first tab, two in the second, as in a review in its own tab. `gn`
/// and the buffer picker go through the same buffers; closing a tab gives its buffers to
/// the tab shown; `scope = "global"` lists them all.
#[tokio::test(flavor = "multi_thread")]
async fn each_tab_lists_its_own_buffers() -> anyhow::Result<()> {
    let files: Vec<_> = (0..4)
        .map(|i| helpers::temp_file_with_contents(format!("file {i}\n")))
        .collect::<Result<_, _>>()?;
    let names: Vec<String> = files
        .iter()
        .map(|file| {
            file.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let path = |i: usize| files[i].path().display().to_string();
    let first_tab = vec![names[0].clone(), names[1].clone()];
    let second_tab = vec![names[2].clone(), names[3].clone()];
    let all = names.clone();

    // the default
    assert_eq!(
        helix_view::editor::BufferLineConfig::default().scope,
        helix_view::editor::BufferLineScope::Tab
    );

    let mut app = helpers::AppBuilder::new()
        .with_config(bufferline_config("tab"))
        .with_file(files[0].path(), None)
        .build()?;
    let open_first = format!(":open {}<ret>", path(1));
    let open_second = format!(":tab-new {}<ret>:open {}<ret>", path(2), path(3));
    let (t1, t2, t1b, t2b, everything) = (
        first_tab.clone(),
        second_tab.clone(),
        first_tab.clone(),
        second_tab.clone(),
        all.clone(),
    );
    let shown = |app: &Application| app.editor.tab_label(app.editor.tree.active_tab());
    let (n2, n3) = (names[2].clone(), names[3].clone());
    test_key_sequences(
        &mut app,
        vec![
            // one tab: every buffer, as before
            (Some(&open_first), Some(&|app| assert_eq!(listed(app), t1))),
            // the second tab lists only its two buffers, on the screen too
            (
                Some(&open_second),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 1_usize));
                    assert_eq!(listed(app), t2);
                    let bufferline = app.rendered_rows()[1].clone();
                    assert!(bufferline.contains(&t2[0]), "{bufferline}");
                    assert!(bufferline.contains(&t2[1]), "{bufferline}");
                    assert!(!bufferline.contains(&t1[0]), "{bufferline}");
                    assert!(!bufferline.contains(&t1[1]), "{bufferline}");
                    // every buffer is still open
                    assert_eq!(app.editor.documents.len(), 4_usize);
                }),
            ),
            // `gn` goes around the tab's buffers only: from the fourth file to the third,
            // and back
            (Some("gn"), Some(&|app| assert_eq!(shown(app), n2))),
            (Some("gn"), Some(&|app| assert_eq!(shown(app), n3))),
            // the first tab still lists its own
            (
                Some("<C-w>["),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 0_usize));
                    assert_eq!(listed(app), t1b);
                }),
            ),
            // closing the second tab: its buffers join the first, nothing goes missing
            (
                Some("<C-w>]:tab-close<ret>"),
                Some(&|app| {
                    assert_eq!(tabs(app), (1_usize, 0_usize));
                    assert_eq!(listed(app), everything);
                }),
            ),
            // a new tab of a buffer already open lists just it
            (
                Some("<C-w>T"),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 1_usize));
                    assert_eq!(listed(app).len(), 1_usize);
                    let _ = &t2b;
                }),
            ),
            (Some(":tab-only<ret>"), None),
        ],
        false,
    )
    .await?;

    // `scope = "global"`: every tab lists every buffer
    let mut app = helpers::AppBuilder::new()
        .with_config(bufferline_config("global"))
        .with_file(files[0].path(), None)
        .build()?;
    let keys = format!(
        ":open {}<ret>:tab-new {}<ret>:open {}<ret>",
        path(1),
        path(2),
        path(3)
    );
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&keys),
                Some(&|app| {
                    assert_eq!(tabs(app), (2_usize, 1_usize));
                    assert_eq!(listed(app), all);
                }),
            ),
            (Some(":tab-only<ret>"), None),
        ],
        false,
    )
    .await?;
    Ok(())
}
