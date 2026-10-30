use super::*;

use helix_term::config::ConfigLoadError;

fn config(toml: &str) -> Config {
    Config::load(Ok(&toml.to_owned()), Err(ConfigLoadError::default())).unwrap()
}

fn register(app: &helix_term::application::Application, name: char) -> Option<String> {
    app.editor
        .registers
        .read(name, &app.editor)
        .map(|values| values.map(|v| v.to_string()).collect::<Vec<_>>().join("|"))
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_are_expanded_and_missing_ones_are_empty() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands]
            ":say" = ":set-register a %arg{0}"
            ":second" = ":set-register b %arg{1}"
            ":line" = ":goto %arg{0}"
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("iA\nB\nC<esc>"), None),
            // a quoted argument with a space stays one argument
            (
                Some(r#":say "hello world"<ret>"#),
                Some(&|app| assert_eq!(register(app, 'a').as_deref(), Some("hello world"))),
            ),
            // a missing argument is empty
            (
                Some(":second only-one<ret>"),
                Some(&|app| assert_eq!(register(app, 'b').as_deref(), Some(""))),
            ),
            (
                Some(":line 2<ret>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.cursor_line(view.id), 1);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn static_commands_and_macros_run() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands]
            ":down2" = ["move_line_down", "move_line_down"]
            ":shout" = "@ihello <esc>"
            ":mixed" = ["move_line_up", ":set-register a mixed"]
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("iA\nB\nC\nD<esc>gg"), None),
            (
                Some(":down2<ret>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.cursor_line(view.id), 2);
                }),
            ),
            // a macro types its keys after the command line has closed
            (
                Some(":shout<ret>"),
                Some(&|app| {
                    let doc = app.editor.documents().next().unwrap();
                    assert!(doc.text().to_string().contains("hello "), "{}", doc.text());
                }),
            ),
            // static and typable commands in one sequence
            (
                Some(":mixed<ret>"),
                Some(&|app| {
                    assert_eq!(register(app, 'a').as_deref(), Some("mixed"));
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.cursor_line(view.id), 1);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_commands_shadow_builtins_and_carets_reach_them() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands]
            ":goto" = ":set-register a shadowed"
            "0" = ":set-register a zero"
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("iA\nB\nC<esc>gg"), None),
            (
                Some(":goto 3<ret>"),
                Some(&|app| {
                    assert_eq!(register(app, 'a').as_deref(), Some("shadowed"));
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.cursor_line(view.id), 0, "the built-in must not run");
                }),
            ),
            // `:^goto` bypasses the custom command
            (
                Some(":^goto 3<ret>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(doc.cursor_line(view.id), 2);
                }),
            ),
            // a custom command named like a number wins over `:<line>`
            (
                Some(":0<ret>"),
                Some(&|app| assert_eq!(register(app, 'a').as_deref(), Some("zero"))),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_stop_the_sequence_and_name_the_command() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands]
            ":bad" = [":goto nope", ":set-register a ran"]
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![(
            Some(":bad<ret>"),
            Some(&|app| {
                let (message, severity) = app.editor.get_status().unwrap();
                assert!(format!("{severity:?}").contains("Error"), "{message}");
                assert!(message.contains("'bad'"), "{message}");
                // the command after the failing one did not run
                assert_eq!(register(app, 'a'), None);
            }),
        )],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn key_bindings_can_run_custom_commands() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands]
            ":say" = ":set-register a %arg{0}"
            [keys.normal]
            X = ":say from-a-key"
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![(
            Some("X"),
            Some(&|app| assert_eq!(register(app, 'a').as_deref(), Some("from-a-key"))),
        )],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_prompt_completes_and_describes_custom_commands() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            r#"
            [commands.":save-copy"]
            commands = [':write "%arg{0}"']
            desc = "Save a copy under another name"
            accepts = "<path>"
            completer = ":write"
            [commands]
            "secret" = ":goto 1"
            "#,
        ))
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            // while typing the name it is offered ...
            (
                Some(":save-c"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("save-copy"), "{rows}");
                }),
            ),
            // ... and once it is complete its help is shown
            (Some("opy"), None),
            (
                Some(" "),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("Save a copy under another name"), "{rows}");
                    assert!(rows.contains("Arguments: <path>"), "{rows}");
                }),
            ),
            (Some("<esc>:secre"), None),
            (
                Some("t"),
                Some(&|app| {
                    // hidden commands run but are not offered
                    let rows = app.rendered_rows().join("\n");
                    assert!(
                        !rows.contains("secret goto") && rows.matches("secret").count() == 1,
                        "{rows}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
