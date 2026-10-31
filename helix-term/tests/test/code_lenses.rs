use super::*;
use helix_term::application::Application;

// A small Python language server (tests/test/fixtures) answers the requests: the first line gets
// a lens with a command ("Run"), the second one a lens that has to be resolved ("2 references").

fn lang_conf(log: &std::path::Path) -> String {
    let server = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/test/fixtures/fake_code_lens_ls.py"
    );
    format!(
        r#"
        [language-server.fake-lens]
        command = "python3"
        args = ["{server}", "{log}"]

        [[language]]
        name = "rust"
        language-servers = ["fake-lens"]
        "#,
        log = log.display()
    )
}

fn config(display_code_lenses: bool) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(&format!(
        r#"
        [editor.lsp]
        display-code-lenses = {display_code_lenses}
        "#
    ))
    .unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

fn log_of(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

#[test]
fn code_lenses_are_off_by_default() {
    assert!(
        !helix_view::editor::Config::default()
            .lsp
            .display_code_lenses
    );
    assert!(!Config::default().editor.lsp.display_code_lenses);
}

#[tokio::test(flavor = "multi_thread")]
async fn lenses_are_shown_resolved_and_run() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
    let log = tempfile::NamedTempFile::new()?;
    let log_path = log.path().to_path_buf();
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config(true))
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf(&log_path))))
        .build()?;

    let run_log = log_path.clone();
    let picker_log = log_path.clone();
    test_key_sequences(
        &mut app,
        vec![
            // idle rounds, to let the language server start up
            (Some("i"), None),
            (Some("<esc>i"), None),
            (Some("<esc>i"), None),
            // typing schedules a request (after a short debounce)
            (Some("fn main() {}<ret>fn other() {}<esc>gg"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (
                Some("i<esc>"),
                Some(&|app| {
                    let doc = helix_view::doc!(app.editor);
                    assert_eq!(doc.code_lenses.len(), 2, "{}", screen(app));
                    assert!(doc.code_lenses_are_current());
                    // the lens that came with a command is on the first line ...
                    let rows = app.rendered_rows();
                    assert!(rows[0].contains("fn main() {}  ▶ Run"), "{}", screen(app));
                }),
            ),
            // ... the other one was resolved because it is on screen
            (
                Some("i<esc>"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    assert!(
                        rows[1].contains("fn other() {}  ▶ 2 references"),
                        "{}",
                        screen(app)
                    );
                }),
            ),
            // the lenses of the cursor line can be run
            (
                Some("<space>A"),
                Some(&|app| {
                    // "Run" is in the menu, next to the label at the end of the line
                    assert_eq!(screen(app).matches("Run").count(), 2, "{}", screen(app));
                }),
            ),
            (
                Some("<ret>"),
                Some(&|app| {
                    // the menu is gone again
                    assert_eq!(screen(app).matches("Run").count(), 1, "{}", screen(app));
                }),
            ),
            (Some("i<esc>"), None),
            (
                Some("i<esc>"),
                Some(&move |_| {
                    let log = log_of(&run_log);
                    assert!(log.contains(&"codeLens".to_string()), "{log:?}");
                    assert!(log.contains(&"resolve".to_string()), "{log:?}");
                    assert!(log.contains(&"execute:fake.run".to_string()), "{log:?}");
                }),
            ),
            // the picker lists the lenses of the whole file ...
            (Some("<space>o"), None),
            (
                Some("<C-n>"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(screen.contains("Code Lenses"), "{screen}");
                    assert!(screen.contains("2 references"), "{screen}");
                    // ... in the frame the other pickers have
                    assert!(screen.contains('┌') && screen.contains('┘'), "{screen}");
                }),
            ),
            // the lens of the first line is the first entry; choosing it runs it again
            (Some("<C-p>"), None),
            (Some("<ret>"), None),
            (Some("i<esc>"), None),
            (
                Some("i<esc>"),
                Some(&move |_| {
                    let runs = log_of(&picker_log)
                        .iter()
                        .filter(|entry| *entry == "execute:fake.run")
                        .count();
                    assert_eq!(runs, 2, "{:?}", log_of(&picker_log));
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_requested_while_the_setting_is_off() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
    let log = tempfile::NamedTempFile::new()?;
    let log_path = log.path().to_path_buf();
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config(false))
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf(&log_path))))
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("i"), None),
            (Some("<esc>i"), None),
            (Some("<esc>i"), None),
            (Some("fn main() {}<ret>fn other() {}<esc>gg"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (Some("i<esc>"), None),
            (
                Some("<space>A"),
                Some(&move |app| {
                    let doc = helix_view::doc!(app.editor);
                    assert!(doc.code_lenses.is_empty());
                    assert!(!screen(app).contains('▶'), "{}", screen(app));
                    assert_eq!(log_of(&log_path), Vec::<String>::new());
                }),
            ),
        ],
        false,
    )
    .await
}
