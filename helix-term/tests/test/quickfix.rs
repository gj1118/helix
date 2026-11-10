//! The quickfix panel (`[editor.quickfix]`): lists of locations at the bottom of the editor.
#![cfg(not(windows))]

use super::*;
use helix_term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;

fn config(toml: &str) -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(toml).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// The panel on, without notifications (their popups would sit over the code).
fn enabled() -> Config {
    config("[editor.quickfix]\nenable = true\n[editor.notifications]\nenable = false\n")
}

async fn send(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    let mut stream = UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// Thirty lines, `row 00` to `row 29`.
fn rows() -> String {
    (0..30).map(|i| format!("row {i:02}\n")).collect()
}

/// The zero-based line of the cursor.
fn cursor_line(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    text.char_to_line(doc.selection(view.id).primary().cursor(text))
}

/// The bottom (exclusive) of the focused view.
fn view_bottom(app: &Application) -> u16 {
    helix_view::view!(app.editor).area.bottom()
}

/// An app on 30 lines, with lines 3, 10 and 20 (`row 02`, `row 09`, `row 19`) in the quicklist and the cursor back on the
/// first line.
async fn app_with_quicklist(
    config: Config,
) -> anyhow::Result<(Application, tempfile::NamedTempFile)> {
    let file = helpers::temp_file_with_contents(rows())?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    app.resize_terminal(80, 30).await;
    send(&mut app, "3gg<A-q>10gg<A-q>20gg<A-q>gg").await?;
    assert_eq!(app.editor.quicklist.len(), 3);
    assert_eq!(cursor_line(&app), 0);
    Ok((app, file))
}

/// With `enable`, a list of locations opens in the panel at the bottom: the views give it
/// their bottom rows (`height`, 12), it has the focus, and `Enter` visits an entry and gives
/// the focus to the code while the panel stays. `Ctrl-w j` focuses it again, `]q` visits the
/// next entry from the code, `q` closes it and `:copen` shows it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_list_of_locations_opens_in_the_panel() -> anyhow::Result<()> {
    let (mut app, _file) = app_with_quicklist(enabled()).await?;
    // the screen is 30 rows: 29 for the views above the command line
    assert_eq!(view_bottom(&app), 29);

    send(&mut app, "<space>q").await?;
    assert_eq!(
        app.editor.quickfix_panel,
        helix_view::quicklist::QuickfixPanel {
            visible: true,
            focused: true
        }
    );
    // 12 rows at the bottom of the views
    assert_eq!(view_bottom(&app), 17);
    let rows = app.rendered_rows();
    assert!(rows[17].contains("Quicklist"), "{rows:#?}");
    assert!(
        rows[17..29].iter().any(|row| row.contains("row 02")),
        "{rows:#?}"
    );
    assert!(
        rows[17..29].iter().any(|row| row.contains("row 19")),
        "{rows:#?}"
    );
    assert!(
        rows[17..29].iter().any(|row| row.contains("3/3")),
        "{rows:#?}"
    );

    // Enter: to the first entry, the focus to the code (`j` moves the cursor there)
    send(&mut app, "<ret>").await?;
    assert_eq!(cursor_line(&app), 2);
    assert!(app.editor.quickfix_panel.visible);
    assert!(!app.editor.quickfix_panel.focused);
    send(&mut app, "j").await?;
    assert_eq!(cursor_line(&app), 3);

    // Ctrl-w j: back to the panel, where `j` moves through the list
    send(&mut app, "<C-w>j").await?;
    assert!(app.editor.quickfix_panel.focused);
    send(&mut app, "j<ret>").await?;
    assert_eq!(cursor_line(&app), 9);

    // ]q from the code: the next entry of the panel
    send(&mut app, "]q").await?;
    assert_eq!(cursor_line(&app), 19);
    let (status, _) = app.editor.get_status().expect("a status");
    assert_eq!(status, "Quickfix 3/3");

    // q closes it, and the views have their rows back
    send(&mut app, "<C-w>jq").await?;
    assert!(!app.editor.quickfix_panel.visible);
    assert_eq!(view_bottom(&app), 29);
    assert!(!app
        .rendered_rows()
        .iter()
        .any(|row| row.contains("Quicklist")));

    // :copen shows it again, focused
    send(&mut app, ":copen<ret>").await?;
    assert!(app.editor.quickfix_panel.visible && app.editor.quickfix_panel.focused);
    assert_eq!(view_bottom(&app), 17);
    assert!(app.rendered_rows()[17].contains("Quicklist"));
    send(&mut app, ":cclose<ret>").await?;
    assert!(!app.editor.quickfix_panel.visible);

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// In the panel, `/` filters the list (the keys edit the query), and `Esc` (or `Enter`) gives
/// the keys back to the list; `Esc` in the list closes the panel.
#[tokio::test(flavor = "multi_thread")]
async fn slash_filters_the_list_in_the_panel() -> anyhow::Result<()> {
    let (mut app, _file) = app_with_quicklist(enabled()).await?;
    send(&mut app, "<space>q").await?;

    // after `/`, the keys are typed (`j` would move through the list): the text column
    send(&mut app, "/%text row 19").await?;
    let panel: Vec<String> = app.rendered_rows()[17..29].to_vec();
    assert!(panel.iter().any(|row| row.contains("1/3")), "{panel:#?}");
    assert!(
        !panel.iter().any(|row| row.contains("row 02")),
        "{panel:#?}"
    );
    // the cursor is in the query
    let area = app.compositor.size();
    let (cursor, kind) = app.compositor.cursor(area, &app.editor);
    assert!(cursor.is_some() && kind != helix_view::graphics::CursorKind::Hidden);

    // Esc: the list again (the query stays), Enter visits the entry left
    send(&mut app, "<esc><ret>").await?;
    assert_eq!(cursor_line(&app), 19);
    assert!(app.editor.quickfix_panel.visible);

    // Esc in the list closes the panel
    send(&mut app, "<C-w>j<esc>").await?;
    assert!(!app.editor.quickfix_panel.visible);

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// While the panel has the focus, the selected location is previewed over the code; with the
/// focus in the code, the code is shown.
#[tokio::test(flavor = "multi_thread")]
async fn the_panel_previews_over_the_code_while_focused() -> anyhow::Result<()> {
    let (mut app, _file) = app_with_quicklist(enabled()).await?;
    send(&mut app, "<space>q").await?;
    let focused = app.rendered_rows();
    // the preview's frame on the first row, not the code
    assert!(!focused[0].contains("row 00"), "{focused:#?}");

    send(&mut app, "<C-w>k").await?;
    assert!(!app.editor.quickfix_panel.focused);
    let unfocused = app.rendered_rows();
    assert!(unfocused[0].contains("row 00"), "{unfocused:#?}");
    // the panel is still there
    assert!(unfocused[17].contains("Quicklist"));

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

/// Without `enable` (the default), the lists stay pickers.
#[tokio::test(flavor = "multi_thread")]
async fn without_enable_the_lists_are_pickers() -> anyhow::Result<()> {
    let config = config("[editor.notifications]\nenable = false\n");
    assert!(!config.editor.quickfix.enable);
    let (mut app, _file) = app_with_quicklist(config).await?;
    send(&mut app, "<space>q").await?;
    assert!(!app.editor.quickfix_panel.visible);
    assert_eq!(view_bottom(&app), 29);
    // a picker over the editor: its title near the top
    let rows = app.rendered_rows();
    let title = rows
        .iter()
        .position(|row| row.contains("Quicklist"))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(title < 5, "{rows:#?}");
    // Enter visits and closes the picker (its column header goes)
    send(&mut app, "<ret>").await?;
    assert_eq!(cursor_line(&app), 2);
    assert!(!app
        .rendered_rows()
        .iter()
        .any(|row| row.contains("location")));

    // :copen asks for the panel, whatever `enable` says
    send(&mut app, ":copen<ret>").await?;
    assert!(app.editor.quickfix_panel.visible);
    assert_eq!(view_bottom(&app), 17);

    send(&mut app, ":q!<ret>").await?;
    Ok(())
}

#[test]
fn the_panel_is_off_by_default() {
    let config = helix_view::editor::Config::default();
    let quickfix: helix_view::editor::QuickfixConfig = config.quickfix;
    assert!(!quickfix.enable);
    assert_eq!(quickfix.height, 12);
    assert!(quickfix.preview);
}

/// A global search in the panel starts with its query to type (it lists nothing before), the
/// other lists with the list.
#[tokio::test(flavor = "multi_thread")]
async fn a_global_search_in_the_panel_starts_with_its_query() -> anyhow::Result<()> {
    let (mut app, _file) = app_with_quicklist(enabled()).await?;
    let cursor_kind = |app: &Application| {
        let area = app.compositor.size();
        app.compositor.cursor(area, &app.editor).1
    };

    send(&mut app, "<space>/").await?;
    assert!(app.editor.quickfix_panel.focused);
    let rows = app.rendered_rows();
    assert!(rows[17].contains("Global Search"), "{rows:#?}");
    assert_ne!(cursor_kind(&app), helix_view::graphics::CursorKind::Hidden);

    // the quicklist replaces it in the panel, with the list to move through
    send(&mut app, "<esc><esc><space>q").await?;
    assert!(app.editor.quickfix_panel.visible);
    assert!(app.rendered_rows()[17].contains("Quicklist"));
    assert_eq!(cursor_kind(&app), helix_view::graphics::CursorKind::Hidden);

    send(&mut app, "<C-w>k:q!<ret>").await?;
    Ok(())
}

/// The distinct colors of row `y` from column `from` to `to` (exclusive).
fn colors(app: &Application, y: u16, from: u16, to: u16) -> Vec<helix_view::graphics::Color> {
    let mut colors: Vec<helix_view::graphics::Color> = Vec::new();
    for x in from..to {
        let color = app.rendered_fg(x, y);
        if !colors.contains(&color) {
            colors.push(color);
        }
    }
    colors
}

/// With `gradient-borders`, the panel and its preview over the code keep plain borders (they
/// are part of the editor's frame), while a picker over the editor has its gradient.
#[tokio::test(flavor = "multi_thread")]
async fn the_panel_has_no_gradient_border() -> anyhow::Result<()> {
    let gradient = "[editor.gradient-borders]\nenable = true\n";
    let with_panel = format!("{gradient}[editor.quickfix]\nenable = true\n");
    let (mut app, _file) = app_with_quicklist(config(&with_panel)).await?;
    send(&mut app, "<space>q").await?;
    assert!(app.editor.quickfix_panel.focused);
    // along the top border of the panel (after its title) and of the preview: one color, the
    // same
    let panel = colors(&app, 17, 30, 80);
    let preview = colors(&app, 0, 0, 80);
    assert_eq!(panel.len(), 1, "{panel:?}");
    assert_eq!(preview, panel);
    send(&mut app, ":q!<ret>").await?;

    // a picker over the editor: its gradient
    let (mut app, _file) = app_with_quicklist(config(gradient)).await?;
    send(&mut app, "<space>q").await?;
    let rows = app.rendered_rows();
    let top = rows
        .iter()
        .position(|row| row.contains("Quicklist"))
        .expect("the picker") as u16;
    let border = colors(&app, top, 30, 80);
    assert!(border.len() > 2, "{border:?}");
    send(&mut app, "<esc>:q!<ret>").await?;
    Ok(())
}
