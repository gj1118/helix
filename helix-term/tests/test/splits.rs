use super::*;

use helix_stdx::path;
use helix_term::application::Application;

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_all() -> anyhow::Result<()> {
    let mut file1 = tempfile::NamedTempFile::new()?;
    let mut file2 = tempfile::NamedTempFile::new()?;
    let mut file3 = tempfile::NamedTempFile::new()?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file1.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&format!(
                    "ihello1<esc>:sp<ret>:o {}<ret>ihello2<esc>:sp<ret>:o {}<ret>ihello3<esc>",
                    file2.path().to_string_lossy(),
                    file3.path().to_string_lossy()
                )),
                Some(&|app| {
                    let docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(3, docs.len());

                    let doc1 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == path::normalize(file1.path()))
                        .unwrap();

                    assert_eq!("hello1", doc1.text().to_string());

                    let doc2 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == path::normalize(file2.path()))
                        .unwrap();

                    assert_eq!("hello2", doc2.text().to_string());

                    let doc3 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == path::normalize(file3.path()))
                        .unwrap();

                    assert_eq!("hello3", doc3.text().to_string());

                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(3, app.editor.tree.views().count());
                }),
            ),
            (
                Some(":wqa<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(0, app.editor.tree.views().count());
                }),
            ),
        ],
        true,
    )
    .await?;

    helpers::assert_file_has_content(&mut file1, &LineFeedHandling::Native.apply("hello1"))?;
    helpers::assert_file_has_content(&mut file2, &LineFeedHandling::Native.apply("hello2"))?;
    helpers::assert_file_has_content(&mut file3, &LineFeedHandling::Native.apply("hello3"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_same_file() -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("O<esc>ihello<esc>:sp<ret>ogoodbye<esc>"),
                Some(&|app| {
                    assert_eq!(2, app.editor.tree.views().count());
                    helpers::assert_status_not_error(&app.editor);

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(doc.is_modified());
                }),
            ),
            (
                Some(":wq<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(1, app.editor.tree.views().count());

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(!doc.is_modified());
                }),
            ),
        ],
        false,
    )
    .await?;

    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("hello\ngoodbye"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_changes_in_splits_apply_to_all_views() -> anyhow::Result<()> {
    // See <https://github.com/helix-editor/helix/issues/4732>.
    // Transactions must be applied to any view that has the changed document open.
    // This sequence would panic since the jumplist entry would be modified in one
    // window but not the other. Attempting to update the changelist in the other
    // window would cause a panic since it would point outside of the document.

    // The key sequence here:
    // * <C-w>v       Create a vertical split of the current buffer.
    //                Both views look at the same doc.
    // * [<space>     Add a line ending to the beginning of the document.
    //                The cursor is now at line 2 in window 2.
    // * <C-s>        Save that selection to the jumplist in window 2.
    // * <C-w>w       Switch to window 1.
    // * kd           Delete line 1 in window 1.
    // * <C-w>q       Close window 1, focusing window 2.
    // * d            Delete line 1 in window 2.
    //
    // This panicked in the past because the jumplist entry on line 2 of window 2
    // was not updated and after the `kd` step, pointed outside of the document.
    test((
        "#[|]#",
        "<C-w>v[<space><C-s><C-w>wkd<C-w>qd",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // Transactions are applied to the views for windows lazily when they are focused.
    // This case panics if the transactions and inversions are not applied in the
    // correct order as we switch between windows.
    test((
        "#[|]#",
        "[<space>[<space>[<space><C-w>vuuu<C-w>wUUU<C-w>quuu",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // See <https://github.com/helix-editor/helix/issues/4957>.
    // This sequence undoes part of the history and then adds new changes, creating a
    // new branch in the history tree. `View::sync_changes` applies transactions down
    // and up to the lowest common ancestor in the path between old and new revision
    // numbers. If we apply these up/down transactions in the wrong order, this case
    // panics.
    // The key sequence:
    // * 3[<space>    Create three empty lines so we are at the end of the document.
    // * <C-w>v<C-s>  Create a split and save that point at the end of the document
    //                in the jumplist.
    // * <C-w>w       Switch back to the first window.
    // * uu           Undo twice (not three times which would bring us back to the
    //                root of the tree).
    // * 3[<space>    Create three empty lines. Now the end of the document is past
    //                where it was on step 1.
    // * <C-w>q       Close window 1, focusing window 2 and causing a sync. This step
    //                panics if we don't apply in the right order.
    // * %d           Clean up the buffer.
    test((
        "#[|]#",
        "3[<space><C-w>v<C-s><C-w>wuu3[<space><C-w>q%d",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_changes_in_splits_jumplist_sync() -> anyhow::Result<()> {
    // See <https://github.com/helix-editor/helix/issues/9833>
    // When jumping backwards (<C-o>) switches between two documents, we need to
    // ensure that the current view has been synced with all changes to the
    // document that occurred since the last time the view focused this document.
    // If the view isn't synced then this case panics since we try to form a
    // selection on "test" (which was deleted in the other view).
    test((
        "#[test|]#",
        "<C-w>sgf<C-w>wd<C-w>w<C-o><C-w>qd",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reload_all_with_split_jumplist() -> anyhow::Result<()> {
    // See reproduction from <https://github.com/helix-editor/helix/issues/9830>
    //
    // The key sequence:
    // * <C-w>s   Horizontal split: two views on the same document.
    // * ]<space> Add an empty line below, growing the document.
    // * %        Select the whole document.
    // * 2G       Go to line 2. `goto_line` calls `push_jump`, recording a jump
    //            whose selection is valid at the *current* (grown) revision.
    // * ms/      Surround-add `/`, growing the document again.
    // * :rla     reload-all: re-reads the file from disk (shrinking the buffer
    //            back to its original contents) but only syncs the first view of
    //            each document, leaving the other split's `doc_revisions` stale.
    // * %J       Select-all and join, forcing a sync of the stale view.
    //
    // On the unfixed code the jumplist entry recorded by `2G` is left ahead of
    // the stale view's `doc_revisions`; once that view is synced, the entry is
    // mapped through a changeset whose pre-image predates it and
    // `ChangeSet::update_positions` panics.
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new()?;
    // `:reload-all` re-reads from disk, so the file must have on-disk contents
    // for the reload to shrink the (grown) buffer back down.
    file.write_all(b"line1\nline2\nline3\n")?;
    file.flush()?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    test_key_sequence(
        &mut app,
        // The trailing `<C-w>q` closes the split so a single window remains for
        // the harness's automatic `:q!` teardown. It also exercises the sync
        // that runs when a window is closed.
        Some("<C-w>s]<space>%2Gms/:rla<ret>%J<C-w>q"),
        Some(&|app| {
            helpers::assert_status_not_error(&app.editor);
        }),
        false,
    )
    .await?;

    Ok(())
}

/// Sends `keys` to the application and runs its event loop until it is idle.
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

/// Every terminal size, down to nothing, renders without panicking (debug builds check for
/// arithmetic overflow), with splits, the buffer line, breadcrumbs, rulers, folds and the
/// cursor column all on.
#[tokio::test(flavor = "multi_thread")]
async fn tiny_terminals_render_without_panicking() -> anyhow::Result<()> {
    use helix_view::editor::{BufferLineRenderMode, LineNumber};

    let mut text = String::new();
    for i in 0..60 {
        text.push_str(&format!(
            "\tline {i} 日本語のテキスト {}\n",
            "x".repeat(i * 7)
        ));
    }
    let file = helpers::temp_file_with_contents(&text)?;
    let other = helpers::temp_file_with_contents("other\n")?;

    let mut config = helpers::test_config();
    config.editor.bufferline.render_mode = BufferLineRenderMode::Always;
    config.editor.breadcrumb.enable = true;
    config.editor.rulers = vec![1, 4, 30];
    config.editor.cursorline = true;
    config.editor.cursorcolumn = true;
    config.editor.line_number = LineNumber::Relative;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    send_keys(
        &mut app,
        &format!(
            ":o {}<ret>:vs<ret>:bp<ret>:vs<ret>:sp<ret>10ggxxxx:fold --selection<ret>40gg$",
            other.path().to_string_lossy()
        ),
    )
    .await?;
    assert_eq!(app.editor.tree.views().count(), 4);

    let sizes = [
        (0, 0),
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 2),
        (2, 4),
        (5, 5),
        (7, 3),
        (3, 40),
        (40, 3),
        (13, 9),
        (200, 2),
        (2, 200),
        (80, 24),
    ];
    for (width, height) in sizes {
        app.resize_terminal(width, height).await;
        // Splits squeezed to nothing must still lie within the screen.
        for (view, _) in app.editor.tree.views() {
            assert!(
                view.area.right() <= width && view.area.bottom() <= height,
                "{width}x{height}: view area {:?} sticks out",
                view.area
            );
        }
        // Moving and editing redraws at this size too.
        send_keys(&mut app, "kl<C-w>w").await?;
    }

    send_keys(&mut app, ":qa!<ret>").await?;
    Ok(())
}

/// End of line diagnostics follow the text when the view is scrolled horizontally.
#[tokio::test(flavor = "multi_thread")]
async fn end_of_line_diagnostics_scroll_horizontally() -> anyhow::Result<()> {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity,
    };
    use helix_view::annotations::diagnostics::DiagnosticFilter;

    let long = "a".repeat(100);
    let visible = "v".repeat(60);
    let text = format!("{long}\n{visible}\nshort\n");
    let file = helpers::temp_file_with_contents(&text)?;

    let mut config = helpers::test_config();
    config.editor.end_of_line_diagnostics = DiagnosticFilter::Enable(Severity::Hint);
    config.editor.inline_diagnostics.cursor_line = DiagnosticFilter::Disable;
    config.editor.inline_diagnostics.other_lines = DiagnosticFilter::Disable;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let diagnostic = |start: usize, end: usize, line: usize, message: &str| Diagnostic {
        range: Range { start, end },
        ends_at_word: false,
        starts_at_word: false,
        zero_width: false,
        line,
        message: message.to_string(),
        severity: Some(Severity::Error),
        code: None,
        provider: DiagnosticProvider::Lsp {
            server_id: LanguageServerId::default(),
            identifier: None,
        },
        tags: Vec::new(),
        source: None,
        data: None,
    };
    {
        let doc = helix_view::doc_mut!(app.editor);
        let visible_start = 101;
        let short_start = visible_start + 61;
        doc.replace_diagnostics(
            [
                // starts on screen: the diagnostics scrolled off to the left aren't drawn
                diagnostic(visible_start + 50, visible_start + 53, 1, "DIAGA"),
                diagnostic(short_start + 1, short_start + 3, 2, "DIAGB"),
            ],
            &[],
            None,
        );
    }
    app.resize_terminal(80, 10).await;
    // the end of the first line: the view scrolls right, past the end of `short`
    send_keys(&mut app, "gl").await?;

    let (view, doc) = helix_view::current_ref!(app.editor);
    let offset = doc.view_offset(view.id).horizontal_offset;
    let text_x = view.inner_area(doc).x as usize;
    assert!(offset > "short".len() && offset < visible.len(), "{offset}");

    let rows = app.rendered_rows();
    // past the line and its newline, a column, the icon and a space, then the message
    let expected = text_x + visible.len() + 1 + 1 + 2 - offset;
    let found = rows[1]
        .find("DIAGA")
        .map(|byte| rows[1][..byte].chars().count());
    assert_eq!(found, Some(expected), "{:?}", rows[1]);
    assert!(!rows[2].contains("DIAGB"), "{:?}", rows[2]);

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// With inline diagnostics on the cursor line and end-of-line diagnostics both disabled, the
/// diagnostic under the cursor is shown in the top right corner of the view (as upstream does),
/// but not when another way of showing it is on, nor where a plugin draws annotations.
#[tokio::test(flavor = "multi_thread")]
async fn diagnostics_are_shown_in_the_corner_when_inline_and_end_of_line_are_off(
) -> anyhow::Result<()> {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity,
    };
    use helix_view::annotations::diagnostics::DiagnosticFilter;
    use helix_view::document::PluginAnnotation;

    let text = "let first = 1;\nlet second = 2;\n";
    let file = helpers::temp_file_with_contents(text)?;
    let hint = Diagnostic {
        range: Range { start: 4, end: 9 },
        ends_at_word: false,
        starts_at_word: false,
        zero_width: false,
        line: 0,
        message: "CORNERHINT".to_string(),
        severity: Some(Severity::Hint),
        code: None,
        provider: DiagnosticProvider::Lsp {
            server_id: LanguageServerId::default(),
            identifier: None,
        },
        tags: Vec::new(),
        source: None,
        data: None,
    };

    let in_corner = |cursor_line: DiagnosticFilter, end_of_line: DiagnosticFilter, plugin: bool| {
        let file = &file;
        let hint = hint.clone();
        async move {
            let mut config = helpers::test_config();
            config.editor.end_of_line_diagnostics = end_of_line;
            config.editor.inline_diagnostics.cursor_line = cursor_line;
            config.editor.inline_diagnostics.other_lines = DiagnosticFilter::Disable;
            // notification popups sit in the same corner
            config.editor.notifications.enable = false;
            let mut app = helpers::AppBuilder::new()
                .with_file(file.path(), None)
                .with_config(config)
                .build()?;
            {
                let (view, doc) = helix_view::current!(app.editor);
                doc.replace_diagnostics([hint], &[], None);
                if plugin {
                    doc.plugin_annotations.insert(
                        view.id,
                        vec![PluginAnnotation {
                            char_idx: 0,
                            text: "PANEL".to_string(),
                            style: None,
                            fg: None,
                            bg: None,
                            offset: 1,
                            is_line: false,
                            virt_line_idx: None,
                            dropped_text: None,
                        }],
                    );
                }
            }
            app.resize_terminal(80, 10).await;
            // onto the diagnostic
            send_keys(&mut app, "wl").await?;
            let rows = app.rendered_rows();
            // the second row of the view, right aligned
            let shown = rows[1].trim_end().ends_with("CORNERHINT");
            let anywhere = rows.iter().any(|row| row.contains("CORNERHINT"));
            send_keys(&mut app, ":q!<ret>").await?;
            anyhow::Ok((shown, anywhere, rows))
        }
    };

    let (shown, _, rows) =
        in_corner(DiagnosticFilter::Disable, DiagnosticFilter::Disable, false).await?;
    assert!(shown, "{rows:#?}");

    // end-of-line diagnostics show it (at the end of its line, not in the corner)
    let (_, anywhere, rows) = in_corner(
        DiagnosticFilter::Disable,
        DiagnosticFilter::Enable(Severity::Hint),
        false,
    )
    .await?;
    let in_corner_too = rows[1].trim_end().ends_with("CORNERHINT") && !rows[1].contains("second");
    assert!(anywhere && !in_corner_too, "{rows:#?}");

    // inline diagnostics on the cursor line are on: no corner
    let (shown, _, rows) = in_corner(
        DiagnosticFilter::Enable(Severity::Warning),
        DiagnosticFilter::Disable,
        false,
    )
    .await?;
    assert!(!shown, "{rows:#?}");

    // a plugin draws its own panel: no corner
    let (_, anywhere, rows) =
        in_corner(DiagnosticFilter::Disable, DiagnosticFilter::Disable, true).await?;
    assert!(!anywhere, "{rows:#?}");
    Ok(())
}
