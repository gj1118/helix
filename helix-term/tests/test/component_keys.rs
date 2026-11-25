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
        .position(|row| row.contains("▏>"))
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

/// The replacement of a global search is shown with the picker's `toggle_replace`
/// (`Alt-r`), which another key can be given.
#[tokio::test(flavor = "multi_thread")]
async fn the_replace_key_of_global_search_can_be_bound() -> anyhow::Result<()> {
    let mut app = app(
        "[keys.picker]\n\"A-x\" = \"toggle_replace\"\n\"A-r\" = \"no_op\"\n",
        &[],
    )?;
    // (the label of the replacement, at the start of its row in the picker)
    let shown = |app: &Application| {
        app.rendered_rows().iter().any(|row| {
            row.trim_start_matches([' ', '▏', '│'])
                .starts_with("replace:")
        })
    };
    // a pattern that finds nothing, not even itself: no preview to look like the label
    send_keys(&mut app, "<space>/q[z]qzz9").await?;
    assert!(!shown(&app), "{:#?}", app.rendered_rows());
    send_keys(&mut app, "<A-r>").await?;
    assert!(
        !shown(&app),
        "Alt-r was taken away: {:#?}",
        app.rendered_rows()
    );
    send_keys(&mut app, "<A-x>").await?;
    assert!(shown(&app), "{:#?}", app.rendered_rows());
    send_keys(&mut app, "<esc>").await?;
    Ok(())
}

/// The file explorer picker's own keys: `[keys.file-explorer]`.
#[tokio::test(flavor = "multi_thread")]
async fn the_keys_of_a_pickers_own_actions_can_be_bound() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let dir_path = dir.path().canonicalize()?;
    let doomed = dir_path.join("doomed.txt");
    let kept = dir_path.join("kept.txt");
    std::fs::write(&doomed, "x")?;
    std::fs::write(&kept, "x")?;
    let mut app = app(
        "[keys.file-explorer]\n\"C-r\" = \"delete\"\n\"A-d\" = \"no_op\"\n",
        &[&kept],
    )?;
    let asked = |app: &Application| {
        app.rendered_rows()
            .iter()
            .any(|row| row.contains("Delete ") && row.contains("(y/n)"))
    };

    // the explorer of the buffer's folder, on `doomed.txt`
    send_keys(&mut app, "<space>.doomed").await?;
    assert!(
        app.rendered_rows()
            .iter()
            .any(|row| row.contains("doomed.txt")),
        "{:#?}",
        app.rendered_rows()
    );
    // the default key was taken away
    send_keys(&mut app, "<A-d>").await?;
    assert!(!asked(&app), "{:#?}", app.rendered_rows());
    // the key bound to the action asks
    send_keys(&mut app, "<C-r>").await?;
    assert!(asked(&app), "{:#?}", app.rendered_rows());
    send_keys(&mut app, "y<ret>").await?;
    for _ in 0..100 {
        if !doomed.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        send_keys(&mut app, "").await?;
    }
    assert!(
        !doomed.exists(),
        "the file is deleted: {:#?}",
        app.rendered_rows()
    );
    assert!(kept.exists());
    send_keys(&mut app, "<esc>").await?;
    Ok(())
}

/// `.` is a command of the keymap like any other (`repeat_last_insert`): it can be taken
/// away and given to another key.
#[tokio::test(flavor = "multi_thread")]
async fn the_repeat_key_can_be_bound() -> anyhow::Result<()> {
    let text = |app: &Application| helix_view::doc!(app.editor).text().to_string();
    // by default
    let mut default = app("", &[])?;
    send_keys(&mut default, "iab<esc>.3.").await?;
    assert_eq!(text(&default).trim_end(), "ababababab");

    let mut app = app(
        "[keys.normal]\n\".\" = \"no_op\"\n\",\" = \"repeat_last_insert\"\n",
        &[],
    )?;
    send_keys(&mut app, "iab<esc>.").await?;
    assert_eq!(text(&app).trim_end(), "ab", "`.` was taken away");
    send_keys(&mut app, ",2,").await?;
    assert_eq!(text(&app).trim_end(), "abababab");
    // one undo step for each repeat
    send_keys(&mut app, "u").await?;
    assert_eq!(text(&app).trim_end(), "abab");
    Ok(())
}
