//! Popups, prompts and menus on small screens and with unusual content.
#![cfg(not(windows))]

use super::*;
use helix_term::{application::Application, compositor::Component};
use std::time::Duration;
use tokio_stream::wrappers::UnboundedReceiverStream;

fn config(toml: &str) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(toml).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

fn keys(keys: &str) -> Vec<termina::Event> {
    helix_view::input::parse_macro(keys)
        .unwrap()
        .into_iter()
        .map(|key| termina::Event::Key(key.into()))
        .collect()
}

/// Sends `events` to the application and runs it until it is idle.
async fn send(app: &mut Application, events: Vec<termina::Event>) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = UnboundedReceiverStream::new(rx);
    for event in events {
        tx.send(Ok(event))?;
    }
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// Quits the application.
async fn quit(mut app: Application) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = UnboundedReceiverStream::new(rx);
    for event in keys("<esc><esc>:q!<ret>") {
        tx.send(Ok(event))?;
    }
    tokio::time::timeout(Duration::from_millis(500), app.event_loop(&mut stream)).await?;
    let errors = app.close().await;
    anyhow::ensure!(errors.is_empty(), "errors closing the app: {errors:?}");
    Ok(())
}

/// An option of a [`helix_term::ui::Select`].
struct Choice(&'static str);

impl helix_term::ui::menu::Item for Choice {
    type Data = ();

    fn format(&self, _data: &()) -> helix_term::ui::menu::Row<'_> {
        helix_term::ui::menu::Row::new([helix_term::ui::menu::Cell::from(self.0)])
    }
}

/// The popups, prompts and menus, to render on screens of every size.
fn components(editor: &helix_view::Editor) -> Vec<Box<dyn Component>> {
    use helix_term::ui::{self, Markdown, Popup, Prompt, PromptEvent, Select};
    let completions = |_: &helix_view::Editor, _: &str| -> Vec<ui::prompt::Completion> {
        (0..50)
            .map(|i| (0.., format!("write-{i}-{}", "x".repeat(i)).into()))
            .collect()
    };
    let none = |_: &mut helix_term::compositor::Context, _: &str, _: PromptEvent| {};

    let cmdline = ui::CmdlinePopup::new(
        "Cmdline".into(),
        None,
        completions,
        none,
        helix_view::editor::CmdlineStyle::Popup,
    )
    .with_line("w".repeat(300), editor);
    let mut prompt =
        Prompt::new("replace:".into(), None, completions, none).with_line("é".repeat(300), editor);
    prompt.doc_fn = Box::new(|_| Some("documentation ".repeat(100).into()));
    let huge = format!(
        "# Title\n\n{}\n\n```rust\nfn main() {{}}\n```\n{}",
        "a long line ".repeat(10_000),
        "line\n\n".repeat(40_000)
    );
    let markdown = Popup::new("hover", Markdown::new(huge, editor.syn_loader.clone()));
    let text = Popup::new("text", ui::Text::new("日本語 ".repeat(2_000)));
    let select = Select::new(
        "a question ".repeat(30),
        [Choice("yes"), Choice("no")],
        (),
        |_, _, _| {},
    );
    let confirm = ui::ConfirmBox::new("Delete", "x".repeat(500), "Delete", |_, _| {});
    let info = helix_view::info::Info::new("Info", &[("a", "b".repeat(300))]);
    vec![
        Box::new(cmdline),
        Box::new(prompt),
        Box::new(markdown),
        Box::new(text),
        Box::new(select),
        Box::new(confirm),
        Box::new(info),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn popups_render_on_screens_of_any_size() -> anyhow::Result<()> {
    use helix_term::compositor::Context;
    use helix_view::graphics::Rect;

    for border in ["plain", "rounded", "none"] {
        for gradient in [false, true] {
            let mut app = helpers::AppBuilder::new()
                .with_config(config(&format!(
                    "[editor]\nborder-style = \"{border}\"\n\
                     [editor.gradient-borders]\nenable = {gradient}\nthickness = 4\n"
                )))
                .build()?;
            let mut components = components(&app.editor);
            let mut jobs = helix_term::job::Jobs::new();
            // Every size from nothing to roomy: the list of completions below the command
            // line popup used to be drawn past the bottom of the screen, a select box larger
            // than the screen underflowed its position, ...
            for (cols, rows) in [
                (0, 0),
                (1, 1),
                (2, 2),
                (3, 3),
                (8, 1),
                (10, 2),
                (20, 3),
                (30, 4),
                (40, 6),
                (60, 9),
                (80, 14),
                (120, 40),
            ] {
                let area = Rect::new(0, 0, cols, rows);
                app.editor.resize(area.clip_bottom(1));
                for component in &mut components {
                    let mut surface = tui::buffer::Buffer::empty(area);
                    let mut cx = Context {
                        editor: &mut app.editor,
                        scroll: None,
                        jobs: &mut jobs,
                        plugin_manager: None,
                    };
                    component.required_size((cols, rows));
                    component.render(area, &mut surface, &mut cx);
                    let _ = component.cursor(area, &app.editor);
                }
            }
            app.editor.resize(Rect::new(0, 0, 120, 149));
            quit(app).await?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn command_line_popup_lists_completions_inside_the_screen() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config("[editor.cmdline]\nstyle = \"popup\"\n"))
        .build()?;
    send(&mut app, keys(":w")).await?;
    let screen = app.rendered_rows();
    // the completions of `w` are listed below the input
    assert!(
        screen.iter().any(|row| row.contains("write")),
        "{}",
        screen.join("\n")
    );
    // move through them, with the list scrolling
    send(&mut app, keys(&"<tab>".repeat(15))).await?;
    quit(app).await
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_cursor_follows_its_label_with_the_popup_command_line() -> anyhow::Result<()> {
    // `A-r` opens a plain prompt, which draws its label at the bottom whatever the command
    // line style is: the cursor has to be after the label and the input.
    for style in ["popup", "bottom"] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!("[editor.cmdline]\nstyle = \"{style}\"\n")))
            .build()?;
        send(&mut app, keys("<A-r>abc")).await?;
        let rows = app.rendered_rows();
        let (y, row) = rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row.contains("replace:abc"))
            .unwrap_or_else(|| panic!("no prompt:\n{}", rows.join("\n")));
        let x = row.find("replace:abc").unwrap() + "replace:abc".len();
        let area = app.compositor.size();
        let (cursor, _) = app.compositor.cursor(area, &app.editor);
        let cursor = cursor.expect("a cursor");
        assert_eq!((cursor.col, cursor.row), (x, y), "{style}");
        quit(app).await?;
    }
    Ok(())
}
