//! kitty's multiple cursors protocol (helix-editor/helix#14757): with a bar or underline cursor,
//! the terminal draws the secondary cursors next to the main one instead of the editor faking
//! them with highlighted cells.

use super::*;

use helix_term::application::Application;
use helix_view::{current_ref, editor::CursorShapeConfig, graphics::Rect};

/// Bar cursors in insert mode (block in normal mode), on a terminal that can draw secondary
/// cursors when `kitty` is set.
fn app(text: &str, kitty: bool) -> anyhow::Result<Application> {
    let mut config = helpers::test_config();
    config.editor.cursor_shape = toml::from_str::<CursorShapeConfig>("insert = \"bar\"")?;
    config.editor.auto_completion = false;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_input_text(text)
        .build()?;
    app.editor.kitty_multi_cursor_support = kitty;
    Ok(app)
}

fn inner_area(app: &Application) -> Rect {
    let (view, doc) = current_ref!(app.editor);
    view.inner_area(doc)
}

/// Whether the editor painted a cursor on the cell at `(x, y)`: it looks different from the
/// plain text cell next to it.
fn painted(app: &Application, x: u16, y: u16) -> bool {
    let (cell, next) = (app.rendered_cell(x, y), app.rendered_cell(x + 1, y));
    (cell.fg, cell.bg, cell.modifier) != (next.fg, next.bg, next.modifier)
}

/// The secondary cursors the terminal currently draws.
fn terminal_cursors(app: &Application) -> Vec<(u16, u16)> {
    app.sent_secondary_cursors()
        .last()
        .cloned()
        .unwrap_or_default()
}

const THREE_LINES: &str = "#[|o]#ne\n#(|t)#wo\n#(|t)#hree\n";

/// In insert mode (bar cursor) the terminal draws the two secondary cursors, at the start of
/// the second and third lines, and the editor no longer paints them; back in normal mode
/// (block cursor) the editor draws them again and the terminal's are removed.
#[tokio::test(flavor = "multi_thread")]
async fn terminal_draws_secondary_bar_cursors() -> anyhow::Result<()> {
    let mut app = app(THREE_LINES, true)?;
    let inner = inner_area(&app);

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("i"),
                Some(&|app| {
                    assert_eq!(
                        terminal_cursors(app),
                        [(inner.x, inner.y + 1), (inner.x, inner.y + 2)]
                    );
                    for row in [1, 2] {
                        let cell = app.rendered_cell(inner.x, inner.y + row);
                        assert_eq!(cell.symbol.as_str(), "t");
                        assert!(!painted(app, inner.x, inner.y + row), "painted cursor");
                    }
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert_eq!(terminal_cursors(app), []);
                    assert!(painted(app, inner.x, inner.y + 1), "no painted cursor");
                }),
            ),
        ],
        false,
    )
    .await
}

/// Without terminal support the editor keeps drawing every secondary cursor itself.
#[tokio::test(flavor = "multi_thread")]
async fn without_terminal_support_the_editor_draws_secondary_cursors() -> anyhow::Result<()> {
    let mut app = app(THREE_LINES, false)?;
    let inner = inner_area(&app);
    test_key_sequence(
        &mut app,
        Some("i"),
        Some(&|app| {
            assert_eq!(terminal_cursors(app), []);
            for row in [1, 2] {
                assert!(painted(app, inner.x, inner.y + row), "no painted cursor");
            }
        }),
        false,
    )
    .await
}

/// A secondary cursor under something drawn over the views (here the completion menu, which
/// opens below the primary cursor, covering the next three lines from the cursor's column on)
/// is not drawn by the terminal: kitty would draw it on top of the menu. The one the menu
/// leaves free still is.
#[tokio::test(flavor = "multi_thread")]
async fn secondary_cursors_under_the_completion_menu_are_left_out() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("hello"), "")?;
    let mut app = app("./he#[|\n]#./he#(|\n)#./he\n./he\n./he#(|\n)#", true)?;
    let (_, doc) = helix_view::current!(app.editor);
    doc.set_path(Some(&dir.path().join("buffer")));
    let inner = inner_area(&app);

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("i"),
                Some(&|app| {
                    assert_eq!(
                        terminal_cursors(app),
                        [(inner.x + 4, inner.y + 1), (inner.x + 4, inner.y + 4)]
                    );
                }),
            ),
            (
                Some("<C-x>"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let menu = &rows[(inner.y + 2) as usize];
                    assert!(
                        menu.contains("hello"),
                        "no menu below the cursor:\n{rows:#?}"
                    );
                    assert_eq!(terminal_cursors(app), [(inner.x + 4, inner.y + 4)]);
                }),
            ),
            (
                Some("<C-c>"),
                Some(&|app| {
                    assert!(app.editor.mode() == helix_view::document::Mode::Insert);
                    assert_eq!(
                        terminal_cursors(app),
                        [(inner.x + 4, inner.y + 1), (inner.x + 4, inner.y + 4)]
                    );
                }),
            ),
        ],
        false,
    )
    .await
}

/// While a prompt has the terminal cursor, the editor draws all of its cursors itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_takes_the_secondary_cursors_back() -> anyhow::Result<()> {
    let mut config = helpers::test_config();
    config.editor.cursor_shape =
        toml::from_str::<CursorShapeConfig>("normal = \"bar\"\ninsert = \"bar\"")?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_input_text(THREE_LINES)
        .build()?;
    app.editor.kitty_multi_cursor_support = true;
    let inner = inner_area(&app);

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("l"),
                Some(&|app| {
                    assert_eq!(
                        terminal_cursors(app),
                        [(inner.x + 1, inner.y + 1), (inner.x + 1, inner.y + 2)]
                    );
                }),
            ),
            (
                Some(":"),
                Some(&|app| {
                    assert_eq!(terminal_cursors(app), []);
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert_eq!(
                        terminal_cursors(app),
                        [(inner.x + 1, inner.y + 1), (inner.x + 1, inner.y + 2)]
                    );
                }),
            ),
        ],
        false,
    )
    .await
}
