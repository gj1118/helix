use super::*;

use helix_term::application::Application;
use helix_view::annotations::panel::{PanelItem, PluginPanel};
use helix_view::graphics::Color;

/// Sends `keys` to the application and runs its event loop until it is idle.
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

/// Twenty lines of 25 characters each.
fn code() -> String {
    (0..20)
        .map(|i| format!("code line {i:02} with stuff\n"))
        .collect()
}
/// The length of a line of `code()`, without its line ending.
const LINE_LEN: usize = "code line 00 with stuff".len();

/// The characters of `row` from column `col` on, `len` of them.
fn cols(row: &str, col: usize, len: usize) -> String {
    row.chars().skip(col).take(len).collect()
}

fn item(text: &str) -> PanelItem {
    PanelItem {
        icon: "●".to_string(),
        icon_fg: Some("#ff0000".to_string()),
        text: text.to_string(),
    }
}

fn panel(char_idx: usize) -> PluginPanel {
    PluginPanel {
        char_idx,
        items: vec![
            item("ALPHA one two three four five six seven eight nine ten"),
            item("BETA"),
        ],
        footer: Some("+1 more".to_string()),
        fg: Some("#000000".to_string()),
        bg: Some("#cba6f7".to_string()),
        ..Default::default()
    }
}

/// The test config, without notification popups (they sit over the right side of the view).
fn config() -> Config {
    let mut config = helpers::test_config();
    config.editor.notifications.enable = false;
    config
}

/// The rows of the screen, padded to `width` (`rendered_rows` trims them).
fn screen(app: &Application, width: usize) -> Vec<String> {
    app.rendered_rows()
        .into_iter()
        .map(|row| format!("{row:width$}"))
        .collect()
}

/// The left column of the text area and its height.
fn text_area(app: &Application) -> (usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let area = view.inner_area(doc);
    (area.x as usize, area.height as usize)
}

fn set_panel(app: &mut Application, panel: Option<PluginPanel>) {
    let (view, doc) = helix_view::current!(app.editor);
    match panel {
        Some(panel) => doc.plugin_panels.insert(view.id, panel),
        None => doc.plugin_panels.remove(&view.id),
    };
}

fn view_offset(app: &Application) -> (usize, usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let offset = doc.view_offset(view.id);
    (
        offset.anchor,
        offset.horizontal_offset,
        offset.vertical_offset,
    )
}

/// Beside the line, the panel is drawn over the text: every line of code stays on its row (the
/// rows the panel covers keep their code up to the panel), the view does not scroll, and the
/// rows below it are untouched.
#[tokio::test(flavor = "multi_thread")]
async fn a_panel_beside_the_line_is_drawn_over_the_code_and_moves_nothing() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(code())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config())
        .build()?;
    app.resize_terminal(80, 12).await;
    let without = screen(&app, 80);
    let offset = view_offset(&app);

    set_panel(&mut app, Some(panel(0)));
    app.resize_terminal(80, 12).await;
    let with = screen(&app, 80);
    let (x, height) = text_area(&app);
    assert_eq!(view_offset(&app), offset);

    // the code (gutter included) is where it was, on every row
    for y in 0..height {
        assert_eq!(
            cols(&with[y], 0, x + LINE_LEN),
            cols(&without[y], 0, x + LINE_LEN),
            "row {y}: {with:#?}"
        );
    }
    // a free column (the cursor at the end of the line), the arrow, the icon, then the text
    let panel_x = x + LINE_LEN + 1 + 3;
    assert_eq!(cols(&with[0], x + LINE_LEN, 4), "  ← ", "{:?}", with[0]);
    assert_eq!(cols(&with[0], panel_x, 8), "● ALPHA ");
    // the long message wraps, over the next line, at the same column
    let message = |y: usize| cols(&with[y], panel_x + 2, usize::MAX).trim().to_string();
    assert_eq!(cols(&with[1], panel_x, 2), "  ");
    assert_eq!(
        format!("{} {}", message(0), message(1)),
        "ALPHA one two three four five six seven eight nine ten"
    );
    assert_eq!(cols(&with[2], panel_x, 7), "● BETA ");
    assert_eq!(cols(&with[3], panel_x, 10), "  +1 more ");
    // and nothing else changes
    for y in 4..with.len() {
        assert_eq!(with[y], without[y], "row {y}");
    }

    // the colors: the icon in its own, the text in the panel's
    let cell = app.rendered_cell(panel_x as u16, 0);
    assert_eq!(cell.symbol.as_str(), "●");
    assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
    assert_eq!(cell.bg, Color::Rgb(0xcb, 0xa6, 0xf7));
    let cell = app.rendered_cell(panel_x as u16 + 2, 0);
    assert_eq!(cell.symbol.as_str(), "A");
    assert_eq!(cell.fg, Color::Rgb(0, 0, 0));
    assert_eq!(cell.bg, Color::Rgb(0xcb, 0xa6, 0xf7));
    // the arrow takes the panel's color, on the editor's background
    let arrow = app.rendered_cell((x + LINE_LEN + 2) as u16, 0);
    assert_eq!(arrow.symbol.as_str(), "←");
    assert_eq!(arrow.fg, Color::Rgb(0xcb, 0xa6, 0xf7));
    assert_ne!(arrow.bg, Color::Rgb(0xcb, 0xa6, 0xf7));

    // removed: the screen is as before
    set_panel(&mut app, None);
    app.resize_terminal(80, 12).await;
    assert_eq!(screen(&app, 80), without);

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// With too few columns left beside the line (a small terminal), the panel goes under it, from
/// its first column; the lines still keep their rows, and widening the terminal brings the
/// panel back beside the line.
#[tokio::test(flavor = "multi_thread")]
async fn a_panel_goes_under_the_line_in_a_narrow_view() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(code())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config())
        .build()?;
    app.resize_terminal(45, 12).await;
    let without = screen(&app, 45);
    let offset = view_offset(&app);

    set_panel(&mut app, Some(panel(0)));
    app.resize_terminal(45, 12).await;
    let with = screen(&app, 45);
    let (x, height) = text_area(&app);
    assert_eq!(view_offset(&app), offset);

    // the gutter (line numbers) is the same on every row, the cursor line too
    for y in 0..height {
        assert_eq!(cols(&with[y], 0, x), cols(&without[y], 0, x), "row {y}");
    }
    assert_eq!(with[0], without[0]);
    // under the line: the arrow, the icon and the text, wrapped to the view
    assert_eq!(cols(&with[1], x, 10), "╰ ● ALPHA ", "{with:#?}");
    assert!(
        with[2].contains("BETA") || with[3].contains("BETA"),
        "{with:#?}"
    );
    let panel_rows = with.iter().filter(|row| row.contains("+1 more")).count();
    assert_eq!(panel_rows, 1);

    // wider: beside the line again
    app.resize_terminal(80, 12).await;
    let wide = screen(&app, 80);
    assert_eq!(cols(&wide[0], x + LINE_LEN, 4), "  ← ", "{wide:#?}");
    assert_eq!(
        cols(&wide[1], 0, x + LINE_LEN),
        cols(&without[1], 0, x + LINE_LEN)
    );

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// The cursor moving down past the bottom of the view scrolls it like without a panel, and a
/// panel on the last line of the view is cut off at its bottom: it neither scrolls the view nor
/// covers the statusline.
#[tokio::test(flavor = "multi_thread")]
async fn a_panel_never_scrolls_the_view_and_is_cut_off_at_its_bottom() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(code())?;
    let line_start = |line: usize| (LINE_LEN + 1) * line;

    let mut offsets = Vec::new();
    let mut screens = Vec::new();
    for with_panel in [false, true] {
        let mut app = helpers::AppBuilder::new()
            .with_file(file.path(), None)
            .with_config(config())
            .build()?;
        app.resize_terminal(80, 12).await;
        let (_, height) = text_area(&app);
        let mut seen = Vec::new();
        // down to past the bottom of the view, with the panel on the cursor line
        for line in 1..height + 3 {
            send_keys(&mut app, "j").await?;
            if with_panel {
                set_panel(&mut app, Some(panel(line_start(line))));
            }
            app.resize_terminal(80, 12).await;
            seen.push(view_offset(&app));
        }
        // the panel on the last line of the view
        if with_panel {
            // the first line of the view
            let first = view_offset(&app).0 / (LINE_LEN + 1);
            set_panel(&mut app, Some(panel(line_start(first + height - 1))));
            app.resize_terminal(80, 12).await;
            seen.push(view_offset(&app));
        } else {
            seen.push(view_offset(&app));
        }
        offsets.push(seen);
        screens.push((app.rendered_rows(), height));
        send_keys(&mut app, ":q!<ret>").await?;
    }
    // the view scrolled (a panel wouldn't change that)...
    assert_ne!(offsets[0][0], offsets[0][offsets[0].len() - 1]);
    assert_eq!(offsets[0], offsets[1]);

    // ...and the panel on its last row changes that row only
    let ((without, height), (with, _)) = (&screens[0], &screens[1]);
    let last = height - 1;
    assert!(with[last].contains("● ALPHA"), "{with:#?}");
    assert!(!with[last].contains("BETA"));
    for y in (0..with.len()).filter(|&y| y != last) {
        assert_eq!(with[y], without[y], "row {y}");
    }
    Ok(())
}

/// The panel is hidden in insert mode (like tiny-inline-diagnostic's `enable_on_insert =
/// false`), unless it asks to be shown there.
#[tokio::test(flavor = "multi_thread")]
async fn a_panel_is_hidden_in_insert_mode_unless_it_asks() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents(code())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config())
        .build()?;
    app.resize_terminal(80, 12).await;
    set_panel(&mut app, Some(panel(0)));

    let shown = |app: &Application| app.rendered_rows().iter().any(|row| row.contains("ALPHA"));
    send_keys(&mut app, "<esc>").await?;
    assert!(shown(&app));
    send_keys(&mut app, "i").await?;
    assert!(!shown(&app));
    send_keys(&mut app, "<esc>").await?;
    assert!(shown(&app));

    let mut in_insert = panel(0);
    in_insert.show_in_insert = true;
    set_panel(&mut app, Some(in_insert));
    send_keys(&mut app, "i").await?;
    assert!(shown(&app));

    send_keys(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}

/// The `inline-diagnostic` plugin of `contrib/plugins` shows the diagnostics of the cursor
/// line in a panel, and removes it when the cursor leaves the line.
#[tokio::test(flavor = "multi_thread")]
async fn the_inline_diagnostic_plugin_shows_a_panel_for_the_cursor_line() -> anyhow::Result<()> {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity,
    };

    let plugins = tempfile::tempdir()?;
    let source = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../contrib/plugins/inline-diagnostic"
    );
    let dir = plugins.path().join("inline-diagnostic");
    std::fs::create_dir(&dir)?;
    for name in ["init.lua", "plugin.toml"] {
        std::fs::copy(format!("{source}/{name}"), dir.join(name))?;
    }
    let mut config = config();
    config.plugins = Some(toml::from_str(&format!(
        "plugin_dirs = [{:?}]",
        plugins.path()
    ))?);

    let file = helpers::temp_file_with_contents(code())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    assert!(app.plugins_enabled());
    app.resize_terminal(80, 12).await;
    let without = screen(&app, 80);

    let diagnostic = |line: usize, severity: Severity, message: &str| {
        let start = (LINE_LEN + 1) * line + 5;
        Diagnostic {
            range: Range {
                start,
                end: start + 4,
            },
            ends_at_word: false,
            starts_at_word: false,
            zero_width: false,
            line,
            message: message.to_string(),
            severity: Some(severity),
            code: None,
            provider: DiagnosticProvider::Lsp {
                server_id: LanguageServerId::default(),
                identifier: None,
            },
            tags: Vec::new(),
            source: None,
            data: None,
        }
    };
    {
        let doc = helix_view::doc_mut!(app.editor);
        doc.replace_diagnostics(
            [
                diagnostic(1, Severity::Warning, "WARNTHING"),
                diagnostic(1, Severity::Error, "BADTHING happened"),
            ],
            &[],
            None,
        );
    }

    // onto the line: the most severe first, in the plugin's colors
    send_keys(&mut app, "j").await?;
    {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let panel = doc.plugin_panels.get(&view.id).expect("a panel");
        let texts: Vec<&str> = panel.items.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, ["BADTHING happened", "WARNTHING"]);
        assert_eq!(panel.items[0].icon_fg.as_deref(), Some("#ff6b6b"));
        assert_eq!(panel.items[1].icon_fg.as_deref(), Some("#ffd93d"));
        assert_eq!(panel.bg.as_deref(), Some("#cba6f7"));
        assert_eq!(panel.footer, None);
        assert_eq!(panel.min_width, 30);
        assert!(!panel.show_in_insert);
        assert_eq!(panel.char_idx, LINE_LEN + 1 + 5);
    }
    let with = screen(&app, 80);
    let (x, height) = text_area(&app);
    assert!(with[1].contains("BADTHING happened"), "{with:#?}");
    assert!(with[2].contains("WARNTHING"), "{with:#?}");
    // the code stays on its rows
    for y in 0..height {
        assert_eq!(
            cols(&with[y], 0, x + LINE_LEN),
            cols(&without[y], 0, x + LINE_LEN)
        );
    }

    // off the line: gone
    send_keys(&mut app, "k").await?;
    {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert!(!doc.plugin_panels.contains_key(&view.id));
    }
    // (the built-in end-of-line diagnostic stays, without the arrow and the other message)
    let rows = app.rendered_rows();
    let panel_shown = rows
        .iter()
        .any(|row| row.contains('←') || row.contains("WARNTHING"));
    assert!(!panel_shown, "{rows:#?}");

    send_keys(&mut app, ":q!<ret>").await?;
    Ok(())
}
