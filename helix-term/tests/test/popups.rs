//! Popups, prompts and menus on small screens and with unusual content.
#![cfg(not(windows))]

use super::*;
use helix_term::compositor::Component;

fn config(toml: &str) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(toml).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
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
    let completions = |_: &helix_view::Editor, _: &str| -> ui::completers::CompletionResult {
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
    send_keys(&mut app, ":w").await?;
    let screen = app.rendered_rows();
    // the completions of `w` are listed below the input
    assert!(
        screen.iter().any(|row| row.contains("write")),
        "{}",
        screen.join("\n")
    );
    // move through them, with the list scrolling
    send_keys(&mut app, &"<tab>".repeat(15)).await?;
    quit(app).await
}

/// The help of the command below the popup command line has the same border as the input
/// and the completions: the gradient one when gradient borders are on.
/// `position` of `[editor.cmdline]` places the popup: at the bottom with its completions and
/// help above it, right below the cursor's line with them below it.
#[tokio::test(flavor = "multi_thread")]
async fn command_line_popup_position() -> anyhow::Result<()> {
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    for (position, keys) in [("bottom", ""), ("cursor", "6gg"), ("default", "")] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!(
                "[editor.cmdline]\nstyle = \"popup\"\nposition = \"{position}\"\n"
            )))
            .with_input_text(format!("#[|]#{text}"))
            .build()?;
        send_keys(&mut app, keys).await?;
        let cursor_row = app.editor.cursor().0.unwrap().row;
        send_keys(&mut app, ":config-reload").await?;
        let rows = app.rendered_rows();
        let find = |text: &str, not: &str| {
            rows.iter()
                .position(|row| row.contains(text) && (not.is_empty() || !row.contains(not)))
                .unwrap_or_else(|| panic!("{position}: no {text:?}:\n{}", rows.join("\n")))
        };
        // the input, below the popup's title, and its completion
        let input = find("Cmdline", "") + 1;
        assert!(rows[input].contains("config-reload"), "{position}");
        let completion = rows
            .iter()
            .enumerate()
            .position(|(y, row)| y != input && row.contains("config-reload"))
            .unwrap_or_else(|| panic!("{position}: no completion"));
        let help = find("Refresh user config", "");
        let context = format!("{position}:\n{}", rows.join("\n"));
        match position {
            "bottom" => {
                assert_eq!(input, rows.len() - 3, "{context}");
                assert!(help < completion && completion < input, "{context}");
            }
            "cursor" => {
                assert_eq!(input, cursor_row + 2, "{context}");
                assert!(input < completion && completion < help, "{context}");
            }
            _ => {
                assert!(input < rows.len() / 2, "{context}");
                assert!(input < completion && completion < help, "{context}");
            }
        }
        send_keys(&mut app, "<esc>").await?;
        quit(app).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn command_line_popup_keeps_its_title_without_borders() -> anyhow::Result<()> {
    for gradient in [true, false] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!(
                "[editor]\nborder-style = \"none\"\n\
                 [editor.cmdline]\nstyle = \"popup\"\n\
                 [editor.gradient-borders]\nenable = {gradient}\n"
            )))
            .build()?;
        send_keys(&mut app, ":config-reload").await?;
        let rows = app.rendered_rows();
        let context = format!("gradient {gradient}:\n{}", rows.join("\n"));
        let title = rows.iter().position(|row| row.contains("Cmdline"));
        assert_eq!(
            title.map(|title| rows[title + 1].contains("config-reload")),
            Some(true),
            "the title is right above the input: {context}"
        );
        // no border glyphs around the input
        let input = &rows[title.unwrap() + 1];
        assert!(!input.contains(['│', '▏', '▕', '┃', '║']), "{context}");
        send_keys(&mut app, "<esc>").await?;
        quit(app).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn command_line_popup_help_follows_the_gradient_border() -> anyhow::Result<()> {
    use helix_view::graphics::Color;
    for gradient in [true, false] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!(
                "[editor]\nborder-style = \"plain\"\n\
                 [editor.cmdline]\nstyle = \"popup\"\n\
                 [editor.gradient-borders]\nenable = {gradient}\n\
                 start-color = \"#ff0000\"\nend-color = \"#ff0000\"\nanimation-speed = 0\n"
            )))
            .build()?;
        send_keys(&mut app, ":config-reload").await?;
        let rows = app.rendered_rows();
        let (y, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains("Refresh user config"))
            .unwrap_or_else(|| panic!("no help:\n{}", rows.join("\n")));
        // the border is left of the column of margin before the text
        let text_x = row[..row.find("Refresh user config").unwrap()]
            .chars()
            .count();
        let border_x = (text_x - 2) as u16;
        let help_border = app.rendered_cell(border_x, y as u16).clone();
        // the left border of the input box, on the row of the input
        let input_y = rows
            .iter()
            .position(|row| row.contains("config-reload"))
            .expect("the input") as u16;
        let input_border = app.rendered_cell(border_x, input_y).clone();
        assert_eq!(
            help_border.symbol,
            input_border.symbol,
            "{gradient}:\n{}",
            rows.join("\n")
        );
        if gradient {
            assert_eq!(help_border.fg, input_border.fg);
        }
        let red = help_border.fg == Color::Rgb(255, 0, 0);
        assert_eq!(
            red, gradient,
            "{gradient}: border colour {:?}",
            help_border.fg
        );
        quit(app).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_cursor_follows_its_label_with_the_popup_command_line() -> anyhow::Result<()> {
    // `A-r` opens a plain prompt, which draws its label at the bottom whatever the command
    // line style is: the cursor has to be after the label and the input.
    for style in ["popup", "bottom"] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!("[editor.cmdline]\nstyle = \"{style}\"\n")))
            .build()?;
        send_keys(&mut app, "<A-r>abc").await?;
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

/// A signature help with documentation wider than a view.
fn signature_help(
    editor: &helix_view::Editor,
) -> helix_term::ui::Popup<helix_term::ui::lsp::signature_help::SignatureHelp> {
    use helix_term::ui::lsp::signature_help::{Signature, SignatureHelp};
    use helix_term::ui::Popup;
    let contents = SignatureHelp::new(
        "go".to_string(),
        editor.syn_loader.clone(),
        0,
        None,
        vec![Signature {
            signature: "Println(a ...any) (n int, err error)".to_string(),
            signature_doc: Some("Println formats using the default formats. ".repeat(12)),
            active_param_range: None,
        }],
    );
    Popup::new(SignatureHelp::ID, contents)
        .ignore_escape_key(true)
        .panel_border(true)
}

/// A popup at the cursor (signature help, hover, completion) stays within the columns of the
/// cursor's view: in a split it doesn't cover the view beside it. In a view too narrow to
/// read it in, it takes the room it needs.
#[tokio::test(flavor = "multi_thread")]
async fn a_popup_at_the_cursor_stays_within_its_view() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[f|]#mt.Println()\nsecond\nthird\n")
        .build()?;
    app.resize_terminal(160, 40).await;
    send_keys(&mut app, ":vsplit<ret>A").await?;

    let screen = app.compositor.size();
    let (view, _) = helix_view::current_ref!(app.editor);
    let view_area = view.area;
    assert!(
        view_area.left() > 0 && view_area.width < 100,
        "{view_area:?}"
    );
    let mut popup = signature_help(&app.editor);
    let area = popup.area(screen, &app.editor);
    assert!(area.width > 40, "{area:?}");
    assert!(area.left() >= view_area.left(), "{area:?} in {view_area:?}");
    assert!(
        area.right() <= view_area.right(),
        "{area:?} in {view_area:?}"
    );

    // the left view: within it too
    send_keys(&mut app, "<esc><C-w>hA").await?;
    let (view, _) = helix_view::current_ref!(app.editor);
    let view_area = view.area;
    assert_eq!(view_area.left(), 0u16);
    let mut popup = signature_help(&app.editor);
    let area = popup.area(screen, &app.editor);
    assert!(
        area.right() <= view_area.right(),
        "{area:?} in {view_area:?}"
    );

    // one view of 160 columns: 60% of it by default (`popup-max-width-percent`), border
    // included
    send_keys(&mut app, "<esc><C-w>oA").await?;
    let mut popup = signature_help(&app.editor);
    let area = popup.area(screen, &app.editor);
    assert_eq!(area.width, 96u16, "{area:?}");
    // the share can be set: 120 columns at most, and 40 at least
    for (percent, width) in [(100u8, 120u16), (50, 80), (25, 40), (10, 40)] {
        let set = format!("<esc>:set popup-max-width-percent {percent}<ret>A");
        send_keys(&mut app, &set).await?;
        let mut popup = signature_help(&app.editor);
        let area = popup.area(screen, &app.editor);
        assert_eq!(area.width, width, "{percent}%: {area:?}");
    }

    // views too narrow to read a popup in: it spreads over its neighbours
    send_events(
        &mut app,
        key_events("<esc>:vsplit<ret>:vsplit<ret>:vsplit<ret>:vsplit<ret>A"),
    )
    .await?;
    let (view, _) = helix_view::current_ref!(app.editor);
    let view_area = view.area;
    assert!(view_area.width < 40, "{view_area:?}");
    let mut popup = signature_help(&app.editor);
    let area = popup.area(screen, &app.editor);
    assert!(area.width > view_area.width, "{area:?} over {view_area:?}");
    assert!(area.right() <= screen.right(), "{area:?}");

    // one view left, for `quit`
    send_keys(&mut app, "<esc><C-w>o").await?;
    quit(app).await
}

/// The documentation of the selected completion, drawn by the completion menu, and where its
/// text is on the screen: the rows and columns of the cells of `word`.
fn completion_documentation(
    app: &mut helix_term::application::Application,
    word: &str,
) -> Vec<(u16, u16)> {
    use helix_term::compositor::Context;
    use helix_term::ui::CompletionItem;

    let doc = helix_view::doc!(app.editor);
    let item = helix_core::CompletionItem {
        transaction: helix_core::Transaction::new(doc.text()),
        label: "Println".into(),
        kind: "function".into(),
        documentation: Some(
            format!("Println prints its {word} separated by spaces and a newline. ").repeat(6),
        ),
        snippet: None,
        provider: helix_core::completion::CompletionProvider::Word,
    };
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    let mut completion =
        helix_term::ui::Completion::new(&app.editor, vec![CompletionItem::Other(item)], cursor);
    let area = app.compositor.size();
    let mut surface = tui::buffer::Buffer::empty(area);
    let mut jobs = helix_term::job::Jobs::new();
    let mut cx = Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
        plugin_manager: None,
    };
    // the first item selected, as `C-n` does: its documentation is shown
    for key in helix_view::input::parse_macro("<C-n>").unwrap() {
        let _ = completion.handle_event(&helix_view::input::Event::Key(key), &mut cx);
    }
    completion.render(area, &mut surface, &mut cx);
    let mut cells = Vec::new();
    for y in area.top()..area.bottom() {
        let row: String = (area.left()..area.right())
            .map(|x| surface[(x, y)].symbol.chars().next().unwrap_or(' '))
            .collect();
        cells.extend(row.match_indices(word).map(|(x, _)| (y, x as u16)));
    }
    cells
}

/// The documentation of a completion stays in the cursor's view, like hover: beside the menu
/// (on its left when the right has no room), else right above the cursor; never across the
/// screen, over the views beside it.
#[tokio::test(flavor = "multi_thread")]
async fn completion_documentation_stays_within_its_view() -> anyhow::Result<()> {
    // Lines ending in a space: no word before the cursor at their end to filter the menu by.
    let text: String = (0..30)
        .map(|i| format!("line {i} {} \n", "x".repeat(55)))
        .collect();
    let text = format!("#[|]#{text}");
    // Selecting the item doesn't insert it: nothing else of a completion session is set up.
    let mut app = helpers::AppBuilder::new()
        .with_config(config("[editor]\npreview-completion-insert = false\n"))
        .with_input_text(&text)
        .build()?;
    app.resize_terminal(160, 40).await;
    use helix_term::application::Application;
    let view_area = |app: &Application| helix_view::current_ref!(app.editor).0.area;
    let cursor = |app: &Application| app.editor.cursor().0.unwrap();

    // one view: on the right of the menu
    send_keys(&mut app, "24ggA").await?;
    let row = cursor(&app).row as u16;
    let docs = completion_documentation(&mut app, "arguments");
    assert!(!docs.is_empty());
    assert!(
        docs.iter()
            .all(|&(y, x)| y >= row && x > cursor(&app).col as u16 + 15),
        "{docs:?}"
    );

    // the right one of two views, the cursor at its end: on the left of the menu, in the view
    send_keys(&mut app, "<esc>:vsplit<ret>A").await?;
    let area = view_area(&app);
    assert!(area.left() > 0, "{area:?}");
    let docs = completion_documentation(&mut app, "arguments");
    assert!(!docs.is_empty());
    let col = cursor(&app).col as u16;
    assert!(
        docs.iter().all(|&(_, x)| x >= area.left() && x + 9 <= col),
        "{docs:?} in {area:?}, cursor at {col}"
    );

    // the middle one of three narrow views, the cursor in its middle: no room beside the menu,
    // so right above the cursor, in the view (the view, not the top of the screen)
    send_keys(&mut app, "<esc>:vsplit<ret><C-w>h020li <esc>i").await?;
    let area = view_area(&app);
    assert!(area.left() > 0 && area.right() < 160, "{area:?}");
    let docs = completion_documentation(&mut app, "arguments");
    assert!(!docs.is_empty());
    let row = cursor(&app).row as u16;
    assert!(
        docs.iter()
            .all(|&(y, x)| y < row && x >= area.left() && x + 9 <= area.right()),
        "{docs:?} in {area:?}, cursor on row {row}"
    );
    let last = docs.iter().map(|&(y, _)| y).max().unwrap();
    assert!(
        last + 4 >= row,
        "its last line {last} is near the cursor on row {row}"
    );

    send_keys(&mut app, "<esc><C-w>o").await?;
    quit(app).await
}

/// Clicking another view closes the signature help: it is about the call the cursor was in,
/// and stayed up over the view clicked.
#[tokio::test(flavor = "multi_thread")]
async fn clicking_another_view_closes_the_signature_help() -> anyhow::Result<()> {
    use helix_term::ui::lsp::signature_help::SignatureHelp;
    use termina::event::{Modifiers as KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[f|]#mt.Println()\nsecond\nthird\n")
        .build()?;
    app.resize_terminal(160, 40).await;
    send_keys(&mut app, ":vsplit<ret>A").await?;
    let origin = app.editor.tree.focus;
    let popup = signature_help(&app.editor);
    app.compositor.push(Box::new(popup));
    helpers::run_event_loop_until_idle(&mut app).await;
    assert!(app.compositor.has_id(SignatureHelp::ID));

    // a click in the same view leaves it
    let click = |column, row| {
        vec![termina::Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        })]
    };
    let (view, doc) = helix_view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    send_events(&mut app, click(inner.x + 3, inner.y + 30)).await?;
    assert_eq!(app.editor.tree.focus, origin);
    assert!(app.compositor.has_id(SignatureHelp::ID));

    // a click in the other view closes it
    let other = app
        .editor
        .tree
        .views()
        .map(|(view, _)| view)
        .find(|view| view.id != origin)
        .map(|view| (view.id, view.area))
        .unwrap();
    send_events(&mut app, click(other.1.x + 12, other.1.y + 30)).await?;
    assert_eq!(app.editor.tree.focus, other.0);
    assert!(!app.compositor.has_id(SignatureHelp::ID));
    send_keys(&mut app, "<esc><C-w>o").await?;
    quit(app).await
}

/// `[editor.cmdline] use-full-height`: with the popup command line, the editor takes the
/// bottom row too (the status line is the last row); without it, the bottom row stays for
/// the command line and messages.
#[tokio::test(flavor = "multi_thread")]
async fn popup_command_line_full_height_takes_the_bottom_row() -> anyhow::Result<()> {
    for (style, full_height, last_row_is_status) in [
        ("popup", true, true),
        ("popup", false, false),
        ("bottom", true, false),
    ] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!(
                "[editor.cmdline]\nstyle = \"{style}\"\nuse-full-height = {full_height}\n\
                 [editor.bufferline]\nrender-mode = \"always\"\n"
            )))
            .build()?;
        send_keys(&mut app, "ihello<esc>").await?;
        let rows = app.rendered_rows();
        let last = rows.len() - 1;
        let status_row = rows
            .iter()
            .rposition(|row| row.contains("NOR") && row.contains("[scratch]"))
            .unwrap_or_else(|| panic!("no status line:\n{}", rows.join("\n")));
        let expected = if last_row_is_status { last } else { last - 1 };
        assert_eq!(
            status_row, expected,
            "{style} use-full-height = {full_height}"
        );
        // the window reaches the status line: its area ends on the status line's row
        let view = app.editor.tree.get(app.editor.tree.focus);
        assert_eq!(
            (view.area.y + view.area.height - 1) as usize,
            expected,
            "{style} use-full-height = {full_height}"
        );
        quit(app).await?;
    }
    Ok(())
}

/// A notification has a header: the icon and the severity, the time it came on the right,
/// and a line under them, each part in the style of its theme scope; the message is below.
#[tokio::test(flavor = "multi_thread")]
async fn a_notification_has_a_header_styled_by_the_theme() -> anyhow::Result<()> {
    use helix_view::graphics::Color;
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            "[editor.notifications]\nstyle = \"popup\"\nshow-emojis = false\nshadow.enable = false\n",
        ))
        .build()?;
    app.editor.set_theme(toml::from_str(
        r##"
        "ui.selection" = { modifiers = ["reversed"] }
        "ui.cursor" = { modifiers = ["reversed"] }
        "ui.popup" = { fg = "#101010", bg = "#202020" }
        "warning" = { fg = "#d0a050" }
        "ui.notification.text" = { fg = "#c0c0c0" }
        "ui.notification.time" = { fg = "#a0a0a0" }
        "ui.notification.separator.warning" = { fg = "#806030" }
        "##,
    )?)?;
    app.editor
        .set_warning("Formatters unavailable for yaml file");
    send_keys(&mut app, "").await?;

    let rows = app.rendered_rows();
    if std::env::var_os("SHOW_CARD").is_some() {
        for row in &rows[..8] {
            eprintln!("CARD|{row}");
        }
    }
    let row_of = |text: &str| {
        rows.iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no {text}: {rows:#?}"))
    };
    let (title, message) = (row_of("⚠ Warning"), row_of("Formatters unavailable"));
    // the title, then its line, then the message
    assert_eq!(message, title + 2, "{rows:#?}");
    let line = &rows[title + 1];
    assert!(line.contains(&"─".repeat(30)), "{rows:#?}");

    // the time of the notification, at the right end of the title's row
    let clock = {
        let notifications = app.editor.get_active_notifications();
        notifications[0].clock.clone()
    };
    assert_eq!(clock.len(), 8, "HH:MM:SS: {clock}");
    assert_eq!(clock.as_bytes()[2], b':');
    let header = rows[title].trim_end();
    assert!(header.contains(&clock), "{rows:#?}");
    let message_end = rows[message]
        .trim_end()
        .trim_end_matches('│')
        .trim_end()
        .chars()
        .count();
    let time_end = header.trim_end_matches('│').trim_end().chars().count();
    // (the card is as wide as the message: the time ends where its column ends)
    assert!(time_end <= message_end + 1, "{rows:#?}");

    // each part in its color
    let x_of = |row: usize, text: &str| {
        rows[row]
            .find(text)
            .map(|at| rows[row][..at].chars().count())
    };
    let color = |x: usize, y: usize| app.rendered_cell(x as u16, y as u16).fg;
    let title_x = x_of(title, "Warning").unwrap();
    assert_eq!(color(title_x, title), Color::Rgb(0xd0, 0xa0, 0x50));
    let time_x = x_of(title, &clock).unwrap();
    assert_eq!(color(time_x, title), Color::Rgb(0xa0, 0xa0, 0xa0));
    let line_x = x_of(title + 1, "─").unwrap();
    assert_eq!(color(line_x + 3, title + 1), Color::Rgb(0x80, 0x60, 0x30));
    let message_x = x_of(message, "Formatters").unwrap();
    assert_eq!(color(message_x, message), Color::Rgb(0xc0, 0xc0, 0xc0));
    assert_eq!(
        app.rendered_cell(message_x as u16, message as u16).bg,
        Color::Rgb(0x20, 0x20, 0x20)
    );

    // without a header: the icon before the message, in the severity's color
    let mut app = helpers::AppBuilder::new()
        .with_config(config(
            "[editor.notifications]\nstyle = \"popup\"\nshow-emojis = false\nheader = false\n",
        ))
        .build()?;
    app.editor
        .set_warning("Formatters unavailable for yaml file");
    send_keys(&mut app, "").await?;
    let rows = app.rendered_rows();
    assert!(
        rows.iter()
            .any(|row| row.contains("⚠ Formatters unavailable")),
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|row| row.contains("Warning")), "{rows:#?}");
    Ok(())
}
