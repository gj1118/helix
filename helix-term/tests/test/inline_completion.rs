use super::*;

// A small Python language server (tests/test/fixtures) answers the requests, so this
// exercises the whole path: request, response conversion, state, commands.
fn lang_conf() -> String {
    let server = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/test/fixtures/fake_inline_ls.py"
    );
    format!(
        r#"
        [language-server.fake-inline]
        command = "python3"
        args = ["{server}"]

        [[language]]
        name = "rust"
        language-servers = ["fake-inline"]
        "#
    )
}

fn config() -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(
        r#"
        [editor.inline-completion]
        auto-trigger = false

        [keys.insert]
        C-g = "inline_completion_trigger"
        A-n = "inline_completion_next"
        A-p = "inline_completion_prev"
        C-y = "inline_completion_accept"
        C-e = "inline_completion_dismiss"
        "#,
    )
    .unwrap();
    let mut config = Config::default();
    config.keys = raw.keys.unwrap().modes;
    config.editor = raw.editor.unwrap().try_into().unwrap();
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_trigger_cycle_accept() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config())
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf())))
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            // idle rounds (on an empty buffer), to let the language server start up
            (Some("i"), None),
            (Some("<esc>i"), None),
            (Some("<esc>i"), None),
            // typing requests nothing while auto-trigger is off
            (
                Some("let x = "),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.inline_completion(view.id).is_none());
                }),
            ),
            // the trigger key asks the server and the first suggestion shows up
            (
                Some("<C-g>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let completion = doc.inline_completion(view.id).expect("a suggestion");
                    assert_eq!(completion.text, "first(suggestion)");
                    // the ghost text is painted behind the cursor
                    assert!(
                        app.rendered_rows()
                            .iter()
                            .any(|row| row.contains("let x = first(suggestion)")),
                        "ghost text not rendered:\n{}",
                        app.rendered_rows().join("\n")
                    );
                    // the document is unchanged until the suggestion is accepted
                    assert_eq!(doc.text().to_string(), "let x = ");
                }),
            ),
            (
                Some("<A-n>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.inline_completion(view.id).unwrap().text, "second");
                }),
            ),
            (
                Some("<A-n>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(
                        doc.inline_completion(view.id).unwrap().text,
                        "first(suggestion)"
                    );
                }),
            ),
            // dismissing hides it without touching the document
            (
                Some("<C-e>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.inline_completion(view.id).is_none());
                    assert_eq!(doc.text().to_string(), "let x = ");
                }),
            ),
            // ask again, pick the second alternative and accept it
            (Some("<C-g>"), None),
            (Some("<A-n>"), None),
            (
                Some("<C-y>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "let x = second");
                    assert!(doc.inline_completion(view.id).is_none());
                    assert_eq!(
                        doc.selection(view.id)
                            .primary()
                            .cursor(doc.text().slice(..)),
                        "let x = second".chars().count()
                    );
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_is_dropped_when_typing() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config())
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf())))
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            // idle rounds (on an empty buffer), to let the language server start up
            (Some("i"), None),
            (Some("<esc>i"), None),
            (Some("<esc>i"), None),
            (Some("ix"), None),
            (
                Some("<C-g>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.inline_completion(view.id).is_some());
                }),
            ),
            (
                Some("y"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.inline_completion(view.id).is_none());
                }),
            ),
        ],
        false,
    )
    .await
}
