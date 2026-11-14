//! The `plaintasks` plugin of `contrib/plugins`: task lists like Sublime Text's PlainTasks.
//! (Its modules have tests of their own: `spec.lua`, run by helix-plugin's tests.)
use super::*;

use helix_term::application::Application;

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

/// The plugin's directory, the task list's directory and the app.
struct Setup {
    app: Application,
    _plugins: tempfile::TempDir,
    dir: tempfile::TempDir,
}

/// An app on 100x20 with the plugin loaded from `contrib/plugins/plaintasks` (the time is
/// 2016-12-31 23:00, `config` adds settings: a TOML inline table's fields) and `name` (in a
/// directory of its own) holding `text`.
async fn app_with(name: &str, text: &str, config: &str) -> anyhow::Result<Setup> {
    let plugins = tempfile::tempdir()?;
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contrib/plugins/plaintasks");
    let dir = plugins.path().join("plaintasks");
    std::fs::create_dir(&dir)?;
    for entry in std::fs::read_dir(&source)? {
        let entry = entry?;
        std::fs::copy(entry.path(), dir.join(entry.file_name()))?;
    }
    let mut app_config = helpers::test_config();
    app_config.editor.notifications.enable = false;
    let settings = if config.is_empty() {
        "now = \"2016-12-31 23:00\"".to_string()
    } else {
        format!("now = \"2016-12-31 23:00\", {config}")
    };
    app_config.plugins = Some(toml::from_str(&format!(
        "plugin_dirs = [{:?}]\n[[plugins]]\nname = \"plaintasks\"\npermissions = [\"edit\", \"commands\", \"keymaps\", \"windows\", \"files\"]\nconfig = {{ {settings} }}\n",
        plugins.path()
    ))?);
    let files = tempfile::tempdir()?;
    let path = files.path().join(name);
    std::fs::write(&path, text)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(app_config)
        .build()?;
    assert!(app.plugins_enabled());
    app.resize_terminal(100, 20).await;
    // the plugin's first drawing
    send_keys(&mut app, "<esc>").await?;
    Ok(Setup {
        app,
        _plugins: plugins,
        dir: files,
    })
}

async fn app(text: &str) -> anyhow::Result<Setup> {
    app_with("list.todo", text, "").await
}

fn text(app: &Application) -> String {
    helix_view::doc!(app.editor).text().to_string()
}

/// The plugin's highlights of the focused buffer: `(text, style)`, in order.
fn highlights(app: &Application) -> Vec<(String, String)> {
    let doc = helix_view::doc!(app.editor);
    let text = doc.text();
    let mut found: Vec<(usize, String, String)> = doc
        .plugin_highlights
        .iter()
        .filter(|h| h.plugin == "plaintasks")
        .map(|h| {
            (
                h.from,
                text.slice(h.from..h.to).to_string(),
                h.scope.clone(),
            )
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|(_, text, style)| (text, style))
        .collect()
}

fn has(found: &[(String, String)], text: &str, style: &str) -> bool {
    found.iter().any(|(t, s)| t == text && s == style)
}

/// The plugin's annotations in the focused window: `(line, text)`.
fn annotations(app: &Application) -> Vec<(usize, String)> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    doc.plugin_annotations
        .get(&view.id)
        .map(|annots| {
            annots
                .iter()
                .filter(|a| a.plugin == "plaintasks")
                .map(|a| (text.char_to_line(a.char_idx), a.text.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The plugin's status line texts.
fn statusline(app: &Application) -> Vec<String> {
    app.editor
        .plugin_statusline
        .iter()
        .filter(|s| s.plugin == "plaintasks")
        .map(|s| s.text.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_list_is_highlighted_with_its_statistics() -> anyhow::Result<()> {
    let setup = app(concat!(
        "Errands:\n",
        " ☐ buy milk @today\n",
        " ✔ call the bank @done (16-12-31 18:00)\n",
        " ✘ fix the bike @cancelled (16-12-31 18:05)\n",
        "   a note\n",
        "--- ✄ -----------------------\n",
    ))
    .await?;
    let app = &setup.app;
    let language: Option<&str> = helix_view::doc!(app.editor).language_name();
    assert_eq!(language, Some("plaintasks"));
    let found = highlights(app);
    assert!(has(&found, "Errands:", "plaintasks.project"), "{found:?}");
    assert!(has(&found, "☐", "plaintasks.bullet"), "{found:?}");
    assert!(has(&found, "@today", "plaintasks.today"), "{found:?}");
    assert!(has(&found, "✔", "plaintasks.done.bullet"), "{found:?}");
    assert!(
        has(
            &found,
            "call the bank @done (16-12-31 18:00)",
            "plaintasks.done"
        ),
        "{found:?}"
    );
    assert!(has(&found, "✘", "plaintasks.cancelled.bullet"), "{found:?}");
    assert!(has(&found, "a note", "plaintasks.note"), "{found:?}");
    assert!(
        has(
            &found,
            "--- ✄ -----------------------",
            "plaintasks.separator"
        ),
        "{found:?}"
    );
    let stats: Vec<String> = statusline(app);
    assert_eq!(
        stats,
        ["2/3 done (66%) ■■■■■■■□□□ Last task @done (16-12-31 18:00)"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tasks_are_done_cancelled_and_made_with_the_keys() -> anyhow::Result<()> {
    let mut setup = app("Errands:\n ☐ buy milk\n  a note\n").await?;
    let app = &mut setup.app;
    // C-d on the task: done, with the moment
    send_keys(app, "j<C-d>").await?;
    assert_eq!(
        text(app),
        "Errands:\n ✔ buy milk @done (16-12-31 23:00)\n  a note\n"
    );
    // A-c: a done task can't be cancelled
    send_keys(app, "<A-c>").await?;
    assert_eq!(
        text(app),
        "Errands:\n ✔ buy milk @done (16-12-31 23:00)\n  a note\n"
    );
    // C-d again: pending again
    send_keys(app, "<C-d>").await?;
    assert_eq!(text(app), "Errands:\n ☐ buy milk\n  a note\n");
    // the <space>t menu: c cancels, and again brings it back
    send_keys(app, "<space>tc").await?;
    assert_eq!(
        text(app),
        "Errands:\n ✘ buy milk @cancelled (16-12-31 23:00)\n  a note\n"
    );
    send_keys(app, "<space>tc").await?;
    assert_eq!(text(app), "Errands:\n ☐ buy milk\n  a note\n");
    // on a note, C-ret makes it a task
    send_keys(app, "j<C-ret><esc>").await?;
    assert_eq!(text(app), "Errands:\n ☐ buy milk\n  ☐ a note\n");
    // C-ret: a new task after it, in insert mode at its end
    send_keys(app, "k<C-ret>eggs<esc>").await?;
    assert_eq!(text(app), "Errands:\n ☐ buy milk\n ☐ eggs\n  ☐ a note\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_keys_keep_their_meaning_in_other_files() -> anyhow::Result<()> {
    let mut setup = app_with("notes.txt", " ☐ buy milk\n", "").await?;
    let app = &mut setup.app;
    let language: Option<&str> = helix_view::doc!(app.editor).language_name();
    assert_ne!(language, Some("plaintasks"));
    // C-d is half a page down here, not "done"
    send_keys(app, "<C-d>").await?;
    assert_eq!(text(app), " ☐ buy milk\n");
    let found = highlights(app);
    assert_eq!(found, Vec::<(String, String)>::new());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tab_expands_tags_and_dates() -> anyhow::Result<()> {
    let mut setup = app(" ☐ pay rent\n").await?;
    let app = &mut setup.app;
    // `t` and Tab: @today; `d` and Tab: @due( ), the cursor in it; a short date and Tab:
    // the date
    send_keys(app, "A t<tab> d<tab>+1<tab><esc>").await?;
    assert_eq!(text(app), " ☐ pay rent @today @due(17-01-01 23:00)\n");
    // `s` Tab Tab: @started with the moment; done: the time spent
    send_keys(app, "A s<tab><tab><esc><C-d>").await?;
    assert_eq!(
        text(app),
        concat!(
            " ✔ pay rent @today @due(17-01-01 23:00) @started(16-12-31 23:00)",
            " @done (16-12-31 23:00) @lasted(a bit)\n"
        )
    );
    // `--` and Tab: a separator
    send_keys(app, "o--<tab><esc>").await?;
    assert!(
        text(app).ends_with("--- ✄ -----------------------\n"),
        "{:?}",
        text(app)
    );
    // Tab elsewhere: what it does without the plugin (an indent)
    send_keys(app, "o<tab>x<esc>").await?;
    let last: String = text(app).lines().last().unwrap_or_default().to_string();
    assert!(last.ends_with("  x") || last.ends_with("\tx"), "{last:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn due_dates_are_marked_with_the_time_left() -> anyhow::Result<()> {
    let setup = app(concat!(
        " ☐ past @due(16-12-30 10:00)\n",
        " ☐ soon @due(17-01-01 10:00)\n",
        " ☐ later @due(17-02-01 10:00)\n",
        " ☐ wrong @due(2-30)\n",
    ))
    .await?;
    let app = &setup.app;
    let found = highlights(app);
    assert!(
        has(&found, "@due(16-12-30 10:00)", "plaintasks.due.past"),
        "{found:?}"
    );
    assert!(
        has(&found, "@due(17-01-01 10:00)", "plaintasks.due.soon"),
        "{found:?}"
    );
    assert!(
        has(&found, "@due(2-30)", "plaintasks.due.invalid"),
        "{found:?}"
    );
    let shown: Vec<(usize, String)> = annotations(app);
    assert_eq!(
        shown,
        [
            (0, " 1 day, 13:00 overdue".to_string()),
            (1, " 11:00 remaining".to_string()),
            (2, " 31 days, 11:00 remaining".to_string()),
        ]
    );
    // the gutter: past and soon
    let doc = helix_view::doc!(app.editor);
    let signs: Vec<(usize, String)> = doc
        .plugin_signs
        .iter()
        .filter(|s| s.plugin == "plaintasks")
        .map(|s| (s.line, s.text.clone()))
        .collect();
    assert_eq!(signs, [(0, "●".to_string()), (1, "•".to_string())]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn done_tasks_are_archived_and_projects_sorted() -> anyhow::Result<()> {
    let mut setup = app(concat!(
        "Work:\n",
        " ☐ later @due(17-02-01 10:00)\n",
        " ✔ mail @done (16-12-30 10:00)\n",
        " ☐ sooner @due(17-01-01 10:00)\n",
    ))
    .await?;
    let app = &mut setup.app;
    send_keys(app, ":tasks-archive<ret>").await?;
    assert_eq!(
        text(app),
        concat!(
            "Work:\n",
            " ☐ later @due(17-02-01 10:00)\n",
            " ☐ sooner @due(17-01-01 10:00)\n",
            "\n",
            "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿\n",
            "Archive:\n",
            " ✔ mail @done (16-12-30 10:00) @project(Work)\n",
        )
    );
    // F5 on the project: its tasks by due date
    send_keys(app, "gg<F5>").await?;
    assert_eq!(
        text(app),
        concat!(
            "Work:\n",
            " ☐ sooner @due(17-01-01 10:00)\n",
            " ☐ later @due(17-02-01 10:00)\n",
            "\n",
            "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿\n",
            "Archive:\n",
            " ✔ mail @done (16-12-30 10:00) @project(Work)\n",
        )
    );
    // undo: one step for the sort
    send_keys(app, "u").await?;
    assert!(text(app).starts_with("Work:\n ☐ later"), "{:?}", text(app));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subtree_is_archived_to_a_file_of_its_own() -> anyhow::Result<()> {
    let mut setup = app("Work:\n ☐ report\nHome:\n ☐ dishes\n").await?;
    let archive = setup.dir.path().join("list_archive.todo");
    let app = &mut setup.app;
    send_keys(app, "<C-S-o>").await?;
    assert_eq!(text(app), "Home:\n ☐ dishes\n");
    // the archive file is written in the background
    let mut written = String::new();
    for _ in 0..100 {
        send_keys(app, "<esc>").await?;
        written = std::fs::read_to_string(&archive).unwrap_or_default();
        if !written.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        written,
        "--- ✄ -----------------------\nArchived (16-12-31 23:00):\nWork:\n ☐ report\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn links_open_their_file_at_their_line() -> anyhow::Result<()> {
    let mut setup = app(" ☐ read ./notes.md:2:3\n").await?;
    std::fs::write(setup.dir.path().join("notes.md"), "one\ntwo three\n")?;
    let app = &mut setup.app;
    send_keys(app, "<A-o>").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let path = doc
        .path()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string());
    assert_eq!(path, Some("notes.md".to_string()));
    let cursor: usize = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(cursor, "one\nt".len() + 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_are_listed_in_a_picker() -> anyhow::Result<()> {
    let mut setup = app("Work:\n ☐ a\n  Errands:\n   ☐ b\nHome:\n").await?;
    let app = &mut setup.app;
    // C-r (the picker opens once the key is handled), then a project's name: the cursor
    // goes to it
    send_keys(app, "<C-r>").await?;
    send_keys(app, "errands<ret>").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let line: usize = text.char_to_line(doc.selection(view.id).primary().cursor(text));
    assert_eq!(line, 2, "{:?}", text.to_string());
    Ok(())
}

/// The keys follow the focused buffer: a task list's in it, the usual ones in another.
#[tokio::test(flavor = "multi_thread")]
async fn the_keys_follow_the_focused_buffer() -> anyhow::Result<()> {
    let mut setup = app(" ☐ in the list\n").await?;
    let notes = setup.dir.path().join("notes.txt");
    std::fs::write(&notes, " ☐ in the notes\n")?;
    let app = &mut setup.app;
    send_keys(app, &format!(":open {}<ret>", notes.display())).await?;
    send_keys(app, "<C-d>").await?;
    assert_eq!(text(app), " ☐ in the notes\n");
    send_keys(app, ":buffer-previous<ret><C-d>").await?;
    assert_eq!(text(app), " ✔ in the list @done (16-12-31 23:00)\n");
    // and back: the usual keys again
    send_keys(app, ":buffer-next<ret><C-d>").await?;
    assert_eq!(text(app), " ☐ in the notes\n");
    Ok(())
}

/// `:open-url` refuses what is not a URL.
#[tokio::test(flavor = "multi_thread")]
async fn open_url_needs_a_url() -> anyhow::Result<()> {
    let mut setup = app(" ☐ a\n").await?;
    let app = &mut setup.app;
    send_keys(app, ":open-url not a url<ret>").await?;
    let status: Option<String> = app.editor.get_status().map(|(text, _)| text.to_string());
    assert!(
        status
            .as_deref()
            .is_some_and(|s| s.contains("expected") || s.contains("not a URL")),
        "{status:?}"
    );
    send_keys(app, ":open-url nope<ret>").await?;
    let status: Option<String> = app.editor.get_status().map(|(text, _)| text.to_string());
    assert!(
        status
            .as_deref()
            .is_some_and(|s| s.contains("'nope' is not a URL")),
        "{status:?}"
    );
    Ok(())
}
