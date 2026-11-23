//! The scrollbar on the right of each view (`[editor.scrollbar]`, after nvim-scrollbar).

use super::*;
use helix_term::application::Application;
use helix_view::graphics::Color;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A config with the editor section `toml`, without notifications (their popups would be
/// drawn over the bar).
fn config(toml: &str) -> Config {
    let toml = format!("{toml}\n[editor.notifications]\nenable = false\n");
    let raw: helix_term::config::ConfigRaw = toml::from_str(&toml).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// The scrollbar with deterministic colours: a red handle, nothing of the background
/// showing through it, and a colour per type of mark.
const COLOURED: &str = r##"
[editor]
true-color = true
[editor.scrollbar]
enable = true
[editor.scrollbar.handle]
color = "#ff0000"
blend = 0
[editor.scrollbar.marks.cursor]
color = "#00ff00"
[editor.scrollbar.marks.search]
color = "#0000ff"
[editor.scrollbar.marks.error]
color = "#010101"
[editor.scrollbar.marks.warn]
color = "#020202"
[editor.scrollbar.marks.hint]
color = "#030303"
[editor.scrollbar.marks.git-change]
color = "#040404"
[editor.scrollbar.marks.git-delete]
color = "#050505"
"##;

const RED: Color = Color::Rgb(255, 0, 0);

/// `COLOURED` with more settings of `[editor.scrollbar]`.
fn coloured_with(settings: &str) -> String {
    COLOURED.replace(
        "[editor.scrollbar]\nenable = true\n",
        &format!("[editor.scrollbar]\nenable = true\n{settings}\n"),
    )
}

/// `line 1` to `line 100`.
fn hundred_lines() -> String {
    (1..=100).map(|i| format!("line {i}\n")).collect()
}

async fn send_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    let mut stream = UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// Lets frames pass until `done` (what runs in the background: the search, the diff).
async fn until(app: &mut Application, done: impl Fn(&Application) -> bool) -> anyhow::Result<()> {
    for _ in 0..150 {
        if done(app) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        send_keys(app, "").await?;
    }
    anyhow::bail!("not done:\n{}", app.rendered_rows().join("\n"))
}

/// An app on a 40×22 screen (a view of 20 rows of text) with `text` in a file `name`.
async fn app_with(
    toml: &str,
    text: &str,
    name: &str,
) -> anyhow::Result<(Application, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(name);
    std::fs::write(&path, text)?;
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config(toml))
        .build()?;
    app.resize_terminal(40, 22).await;
    send_keys(&mut app, "").await?;
    Ok((app, dir))
}

/// The cells of the focused view's scrollbar: their symbol, foreground and background.
fn bar(app: &Application) -> Vec<(String, Color, Color)> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    (inner.top()..inner.bottom())
        .map(|y| {
            let cell = app.rendered_cell(inner.right(), y);
            (cell.symbol.to_string(), cell.fg, cell.bg)
        })
        .collect()
}

/// The rows of the bar with the red handle.
fn handle_rows(bar: &[(String, Color, Color)]) -> Vec<usize> {
    bar.iter()
        .enumerate()
        .filter(|(_, (_, _, bg))| *bg == RED)
        .map(|(row, _)| row)
        .collect()
}

/// The bar takes the last column of the view (the text is a column narrower); the handle
/// covers the rows of the lines on screen and the cursor's line has its mark.
#[tokio::test(flavor = "multi_thread")]
async fn the_handle_follows_the_lines_on_screen_and_the_cursor_has_a_mark() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with(COLOURED, &hundred_lines(), "a.txt").await?;
    {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert_eq!(view.scrollbar_offset(doc), 1);
        assert_eq!(
            view.inner_area(doc).right(),
            39,
            "the bar in the last column"
        );
        assert_eq!(view.inner_width(doc), view.inner_area(doc).width);
    }
    // 100 lines on 20 rows: 5 lines a row; lines 1 to 20 on screen: rows 0 to 3
    let cells = bar(&app);
    assert_eq!(cells.len(), 20);
    assert_eq!(handle_rows(&cells), vec![0, 1, 2, 3]);
    assert_eq!(cells[0], ("•".to_string(), Color::Rgb(0, 255, 0), RED));
    assert_eq!(cells[1].0, " ");
    assert_eq!(cells[4].0, " ");

    // line 50: its mark on row 9 (49 / 5), inside the handle
    send_keys(&mut app, "50gg").await?;
    let cells = bar(&app);
    let handle = handle_rows(&cells);
    assert!(
        handle.contains(&9) && (4..=5).contains(&handle.len()),
        "{handle:?}"
    );
    assert_eq!(cells[9].0, "•");
    assert_eq!(cells[9].1, Color::Rgb(0, 255, 0));
    assert_eq!(cells[0].0, " ", "no mark left behind");
    Ok(())
}

/// Diagnostics, git changes and search matches have their marks; of the marks on a row, the
/// one of the lowest priority is drawn, with its second text (`=`) when there are more.
/// Search marks are shown while the cursor is on a match (`search-calm-down`).
#[tokio::test(flavor = "multi_thread")]
async fn marks_of_diagnostics_git_and_search() -> anyhow::Result<()> {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, DiagnosticTag, Range as DiagnosticRange, Severity,
    };
    let text = hundred_lines();
    let (mut app, _dir) = app_with(COLOURED, &text, "a.txt").await?;
    let provider = DiagnosticProvider::Lsp {
        server_id: helix_lsp::LanguageServerId::default(),
        identifier: None,
    };
    let diagnostic = |line: usize, severity| Diagnostic {
        range: DiagnosticRange { start: 0, end: 1 },
        ends_at_word: true,
        starts_at_word: true,
        zero_width: false,
        line,
        message: "!".into(),
        severity: Some(severity),
        code: None,
        provider: provider.clone(),
        tags: Vec::<DiagnosticTag>::new(),
        source: None,
        data: None,
    };
    {
        let (_view, doc) = helix_view::current!(app.editor);
        doc.replace_diagnostics(
            [
                diagnostic(10, Severity::Error),
                diagnostic(60, Severity::Hint),
                diagnostic(61, Severity::Warning),
            ],
            &[],
            None,
        );
        // since the last commit: line 81 was "old 81", a line after line 90 was removed
        let base: String = (1..=100)
            .flat_map(|i| match i {
                81 => vec!["old 81".to_string()],
                90 => vec!["line 90".to_string(), "gone".to_string()],
                _ => vec![format!("line {i}")],
            })
            .map(|line| line + "\n")
            .collect();
        doc.set_diff_base(base.into_bytes());
    }
    until(&mut app, |app| {
        let cells = bar(app);
        cells[16].0 == "┆" && cells[18].0 == "▁"
    })
    .await?;
    let cells = bar(&app);
    // line 11: the error
    assert_eq!(&cells[2].0, "-");
    assert_eq!(cells[2].1, Color::Rgb(1, 1, 1));
    // lines 61 and 62: the warning over the hint, two marks
    assert_eq!(&cells[12].0, "=");
    assert_eq!(cells[12].1, Color::Rgb(2, 2, 2));
    // line 81 changed, a line removed after line 90
    assert_eq!(cells[16].1, Color::Rgb(4, 4, 4));
    assert_eq!(cells[18].1, Color::Rgb(5, 5, 5));

    // `line 3`, then `line 30` to `line 39`: lines 3 and 30 to 39, rows 0 and 5 to 7
    send_keys(&mut app, "/line 3<ret>").await?;
    until(&mut app, |app| bar(app)[6].0 == "=").await?;
    let cells = bar(&app);
    assert_eq!(cells[6].1, Color::Rgb(0, 0, 255));
    assert_eq!(&cells[5].0, "-", "lines 26 to 30: one match, line 30");
    // row 0: the cursor's mark (priority 0) over the match on its line
    assert_eq!(&cells[0].0, "•");

    // off the match: no search marks
    send_keys(&mut app, "j").await?;
    let cells = bar(&app);
    assert_eq!(&cells[6].0, " ", "{cells:?}");
    assert_eq!(&cells[2].0, "-", "the error's still");

    // `n`: back on a match
    send_keys(&mut app, "n").await?;
    assert_eq!(&bar(&app)[6].0, "=");
    Ok(())
}

/// With `search-calm-down = false`, the matches of the last search stay marked.
#[tokio::test(flavor = "multi_thread")]
async fn search_marks_can_stay() -> anyhow::Result<()> {
    let toml = coloured_with("search-calm-down = false");
    let (mut app, _dir) = app_with(&toml, &hundred_lines(), "a.txt").await?;
    send_keys(&mut app, "/line 3<ret>j").await?;
    until(&mut app, |app| bar(app)[6].0 == "=").await?;
    Ok(())
}

/// `:scrollbar-hide`, `:scrollbar-show` and `:scrollbar-toggle` take the bar (and its column)
/// away and bring it back.
#[tokio::test(flavor = "multi_thread")]
async fn commands_hide_and_show_the_bar() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with(COLOURED, &hundred_lines(), "a.txt").await?;
    let right = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        view.inner_area(doc).right()
    };
    let status = |app: &Application| {
        app.editor
            .get_status()
            .map(|(status, _)| status.to_string())
            .unwrap_or_default()
    };
    assert_eq!(right(&app), 39);
    send_keys(&mut app, ":scrollbar-hide<ret>").await?;
    assert_eq!(right(&app), 40, "the column given back to the text");
    assert_eq!(status(&app), "Scrollbar hidden");
    assert_ne!(app.rendered_cell(39, 0).bg, RED);
    send_keys(&mut app, ":scrollbar-show<ret>").await?;
    assert_eq!(right(&app), 39);
    assert_eq!(status(&app), "Scrollbar shown");
    assert_eq!(handle_rows(&bar(&app)), vec![0, 1, 2, 3]);
    send_keys(&mut app, ":scrollbar-toggle<ret>").await?;
    assert_eq!(right(&app), 40);
    send_keys(&mut app, ":scrollbar-toggle<ret>").await?;
    assert_eq!(right(&app), 39);
    Ok(())
}

/// `show-in-active-only`: the other view keeps the column, empty.
#[tokio::test(flavor = "multi_thread")]
async fn only_in_the_focused_view() -> anyhow::Result<()> {
    let toml = coloured_with("show-in-active-only = true");
    let (mut app, _dir) = app_with(&toml, &hundred_lines(), "a.txt").await?;
    send_keys(&mut app, ":vsplit<ret>").await?;
    let focus = app.editor.tree.focus;
    let other = app
        .editor
        .tree
        .views()
        .map(|(view, _)| view.id)
        .find(|id| *id != focus)
        .expect("two views");
    let column = |id| {
        let view = app.editor.tree.get(id);
        let doc = &app.editor.documents[&view.doc];
        let inner = view.inner_area(doc);
        assert_eq!(view.scrollbar_offset(doc), 1, "the column kept");
        (inner.top()..inner.bottom())
            .filter(|&y| app.rendered_cell(inner.right(), y).bg == RED)
            .count()
    };
    assert_eq!(column(focus), 4);
    assert_eq!(column(other), 0);
    Ok(())
}

/// A buffer on screen whole has its marks but no handle; with `hide-if-all-visible`, no bar.
#[tokio::test(flavor = "multi_thread")]
async fn a_buffer_on_screen_whole() -> anyhow::Result<()> {
    let (app, _dir) = app_with(COLOURED, "one\ntwo\nthree\n", "a.txt").await?;
    let cells = bar(&app);
    assert!(handle_rows(&cells).is_empty());
    assert_eq!(&cells[0].0, "•", "a row per line: the cursor's");

    let toml = coloured_with("hide-if-all-visible = true");
    let (app, _dir) = app_with(&toml, "one\ntwo\nthree\n", "a.txt").await?;
    assert_eq!(&bar(&app)[0].0, " ");
    Ok(())
}

/// No bar (nor column) in a buffer longer than `max-lines` or of an excluded language.
#[tokio::test(flavor = "multi_thread")]
async fn max_lines_and_excluded_languages() -> anyhow::Result<()> {
    let offset = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        view.scrollbar_offset(doc)
    };
    let toml = coloured_with("max-lines = 50");
    let (app, _dir) = app_with(&toml, &hundred_lines(), "a.txt").await?;
    assert_eq!(offset(&app), 0);
    let (app, _dir) = app_with(&toml, "short\n", "a.txt").await?;
    assert_eq!(offset(&app), 1);

    let toml = coloured_with("excluded-languages = [\"rust\"]");
    let (app, _dir) = app_with(&toml, "fn main() {}\n", "a.rs").await?;
    assert_eq!(offset(&app), 0);
    let (app, _dir) = app_with(&toml, "text\n", "a.txt").await?;
    assert_eq!(offset(&app), 1);
    Ok(())
}

/// Each type's text and the handle's can be set, and `handlers` turn marks (and the handle)
/// off.
#[tokio::test(flavor = "multi_thread")]
async fn texts_and_handlers_are_settings() -> anyhow::Result<()> {
    let texts = COLOURED
        .replace(
            "[editor.scrollbar.marks.cursor]\n",
            "[editor.scrollbar.marks.cursor]\ntext = \"@\"\n",
        )
        .replace(
            "[editor.scrollbar.handle]\n",
            "[editor.scrollbar.handle]\ntext = \"|\"\n",
        );
    let (app, _dir) = app_with(&texts, &hundred_lines(), "a.txt").await?;
    let cells = bar(&app);
    assert_eq!(&cells[0].0, "@");
    assert_eq!(cells[0].1, Color::Rgb(0, 255, 0));
    assert_eq!(&cells[1].0, "|");
    assert_eq!(cells[1].2, RED);

    let off = format!("{texts}[editor.scrollbar.handlers]\ncursor = false\nhandle = false\n");
    let (app, _dir) = app_with(&off, &hundred_lines(), "a.txt").await?;
    let cells = bar(&app);
    assert_eq!(&cells[0].0, " ");
    assert!(handle_rows(&cells).is_empty());
    Ok(())
}

/// Folded lines are left out of the bar (`folds`: in buffers of at most that many lines): a
/// fold of 80 lines in 100 leaves 20, a row each; with `folds = 0` they are counted.
#[tokio::test(flavor = "multi_thread")]
async fn folded_lines_are_left_out() -> anyhow::Result<()> {
    let text = format!(
        "fn a() {{\n{}}}\n{}",
        "    let x = 1;\n".repeat(80),
        "fn b() {}\n".repeat(18)
    );
    let folded = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let container = doc.fold_container(view.id)?;
        let start = container.start_points().first()?;
        let fold = start.fold(container);
        Some((fold.start.line, fold.end.line))
    };
    for (settings, folds_counted) in [("", false), ("folds = 0", true)] {
        let (mut app, _dir) = app_with(&coloured_with(settings), &text, "a.rs").await?;
        send_keys(&mut app, "za").await?;
        let (first, last) = folded(&app).expect("a fold");
        let hidden = last - first + 1;
        assert!(hidden >= 79, "the body folded: {first}..={last}");
        // the first line after the fold
        send_keys(&mut app, &format!("{}gg", last + 2)).await?;
        let row = bar(&app).iter().position(|(symbol, _, _)| symbol == "•");
        let expected = if folds_counted {
            // 100 lines on 20 rows
            (last + 1) * 20 / 100
        } else {
            // the lines left fit: a row each, the fold's at its header's
            last + 1 - hidden
        };
        assert_eq!(row, Some(expected), "{settings:?}");
    }
    Ok(())
}

/// 160 lines on 20 rows: a line is an eighth of a row. The handle's ends are blocks as tall
/// as the part of their row it covers: the handle moves by a line at a time.
#[tokio::test(flavor = "multi_thread")]
async fn the_handle_moves_by_eighths_of_rows() -> anyhow::Result<()> {
    use helix_view::graphics::Modifier;
    let text: String = (1..=160).map(|i| format!("line {i}\n")).collect();
    // (no cursor mark over the handle's ends)
    let toml = format!("{COLOURED}[editor.scrollbar.handlers]\ncursor = false\n");
    let (mut app, _dir) = app_with(&toml, &text, "a.txt").await?;
    // lines 1 to 20: 20 eighths, rows 0 and 1 and the upper half of row 2
    let cells = bar(&app);
    assert_eq!(handle_rows(&cells), vec![0, 1]);
    let row = |app: &Application, row: u16| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let inner = view.inner_area(doc);
        let cell = app.rendered_cell(inner.right(), inner.y + row);
        (cell.symbol.to_string(), cell.fg, cell.modifier)
    };
    // the upper half: the lower half in the background, the colours reversed
    assert_eq!(row(&app, 2), ("▄".to_string(), RED, Modifier::REVERSED));

    // three lines down: the handle from 3 eighths into row 0 to 7 eighths into row 2
    send_keys(&mut app, "zjzjzj").await?;
    assert_eq!(row(&app, 0), ("▅".to_string(), RED, Modifier::empty()));
    assert_eq!(handle_rows(&bar(&app)), vec![1]);
    assert_eq!(row(&app, 2), ("▁".to_string(), RED, Modifier::REVERSED));
    assert_eq!(&bar(&app)[3].0, " ");

    // whole rows with `smooth = false`: rows 0 to 2
    let toml = toml.replace(
        "[editor.scrollbar.handle]\n",
        "[editor.scrollbar.handle]\nsmooth = false\n",
    );
    let (mut whole, _dir) = app_with(&toml, &text, "a.txt").await?;
    send_keys(&mut whole, "zjzjzj").await?;
    assert_eq!(handle_rows(&bar(&whole)), vec![0, 1, 2]);
    Ok(())
}

/// A mouse event at `column`, `row`.
async fn mouse(
    app: &mut Application,
    kind: termina::event::MouseEventKind,
    column: u16,
    row: u16,
) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Ok(termina::Event::Mouse(termina::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: termina::event::Modifiers::empty(),
    })))?;
    app.event_loop_until_idle(&mut UnboundedReceiverStream::new(rx))
        .await;
    Ok(())
}

/// A click on the bar scrolls the view to put the handle's middle there, the cursor kept
/// on screen; dragging the handle scrolls along with it; above or below the bar, as far as
/// the view goes.
#[tokio::test(flavor = "multi_thread")]
async fn a_click_and_a_drag_on_the_bar_scroll() -> anyhow::Result<()> {
    use termina::event::{MouseButton, MouseEventKind};
    let toml = format!("{COLOURED}\n[editor]\nmouse = true\n")
        .replace("[editor]\ntrue-color = true\n", "")
        .replace(
            "\n[editor]\nmouse = true\n",
            "\n[editor]\nmouse = true\ntrue-color = true\n",
        );
    let (mut app, _dir) = app_with(&toml, &hundred_lines(), "a.txt").await?;
    let top_line = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        doc.text().char_to_line(doc.view_offset(view.id).anchor)
    };
    let cursor_line = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let text = doc.text().slice(..);
        text.char_to_line(doc.selection(view.id).primary().cursor(text))
    };
    let (x, y) = {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let inner = view.inner_area(doc);
        (inner.right(), inner.y)
    };
    // 100 lines on 20 rows, 20 lines on screen: a handle of 4 rows. A click on row 10 (its
    // middle 84 eighths): the handle from 68 eighths, line 42 at the top
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y + 10).await?;
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, y + 10).await?;
    assert_eq!(top_line(&app), 42);
    let cursor = cursor_line(&app);
    assert!((42..62).contains(&cursor), "the cursor on screen: {cursor}");
    assert_eq!(handle_rows(&bar(&app)).first(), Some(&9));

    // held, not moved: where it was; dragged 5 rows up: 40 eighths, 25 lines
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y + 10).await?;
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, y + 10).await?;
    assert_eq!(top_line(&app), 42);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, y + 5).await?;
    assert_eq!(top_line(&app), 17);
    // past the bottom: the last lines on screen
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, y + 40).await?;
    assert_eq!(top_line(&app), 80);
    // past the top: the first
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, 0).await?;
    assert_eq!(top_line(&app), 0);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, 0).await?;
    assert_eq!(
        cursor_line(&app),
        14,
        "kept on screen, 5 rows from its bottom"
    );

    // `mouse = false`: a click there is a click on the text's last column
    let toml = toml.replace(
        "[editor.scrollbar]\nenable = true\n",
        "[editor.scrollbar]\nenable = true\nmouse = false\n",
    );
    let (mut off, _dir) = app_with(&toml, &hundred_lines(), "a.txt").await?;
    mouse(&mut off, MouseEventKind::Down(MouseButton::Left), x, y + 10).await?;
    mouse(&mut off, MouseEventKind::Up(MouseButton::Left), x, y + 10).await?;
    assert_eq!(top_line(&off), 0);
    Ok(())
}
