//! The location list of each window (Neovim's `:lopen`): filled by `:ladd`, a picker's
//! `Alt-q` or `:ldiagnostics`, walked with `]L` / `[L`, shown with `Space Alt-q` / `:lopen`.

use super::*;

use helix_term::application::Application;

fn cursor_line(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    doc.selection(view.id).primary().cursor_line(text)
}

fn location_count(app: &Application) -> usize {
    let (view, _) = helix_view::current_ref!(app.editor);
    view.location_list.len()
}

#[tokio::test(flavor = "multi_thread")]
async fn each_window_has_a_location_list_of_its_own() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("zero\none\ntwo\nthree\nfour\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            // lines 1 and 3 in this window's list; the quicklist stays empty
            (
                Some("j:ladd<ret>jj:ladd<ret>"),
                Some(&|app| {
                    assert_eq!(location_count(app), 2_usize);
                    assert!(app.editor.quicklist.is_empty());
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(status.as_deref(), Some("Added to the location list (2)"));
                }),
            ),
            // `]L` from the top: line 1, then 3, and around to 1; `[L` back to 3
            (
                Some("gg]L"),
                Some(&|app| assert_eq!(cursor_line(app), 1_usize)),
            ),
            (
                Some("]L"),
                Some(&|app| {
                    assert_eq!(cursor_line(app), 3_usize);
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(status.as_deref(), Some("Location list 2/2"));
                }),
            ),
            (
                Some("]L[L"),
                Some(&|app| assert_eq!(cursor_line(app), 3_usize)),
            ),
            // a split starts with a copy; clearing it leaves the other window's list alone
            (
                Some(":vsplit<ret>"),
                Some(&|app| assert_eq!(location_count(app), 2_usize)),
            ),
            (
                Some(":lclear<ret>"),
                Some(&|app| assert_eq!(location_count(app), 0_usize)),
            ),
            (
                Some("]L"),
                Some(&|app| {
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert!(
                        status.as_deref().is_some_and(
                            |s| s.starts_with("The location list of this window is empty")
                        ),
                        "{status:?}"
                    );
                }),
            ),
            (
                Some("<C-w>w"),
                Some(&|app| assert_eq!(location_count(app), 2_usize)),
            ),
            // (one window again, for the test to end)
            (Some("<C-w>o"), None),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// `Alt-q` in a picker puts its locations in the window's location list (here the quicklist
/// picker's); `Space Alt-q` shows it, and Enter goes to an entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_picker_fills_the_location_list_with_alt_q() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("zero\none\ntwo\nthree\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("j<A-q>jj<A-q>gg"), None),
            (
                Some("<space>q<A-q><esc>"),
                Some(&|app| {
                    assert_eq!(location_count(app), 2_usize);
                    assert_eq!(app.editor.quicklist.len(), 2_usize);
                }),
            ),
            (
                Some("<space><A-q>"),
                Some(&|app| {
                    let screen = app.rendered_rows().join("\n");
                    assert!(screen.contains("Location list"), "{screen}");
                }),
            ),
            (
                Some("<down><ret>"),
                Some(&|app| assert_eq!(cursor_line(app), 3_usize)),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// `:ldiagnostics`: the buffer's diagnostics, in order, into the window's location list;
/// each entry selects the diagnostic's range.
#[tokio::test(flavor = "multi_thread")]
async fn ldiagnostics_lists_the_diagnostics_of_the_buffer() -> anyhow::Result<()> {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, DiagnosticTag, Range as DiagnosticRange, Severity,
    };
    use helix_lsp::LanguageServerId;

    let file = helpers::temp_file_with_contents("one two\nthree\nfour\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    let provider = DiagnosticProvider::Lsp {
        server_id: LanguageServerId::default(),
        identifier: None,
    };
    let diagnostic = |start, end, line, message: &str, severity| Diagnostic {
        range: DiagnosticRange { start, end },
        ends_at_word: true,
        starts_at_word: true,
        zero_width: false,
        line,
        message: message.into(),
        severity: Some(severity),
        code: None,
        provider: provider.clone(),
        tags: Vec::<DiagnosticTag>::new(),
        source: None,
        data: None,
    };
    {
        let (_view, doc) = helix_view::current!(app.editor);
        // given out of order
        doc.replace_diagnostics(
            [
                diagnostic(8, 13, 1, "later", Severity::Warning),
                diagnostic(4, 7, 0, "first\nmore", Severity::Error),
            ],
            &[],
            None,
        );
    }
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":ldiagnostics<ret>"),
                Some(&|app| {
                    let (view, _) = helix_view::current_ref!(app.editor);
                    let texts: Vec<&str> = view
                        .location_list
                        .entries()
                        .iter()
                        .map(|entry| entry.text.as_str())
                        .collect();
                    assert_eq!(texts, vec!["error: first", "warning: later"]);
                }),
            ),
            // the first entry selects "two"
            (
                Some("]L"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let selected = doc
                        .selection(view.id)
                        .primary()
                        .fragment(doc.text().slice(..))
                        .to_string();
                    assert_eq!(selected, "two".to_string());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
