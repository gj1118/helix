use super::*;

use helix_term::application::Application;
use helix_term::config::{Config, ConfigLoadError};

fn config(toml: &str) -> Config {
    Config::load(Ok(&toml.to_owned()), Err(ConfigLoadError::default())).unwrap()
}

fn app(toml: &str, files: &[&std::path::Path]) -> anyhow::Result<Application> {
    let mut builder = helpers::AppBuilder::new().with_config(config(toml));
    for file in files {
        builder = builder.with_file(file, None);
    }
    builder.build()
}

/// The text of the command line prompt, e.g. `:abc`.
fn prompt_line(app: &Application) -> String {
    app.rendered_rows()
        .into_iter()
        .rev()
        .find(|row| row.trim_start().starts_with(':'))
        .map(|row| row.trim().to_owned())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_key_can_be_bound_to_another_action() -> anyhow::Result<()> {
    let mut app = app("[keys.prompt]\nC-o = \"delete_word_back\"\n", &[])?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":abc def<C-o>"),
                Some(&|app| assert_eq!(prompt_line(app), ":abc", "{}", prompt_line(app))),
            ),
            // the default key of the action keeps working
            (
                Some("<C-w>"),
                Some(&|app| assert!(!prompt_line(app).contains("abc"), "{}", prompt_line(app))),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_op_takes_a_key_away_from_a_prompt() -> anyhow::Result<()> {
    let mut app = app("[keys.prompt]\nC-w = \"no_op\"\n", &[])?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":abc def<C-w>"),
                Some(&|app| assert_eq!(prompt_line(app), ":abc def", "{}", prompt_line(app))),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

/// The index of the selected row of the picker on the screen.
fn selected_row(app: &Application) -> usize {
    app.rendered_rows()
        .iter()
        .position(|row| row.contains("│>"))
        .unwrap_or_else(|| panic!("no selection:\n{}", app.rendered_rows().join("\n")))
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_keys_follow_the_user_table() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let files: Vec<_> = ["a.txt", "b.txt", "c.txt"]
        .iter()
        .map(|name| {
            let path = dir.path().join(name);
            std::fs::write(&path, name).unwrap();
            path
        })
        .collect();
    let mut app = app(
        "[keys.picker]\nC-j = \"move_down\"\nC-k = \"move_up\"\nC-p = \"no_op\"\n",
        &files.iter().map(|p| p.as_path()).collect::<Vec<_>>(),
    )?;

    let start = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (a, b, c) = (start.clone(), start.clone(), start.clone());
    use std::sync::atomic::Ordering::SeqCst;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>b"),
                Some(&move |app| a.store(selected_row(app), SeqCst)),
            ),
            // the bound key moves like the key of the action
            (
                Some("<C-j>"),
                Some(&move |app| assert_eq!(selected_row(app), b.load(SeqCst) + 1)),
            ),
            // `C-p` is taken away: it no longer moves up (and does not edit the query)
            (
                Some("<C-p>"),
                Some(&move |app| assert_eq!(selected_row(app), c.load(SeqCst) + 1)),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}
