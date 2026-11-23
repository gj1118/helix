use super::*;
use helix_term::{
    application::Application,
    ui::explorer::{Explorer, EXPLORER_ID},
};

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Ada",
            "-c",
            "user.email=ada@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A repository with `a.txt` and `sub/b.txt` modified and `new.txt` untracked.
fn repo() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    git(&root, &["init", "-q"]);
    std::fs::create_dir(root.join("sub"))?;
    std::fs::write(root.join("a.txt"), "a\n")?;
    std::fs::write(root.join("sub/b.txt"), "b\n")?;
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init"]);
    std::fs::write(root.join("a.txt"), "changed\n")?;
    std::fs::write(root.join("sub/b.txt"), "changed\n")?;
    std::fs::write(root.join("new.txt"), "n\n")?;
    Ok((dir, root))
}

fn config(explorer: &str) -> Config {
    let raw: helix_term::config::ConfigRaw =
        toml::from_str(&format!("[editor.explorer]\n{explorer}\n")).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// Shows the explorer on `root` without going through the workspace lookup.
fn open_explorer(app: &mut Application, root: &std::path::Path) {
    app.editor.explorer_panel.visible = true;
    app.editor.explorer_panel.focused = true;
    let explorer = Explorer::new(root, &app.editor).unwrap();
    app.compositor.push(Box::new(explorer));
    assert!(app.compositor.find_id::<Explorer>(EXPLORER_ID).is_some());
}

/// The default width of the pinned panel.
const PANEL_WIDTH: usize = 30;

/// The explorer's rows (the panel, without the editor beside it), after expanding `sub`.
async fn explorer_rows(explorer_config: &str) -> anyhow::Result<Vec<String>> {
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config(explorer_config))
        .build()?;
    open_explorer(&mut app, &root);

    let rows = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = rows.clone();
    test_key_sequences(
        &mut app,
        vec![
            // idle rounds while git runs: a slow machine needs plenty
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            // the first folder, expanded
            (Some("j"), None),
            (
                Some("l"),
                Some(&move |app| {
                    *out.lock().unwrap() = app
                        .rendered_rows()
                        .iter()
                        .map(|row| row.chars().take(PANEL_WIDTH).collect::<String>())
                        .collect();
                }),
            ),
        ],
        false,
    )
    .await?;
    let rows = rows.lock().unwrap().clone();
    Ok(rows)
}

fn row<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle}:\n{}", rows.join("\n")))
}

fn column(row: &str, needle: &str) -> usize {
    row[..row.find(needle).unwrap()].chars().count()
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_files_and_their_folders_are_marked() -> anyhow::Result<()> {
    let rows = explorer_rows("").await?;
    let text = rows.join("\n");

    // the default icon set is off: letters
    assert!(row(&rows, "a.txt").trim_end().ends_with('M'), "{text}");
    assert!(row(&rows, "new.txt").trim_end().ends_with('?'), "{text}");
    assert!(row(&rows, "b.txt").trim_end().ends_with('M'), "{text}");
    // the folder with a change in it
    assert!(row(&rows, "sub").trim_end().ends_with('•'), "{text}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_status_can_be_turned_off() -> anyhow::Result<()> {
    let rows = explorer_rows("git-status = false").await?;
    let text = rows.join("\n");
    assert!(!row(&rows, "a.txt").trim_end().ends_with('M'), "{text}");
    assert!(!row(&rows, "sub").contains('•'), "{text}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tree_has_lines_and_room_under_the_header() -> anyhow::Result<()> {
    let rows = explorer_rows("").await?;
    let text = rows.join("\n");

    // three columns per level, with the lines of the tree
    assert!(row(&rows, "sub").contains("├─ sub"), "{text}");
    assert!(row(&rows, "b.txt").contains("│  └─ b.txt"), "{text}");
    assert!(row(&rows, "new.txt").contains("└─ new.txt"), "{text}");
    let sub = column(row(&rows, "sub"), "sub");
    let nested = column(row(&rows, "b.txt"), "b.txt");
    assert_eq!(nested - sub, 3, "{text}");

    // the filter box on the first three rows, the tree right under it
    assert!(rows[0].contains(" Explorer "), "{text}");
    assert!(rows[1].contains("> "), "{text}");
    assert!(rows[3].starts_with("..."), "the parent row first:\n{text}");
    assert!(rows[4].starts_with(".tmp"), "then the root:\n{text}");
    Ok(())
}

/// The filter box at the top of the panel: titled, with the prompt, the filter and a count.
/// Without a filter the count is the entries listed; while filtering, the entries matching out
/// of those searched. A click anywhere in the box starts typing a filter.
#[tokio::test(flavor = "multi_thread")]
async fn the_filter_box_shows_the_filter_and_a_count() -> anyhow::Result<()> {
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    let rows_of = |app: &Application| -> Vec<String> {
        app.rendered_rows()
            .iter()
            .map(|row| row.chars().take(PANEL_WIDTH).collect())
            .collect()
    };

    // Keys sent without quitting (`test_key_sequences` quits at its end), as the scan needs
    // waiting for in the middle.
    async fn send(app: &mut Application, keys: &str) -> anyhow::Result<()> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for key in helix_view::input::parse_macro(keys)? {
            tx.send(Ok(termina::Event::Key(key.into())))?;
        }
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        app.event_loop_until_idle(&mut stream).await;
        Ok(())
    }

    send(&mut app, "<C-n><C-p>").await?;
    {
        let rows = rows_of(&app);
        let text = rows.join("\n");
        // in the middle of the top border
        let (left, right) = rows[0].split_once(" Explorer ").expect(&text);
        let (left, right) = (left.chars().count(), right.chars().count());
        assert!(left.abs_diff(right) <= 1, "{text}");
        // the root's `sub`, `a.txt` and `new.txt`
        assert!(rows[1].trim_end().ends_with("3/3 ▕"), "{text}");
        // the empty box says how to search, wildcards included
        assert!(rows[1].contains("▏ > / to search (*.rs) "), "{text}");
    }

    send(&mut app, "/").await?;
    let rows = rows_of(&app);
    assert!(rows[1].contains("> name or *.rs"), "{}", rows.join("\n"));

    // The count while filtering is of the entries scanned: wait for the scan (in a thread,
    // whose speed varies from machine to machine) before checking it.
    send(&mut app, "tx").await?;
    app.compositor
        .find_id::<Explorer>(EXPLORER_ID)
        .expect("the explorer")
        .wait_for_scan(&app.editor);
    let size = app.compositor.size();
    app.resize_terminal(size.width, size.height).await;
    {
        let rows = rows_of(&app);
        let text = rows.join("\n");
        // `a.txt`, `sub/b.txt` and `new.txt` out of the four entries
        assert!(rows[1].contains("> tx"), "{text}");
        assert!(!rows[1].contains("*.rs"), "{text}");
        assert!(rows[1].trim_end().ends_with("3/4 ▕"), "{text}");
        // the cursor is after the filter, in the box
        let area = app.compositor.size();
        let (cursor, _) = app.compositor.cursor(area, &app.editor);
        let cursor = cursor.expect("a cursor in the filter box");
        let column = rows[1].split_once("> tx").unwrap().0.chars().count() + 4;
        assert_eq!((cursor.row, cursor.col), (1, column), "{text}");
    }

    // Escape drops the filter: all listed again (`sub` stays open, as the cursor's file is
    // in it)
    send(&mut app, "<esc>").await?;
    {
        let rows = rows_of(&app);
        let text = rows.join("\n");
        let count = rows[1].trim_end().trim_end_matches('▕').trim_end();
        let count = count.rsplit(' ').next().unwrap();
        let (shown, total) = count.split_once('/').expect(&text);
        let listed = rows[5..]
            .iter()
            .filter(|row| !row.trim().is_empty())
            .count();
        assert_eq!(
            (shown, total),
            (&*listed.to_string(), &*listed.to_string()),
            "{text}"
        );
    }

    send(&mut app, "<esc><C-l>:q!<ret>").await?;
    Ok(())
}

/// A panel too short for the box keeps the one-line header: the folder, or the filter.
#[tokio::test(flavor = "multi_thread")]
async fn a_short_panel_has_a_filter_line() -> anyhow::Result<()> {
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    app.resize_terminal(120, 7).await;
    let rows = app.rendered_rows();
    let text = rows.join("\n");
    assert!(rows[0].contains(".tmp"), "{text}");
    assert!(!text.contains(" Explorer "), "{text}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn indent_guides_can_be_turned_off() -> anyhow::Result<()> {
    let rows = explorer_rows("indent-guides = false").await?;
    let text = rows.join("\n");
    // two columns per level, without lines
    let sub = column(row(&rows, "sub"), "sub");
    let nested = column(row(&rows, "b.txt"), "b.txt");
    assert_eq!(nested - sub, 2, "{text}");
    for glyph in ['├', '└', '─'] {
        assert!(
            !rows.iter().any(|row| row.chars().any(|c| c == glyph)),
            "{glyph} in the panel:\n{text}"
        );
    }
    Ok(())
}

/// The guide lines of the selected row are `ui.explorer.tree.selected`, those of its ancestors
/// `ui.explorer.path` and those of the other rows `ui.explorer.tree`.
#[tokio::test(flavor = "multi_thread")]
async fn the_selected_row_has_its_own_guide_style() -> anyhow::Result<()> {
    use helix_view::graphics::Color;
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config("git-status = false"))
        .build()?;
    app.editor.theme = helix_view::Theme::from(toml::from_str::<toml::Value>(
        r##"
        "ui.selection" = { bg = "#333333" }
        "ui.explorer.tree" = { fg = "#0000ff" }
        "ui.explorer.tree.selected" = { fg = "#ff0000" }
        "ui.explorer.path" = { fg = "#00ff00" }
        "##,
    )?);
    open_explorer(&mut app, &root);
    let (red, green, blue) = (
        Color::Rgb(255, 0, 0),
        Color::Rgb(0, 255, 0),
        Color::Rgb(0, 0, 255),
    );
    test_key_sequences(
        &mut app,
        vec![
            // expand `sub` and select `sub/b.txt`
            (Some("jl"), None),
            (
                Some("j"),
                Some(&move |app| {
                    let rows = app.rendered_rows();
                    let y = |needle: &str| {
                        rows.iter()
                            .position(|row| row.contains(needle))
                            .unwrap_or_else(|| panic!("no {needle}:\n{}", rows.join("\n")))
                            as u16
                    };
                    let x = |row: u16, glyph: char| {
                        rows[row as usize].chars().position(|c| c == glyph).unwrap() as u16
                    };
                    let (selected, parent, sibling) = (y("b.txt"), y("sub"), y("new.txt"));
                    // the selected row: both of its guide blocks
                    assert_eq!(app.rendered_fg(x(selected, '│'), selected), red);
                    assert_eq!(app.rendered_fg(x(selected, '└'), selected), red);
                    // its folder is on the path, a sibling of the folder is not
                    assert_eq!(app.rendered_fg(x(parent, '├'), parent), green);
                    assert_eq!(app.rendered_fg(x(sibling, '└'), sibling), blue);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// `C-h` and `C-l` move focus between the tree and the buffer, also while the tree's
/// filter box is being typed into.
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_h_and_ctrl_l_move_focus_between_tree_and_buffer() -> anyhow::Result<()> {
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<C-l>"),
                Some(&|app| {
                    assert!(
                        !app.editor.explorer_panel.focused,
                        "C-l did not leave the tree"
                    );
                }),
            ),
            (
                Some("<C-h>"),
                Some(&|app| {
                    assert!(
                        app.editor.explorer_panel.focused,
                        "C-h did not focus the tree"
                    );
                }),
            ),
            (Some("/"), None),
            (
                Some("<C-l>"),
                Some(&|app| {
                    assert!(
                        !app.editor.explorer_panel.focused,
                        "C-l was swallowed by the filter box"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// From a hidden explorer: `C-h` opens and focuses the tree, `C-l` moves to the buffer and
/// `C-h` moves back, again and again. The panel stays open throughout.
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_h_and_ctrl_l_move_between_tree_and_buffer_repeatedly() -> anyhow::Result<()> {
    let (_dir, _root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// The panel is open and the tree has (or has not) the keyboard.
fn assert_tree_focus(app: &Application, want_tree: bool) {
    let panel = &app.editor.explorer_panel;
    assert!(panel.visible, "panel was hidden");
    assert_eq!(panel.focused, want_tree, "focus is in the wrong place");
}

/// Opened with `<space>x`, then `C-l` / `C-h` back and forth, and `C-h` pressed while the
/// tree already has focus.
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_keys_after_space_x_open_and_while_tree_focused() -> anyhow::Result<()> {
    let (_dir, _root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>x"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// A folder with 30 files, for moving through more rows than a page.
fn many_files() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    for i in 0..30 {
        std::fs::write(root.join(format!("f{i:02}.txt")), "x\n")?;
    }
    Ok((dir, root))
}

fn tree(app: &Application) -> (usize, usize) {
    let explorer = app
        .compositor
        .get_id::<Explorer>(EXPLORER_ID)
        .expect("the explorer is open");
    (explorer.cursor_row(), explorer.row_count())
}

fn assert_row(app: &Application, want: usize) {
    assert_eq!(tree(app).0, want, "cursor row");
}

/// Moving in the tree goes through the editor's keymap: `gg`, `ge`, `j`/`k` with counts,
/// `5gg` and paging behave like they do in a buffer, and move the tree, not the buffer.
#[tokio::test(flavor = "multi_thread")]
async fn helix_motions_move_the_tree() -> anyhow::Result<()> {
    let (_dir, root) = many_files()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ge"),
                Some(&|app| {
                    let (row, len) = tree(app);
                    assert!(len >= 30, "{len} rows");
                    assert_eq!(row, len - 1, "ge goes to the last row");
                }),
            ),
            (Some("gg"), Some(&|app| assert_row(app, 0))),
            (Some("j"), Some(&|app| assert_row(app, 1))),
            (Some("3j"), Some(&|app| assert_row(app, 4))),
            (Some("2k"), Some(&|app| assert_row(app, 2))),
            (Some("<down><down>"), Some(&|app| assert_row(app, 4))),
            (Some("<up>"), Some(&|app| assert_row(app, 3))),
            (Some("10gg"), Some(&|app| assert_row(app, 9))),
            (
                Some("999gg"),
                Some(&|app| {
                    let (row, len) = tree(app);
                    assert_eq!(row, len - 1, "a count past the end stops at the last row");
                }),
            ),
            (
                Some("gg<C-d>"),
                Some(&|app| {
                    let row = tree(app).0;
                    assert!(row > 0, "C-d moved down, at {row}");
                }),
            ),
            (Some("<C-u>"), Some(&|app| assert_row(app, 0))),
            (
                Some("G"),
                Some(&|app| {
                    let (row, len) = tree(app);
                    assert_eq!(row, len - 1, "G still goes to the last row");
                }),
            ),
            // with a count, `G` goes to that row (`goto_line`), like in a buffer
            (Some("5G"), Some(&|app| assert_row(app, 4))),
            (Some("<home>"), Some(&|app| assert_row(app, 0))),
            (
                Some("ge"),
                Some(&|app| {
                    // The buffer behind the tree did not move.
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let cursor = doc
                        .selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..));
                    assert_eq!(cursor, 0, "the buffer's cursor moved");
                    assert!(app.editor.explorer_panel.focused);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// A count typed before one of the tree's own keys is dropped instead of carrying over to
/// the next key; with the buffer focused, the motions move the buffer as usual.
/// A terminal sends a typed uppercase letter with SHIFT set (`G` is `Char('G')` + SHIFT),
/// unlike `parse_macro("G")`. The tree's uppercase keys work when typed that way.
#[tokio::test(flavor = "multi_thread")]
async fn uppercase_keys_typed_with_shift_reach_the_tree() -> anyhow::Result<()> {
    use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};

    async fn typed(app: &mut Application, keys: &str) -> anyhow::Result<()> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for ch in keys.chars() {
            let modifiers = if ch.is_ascii_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            let key = KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
            };
            tx.send(Ok(termina::Event::Key(key.into())))?;
        }
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        app.event_loop_until_idle(&mut stream).await;
        Ok(())
    }

    let (_dir, root) = many_files()?;
    std::fs::write(root.join(".hidden"), "h\n")?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    let rows = tree(&app).1;

    typed(&mut app, "G").await?;
    assert_row(&app, rows - 1);

    typed(&mut app, "gg5G").await?;
    assert_row(&app, 4);
    assert!(app.editor.count.is_none(), "the count was used up");

    let hidden = app.editor.explorer_show_hidden();
    typed(&mut app, "H").await?;
    assert_eq!(app.editor.explorer_show_hidden(), !hidden);
    assert_eq!(tree(&app).1, if hidden { rows - 1 } else { rows + 1 });
    typed(&mut app, "H").await?;
    assert_eq!(app.editor.explorer_show_hidden(), hidden);
    assert_eq!(tree(&app).1, rows);

    typed(&mut app, "R").await?;
    assert_eq!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("Explorer refreshed")
    );

    // the buffer behind the tree did not move
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(cursor, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_do_not_leak_and_the_buffer_keeps_its_motions() -> anyhow::Result<()> {
    let (_dir, root) = many_files()?;
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\ntwo\nthree\nfour\n")
        .with_config(config(""))
        .build()?;
    open_explorer(&mut app, &root);
    test_key_sequences(
        &mut app,
        vec![
            // `3` then `-` (the tree's parent-directory key): the count is dropped
            (Some("3-"), Some(&|app| assert!(app.editor.count.is_none()))),
            (Some("ggj"), Some(&|app| assert_row(app, 1))),
            // back in the buffer, `ge` moves the buffer's cursor and not the tree's
            (
                Some("<C-l>ge"),
                Some(&|app| {
                    assert!(!app.editor.explorer_panel.focused);
                    assert_row(app, 1);
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let text = doc.text().slice(..);
                    let line = text.char_to_line(doc.selection(view.id).primary().cursor(text));
                    assert!(line >= 3, "ge moved the buffer to its last line, at {line}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// How the floating window's size is expected to come out.
#[derive(Clone, Copy)]
enum FloatSize {
    /// No size configured: the size of the file picker without its preview, 75% x 90%
    /// of the screen.
    Picker,
    /// The configured size.
    Exactly(usize, usize),
}

/// The floating window is sized like the file picker without its preview by default, so it
/// follows the screen; a configured size is used as is, at most that size. It is always
/// centered.
#[tokio::test(flavor = "multi_thread")]
async fn floating_window_is_centered_and_sized_like_the_pickers() -> anyhow::Result<()> {
    for (explorer_toml, size) in [
        ("", FloatSize::Picker),
        (
            "floating-width = 30\nfloating-height = 10",
            FloatSize::Exactly(30, 10),
        ),
        (
            "floating-width = 500\nfloating-height = 500",
            FloatSize::Picker,
        ),
    ] {
        let (_dir, root) = repo()?;
        let mut app = helpers::AppBuilder::new()
            .with_config(config(explorer_toml))
            .build()?;
        app.editor.explorer_panel.floating = true;
        open_explorer(&mut app, &root);
        test_key_sequences(
            &mut app,
            vec![(
                // (not Esc: that closes a floating window)
                Some("k"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let corners = ['┌', '╭', '┏', '╔', '🭽'];
                    let grid: Vec<Vec<char>> =
                        rows.iter().map(|row| row.chars().collect()).collect();
                    let at = |y: usize, x: usize| grid.get(y).and_then(|row| row.get(x)).copied();
                    // The frame that holds the tree (a notification has one too).
                    let (top, left, width, height) = (0..grid.len())
                        .flat_map(|y| (0..grid[y].len()).map(move |x| (y, x)))
                        .filter(|&(y, x)| at(y, x).is_some_and(|c| corners.contains(&c)))
                        // measured to the frame's own corners: text drawn next to the
                        // window can touch its border
                        .filter_map(|(y, x)| {
                            let right = (x + 1..grid[y].len())
                                .find(|&i| at(y, i).is_some_and(|c| "┐╮┓╗🭾".contains(c)))?;
                            let bottom = (y + 1..grid.len())
                                .find(|&j| at(j, x).is_some_and(|c| "└╰┗╚🭼".contains(c)))?;
                            Some((y, x, right - x + 1, bottom - y + 1))
                        })
                        .find(|&(y, x, width, height)| {
                            (y..y + height).any(|j| {
                                grid[j]
                                    .iter()
                                    .skip(x)
                                    .take(width)
                                    .collect::<String>()
                                    .contains("a.txt")
                            })
                        })
                        .unwrap_or_else(|| panic!("no explorer window:\n{}", rows.join("\n")));
                    // no pinned panel: the editor spans the screen
                    let screen_width = app.editor.tree.area().width as usize;
                    // without the status line and the command line, like the pickers
                    let screen_height = rows.len() - 2;
                    let (right, bottom) =
                        (screen_width - left - width, screen_height - top - height);
                    let text = rows.join("\n");
                    assert!(
                        left.abs_diff(right) <= 1,
                        "not centered: {left} | {right}\n{text}"
                    );
                    assert!(
                        top.abs_diff(bottom) <= 1,
                        "not centered: {top} | {bottom}\n{text}"
                    );
                    let want = match size {
                        FloatSize::Picker => (screen_width * 3 / 4, screen_height * 9 / 10),
                        FloatSize::Exactly(width, height) => (width, height),
                    };
                    assert_eq!((width, height), want, "{explorer_toml:?}\n{text}");
                }),
            )],
            false,
        )
        .await?;
    }
    Ok(())
}

/// How many cells of the screen are drawn in the primary cursor's colour (red).
fn primary_cursor_cells(app: &Application) -> usize {
    let red = helix_view::graphics::Color::Rgb(255, 0, 0);
    let height = app.rendered_rows().len() as u16;
    (0..height)
        .flat_map(|y| (0..120).map(move |x| (x, y)))
        .filter(|&(x, y)| app.rendered_fg(x, y) == red)
        .count()
}

/// The focused tree hides the terminal cursor so that the editor's cursor disappears: the
/// editor must not draw its own block cursor then. It still does under the command line (both
/// styles) and a picker, which take the terminal cursor away from it.
#[tokio::test(flavor = "multi_thread")]
async fn the_focused_tree_hides_the_editor_cursor() -> anyhow::Result<()> {
    let (_dir, root) = repo()?;
    for style in ["bottom", "popup"] {
        let raw: helix_term::config::ConfigRaw = toml::from_str(&format!(
            "[editor.explorer]\ngit-status = false\n[editor.cmdline]\nstyle = \"{style}\"\n"
        ))?;
        let mut app = helpers::AppBuilder::new()
            .with_config(Config {
                editor: raw.editor.unwrap().try_into().unwrap(),
                ..Default::default()
            })
            .with_input_text("#[h|]#ello\n")
            .build()?;
        app.editor.theme = helix_view::Theme::from(toml::from_str::<toml::Value>(
            r##"
            "ui.selection" = { bg = "#333333" }
            "ui.cursor" = { fg = "#00ff00" }
            "ui.cursor.primary" = { fg = "#ff0000" }
            "##,
        )?);
        open_explorer(&mut app, &root);
        let drawn = |want: bool, what: &'static str| {
            move |app: &Application| {
                let cells = primary_cursor_cells(app);
                assert_eq!(
                    cells > 0,
                    want,
                    "{what} ({style}): {cells} cells in the cursor colour\n{}",
                    app.rendered_rows().join("\n")
                );
            }
        };
        let tree = drawn(false, "focused tree");
        let buffer = drawn(false, "focused buffer");
        let command_line = drawn(true, "command line");
        let picker = drawn(true, "picker");
        test_key_sequences(
            &mut app,
            vec![
                (None, Some(&tree)),
                (Some("<C-l>"), Some(&buffer)),
                (Some(":"), Some(&command_line)),
                (Some("<esc>"), Some(&buffer)),
                (Some("<space>b"), Some(&picker)),
                (Some("<esc><C-h>"), Some(&tree)),
            ],
            false,
        )
        .await?;
    }
    Ok(())
}

/// The info box of pending keys (the `space` menu) is drawn over the explorer, wherever the
/// panel is: pinned on the right, it would cover the box's right half otherwise.
#[tokio::test(flavor = "multi_thread")]
async fn the_key_info_box_is_drawn_over_the_panel() -> anyhow::Result<()> {
    for side in ["right", "floating"] {
        let (_dir, root) = repo()?;
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!("side = \"{side}\"")))
            .build()?;
        open_explorer(&mut app, &root);
        // the keys go to the editor
        app.editor.explorer_panel.focused = false;
        test_key_sequences(
            &mut app,
            vec![(
                Some("<space>"),
                Some(&|app| {
                    let text = app.rendered_rows().join("\n");
                    // an entry reaching to the box's right edge, over the panel
                    assert!(
                        text.contains("Paste clipboard after selections"),
                        "{side}:\n{text}"
                    );
                }),
            )],
            false,
        )
        .await?;
    }
    Ok(())
}

/// `<space>x` / `:explorer` shows the explorer on the side the config sets, read each time
/// it opens, and hides it; `<space>X` / `:explorer-float` shows a floating window instead,
/// and hides it.
#[tokio::test(flavor = "multi_thread")]
async fn the_explorer_opens_on_the_configured_side() -> anyhow::Result<()> {
    use helix_view::explorer::ExplorerSide;

    fn shown(app: &Application) -> Option<ExplorerSide> {
        let panel = &app.editor.explorer_panel;
        panel.visible.then(|| app.editor.explorer_side())
    }

    let mut app = helpers::AppBuilder::new()
        .with_config(config("side = \"right\""))
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>x"),
                Some(&|app| {
                    assert_eq!(shown(app), Some(ExplorerSide::Right));
                    assert!(app.editor.explorer_panel.focused);
                }),
            ),
            (Some("<space>x"), Some(&|app| assert_eq!(shown(app), None))),
            // the side is read when the panel opens
            (
                Some(":set explorer.side left<ret>:explorer<ret>"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Left))),
            ),
            // a floating window over the configured side
            (
                Some("<space>X"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Floating))),
            ),
            // `<space>x` swaps it for the configured side
            (
                Some("<space>x"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Left))),
            ),
            (
                Some("<space>X"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Floating))),
            ),
            (Some("<space>X"), Some(&|app| assert_eq!(shown(app), None))),
            // the closed float leaves the configured side as it was
            (
                Some(":explorer<ret>"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Left))),
            ),
        ],
        false,
    )
    .await?;

    // `side = "floating"`: `<space>x` opens a floating window, and `<space>X` closes it
    let mut app = helpers::AppBuilder::new()
        .with_config(config("side = \"floating\""))
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>x"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Floating))),
            ),
            (Some("<space>X"), Some(&|app| assert_eq!(shown(app), None))),
            (
                Some(":explorer<ret>"),
                Some(&|app| assert_eq!(shown(app), Some(ExplorerSide::Floating))),
            ),
            (Some("<space>x"), Some(&|app| assert_eq!(shown(app), None))),
        ],
        false,
    )
    .await?;

    // the side commands are gone: the config sets the side
    let mut app = helpers::AppBuilder::new().build()?;
    test_key_sequences(
        &mut app,
        vec![(
            Some(":explorer-left<ret>"),
            Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.contains("Command not found"), "{status}");
                assert_eq!(shown(app), None);
            }),
        )],
        false,
    )
    .await?;
    Ok(())
}

/// `C-w h` / `C-w l` move between the views and the pinned explorer by direction, on
/// whichever side it is: past the last view towards it, and away from its edge back to the
/// code. Up and down from the tree do nothing. `C-h` / `C-l` do not depend on the side:
/// `C-h` to the tree, `C-l` back to the code, from any view.
#[tokio::test(flavor = "multi_thread")]
async fn window_moves_reach_the_explorer_on_its_side() -> anyhow::Result<()> {
    for (side, towards, away) in [("left", "h", "l"), ("right", "l", "h")] {
        let mut app = helpers::AppBuilder::new()
            .with_config(config(&format!("side = \"{side}\"")))
            .build()?;
        let tree_focused = |app: &Application| app.editor.explorer_panel.focused;
        // two views side by side, the focus in the new one (on the left)
        let start = format!("<space>x<C-w>{away}:vsplit<ret>");
        let to_explorer = format!("<C-w>{towards}");
        let back = format!("<C-w>{away}");
        test_key_sequences(
            &mut app,
            vec![
                (
                    Some(&start),
                    Some(&|app| {
                        assert!(!app.editor.explorer_panel.focused);
                        assert_eq!(app.editor.tree.views().count(), 2);
                    }),
                ),
                // past the views to the explorer, whichever view has the focus
                (Some(&to_explorer), None),
                (Some(&to_explorer), Some(&|app| assert!(tree_focused(app)))),
                // the opposite direction is the code again
                (Some(&back), Some(&|app| assert!(!tree_focused(app)))),
                (Some("<C-w>j"), Some(&|app| assert!(!tree_focused(app)))),
                // `C-h` / `C-l`: the same on both sides, from the view next to the explorer
                // too
                (Some("<C-h>"), Some(&|app| assert!(tree_focused(app)))),
                // towards the edge, up and down: the tree keeps the focus
                (Some(&to_explorer), Some(&|app| assert!(tree_focused(app)))),
                (
                    Some("<C-w>j<C-w>k<C-h>"),
                    Some(&|app| assert!(tree_focused(app))),
                ),
                (Some("<C-l>"), Some(&|app| assert!(!tree_focused(app)))),
                (Some(&back), Some(&|app| assert!(!tree_focused(app)))),
                (Some("<C-h>"), Some(&|app| assert!(tree_focused(app)))),
                (Some("<C-l>"), Some(&|app| assert!(!tree_focused(app)))),
                // one view again, for the `:q!` that ends the test
                (Some("<C-w>o"), None),
            ],
            false,
        )
        .await?;
    }

    // a floating explorer is in no direction (`C-h` keeps the focus in it); Esc closes it
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config("side = \"floating\""))
        .build()?;
    open_explorer(&mut app, &root);
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<C-w>l<C-w>h<C-h>"),
                Some(&|app| {
                    let panel = &app.editor.explorer_panel;
                    assert!(panel.visible && panel.focused);
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert!(!app.editor.explorer_panel.visible)),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// A floating window moved or resized with the mouse is centered again at its configured size
/// when it is closed and opened again (`<space>X` twice): there is no command for it.
#[tokio::test(flavor = "multi_thread")]
async fn reopening_the_floating_window_centers_it() -> anyhow::Result<()> {
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config("side = \"floating\""))
        .build()?;
    open_explorer(&mut app, &root);
    // as a drag with the mouse leaves it
    app.editor.explorer_panel.floating_geometry = Some(helix_view::explorer::FloatGeometry {
        x: 2,
        y: 3,
        width: 20,
        height: 8,
    });
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>X"),
                Some(&|app| assert!(!app.editor.explorer_panel.visible)),
            ),
            (
                Some("<space>X"),
                Some(&|app| {
                    let panel = &app.editor.explorer_panel;
                    assert!(panel.visible && panel.focused);
                    assert_eq!(panel.floating_geometry, None);
                }),
            ),
            // the old command is gone
            (
                Some("<esc>:explorer-float-reset<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.contains("Command not found"), "{status}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// The keys to the tree and back can be bound to others (`explorer_focus` /
/// `explorer_blur`), and they work from the tree and its filter box, like `C-h` / `C-l`.
#[tokio::test(flavor = "multi_thread")]
async fn the_focus_keys_can_be_rebound() -> anyhow::Result<()> {
    let raw: helix_term::config::ConfigRaw = toml::from_str(
        r#"
        [editor.explorer]
        side = "right"

        [keys.normal]
        C-e = "explorer_focus"
        C-b = "explorer_blur"
        "#,
    )?;
    let mut keys = helix_term::keymap::default();
    helix_term::keymap::merge_keys(&mut keys, raw.keys.unwrap().modes);
    let config = Config {
        keys,
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    };
    let (_dir, root) = repo()?;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    open_explorer(&mut app, &root);
    test_key_sequences(
        &mut app,
        vec![
            (Some("<C-b>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-e>"), Some(&|app| assert_tree_focus(app, true))),
            // from the filter box too: it stops typing, the filter stays
            (
                Some("/sub<C-b>"),
                Some(&|app| {
                    assert_tree_focus(app, false);
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("sub"), "{rows}");
                }),
            ),
            (Some("<C-e>"), Some(&|app| assert_tree_focus(app, true))),
            // the default keys still work
            (Some("<C-l>"), Some(&|app| assert_tree_focus(app, false))),
            (Some("<C-h>"), Some(&|app| assert_tree_focus(app, true))),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// The app as `hx <dir>` starts it, with `explorer` as the `[editor.explorer]` settings. (The
/// test builder refuses a directory: `main` makes it the working directory, which a test can't.)
fn app_on_directory(dir: &std::path::Path, explorer: &str) -> anyhow::Result<Application> {
    let mut args = helix_term::args::Args::default();
    args.files.insert(dir.to_path_buf(), None);
    let mut config = config(explorer);
    config.keys = helix_term::keymap::default();
    config.editor.lsp.enable = false;
    config.editor.recent_picker.enable = false;
    // as the test builder: no file history (`old-files`)
    config.editor.persistence.old_files = false;
    Application::new(
        args,
        config,
        helpers::test_syntax_loader(None),
        helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
    )
}

/// `hx <dir>` opens a file picker on it, as Helix does, when `open-on-directory` isn't set or
/// is `false`.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_argument_opens_the_picker_by_default() -> anyhow::Result<()> {
    let (_dir, root) = many_files()?;
    for settings in ["", "open-on-directory = false"] {
        let mut app = app_on_directory(&root, settings)?;
        test_key_sequences(
            &mut app,
            vec![(
                None,
                Some(&|app| {
                    assert!(app.compositor.has_id(helix_term::ui::picker::ID));
                    assert!(app.compositor.get_id::<Explorer>(EXPLORER_ID).is_none());
                    assert!(!app.editor.explorer_panel.visible);
                }),
            )],
            false,
        )
        .await?;
    }
    Ok(())
}

/// With `open-on-directory = true`, `hx <dir>` opens the explorer on that directory instead,
/// with the keyboard in the tree.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_argument_opens_the_explorer_when_set() -> anyhow::Result<()> {
    let (_dir, root) = many_files()?;
    let mut app = app_on_directory(&root, "open-on-directory = true")?;
    let expected_root = root.clone();
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    assert!(!app.compositor.has_id(helix_term::ui::picker::ID));
                    let explorer = app
                        .compositor
                        .get_id::<Explorer>(EXPLORER_ID)
                        .expect("the explorer is open");
                    assert_eq!(explorer.root(), expected_root.as_path());
                    // the root's `...` row, then the root, then its 30 files
                    assert!(explorer.row_count() >= 30usize, "{}", explorer.row_count());
                    assert!(app.editor.explorer_panel.visible);
                    assert!(app.editor.explorer_panel.focused);
                    // the buffer beside it is a new, empty one
                    assert_eq!(app.editor.documents().count(), 1usize);
                    assert_eq!(helix_view::doc!(app.editor).path(), None);
                }),
            ),
            // the keyboard is in the tree: `j` moves its cursor
            (
                Some("j"),
                Some(&|app| {
                    let explorer = app
                        .compositor
                        .get_id::<Explorer>(EXPLORER_ID)
                        .expect("the explorer is open");
                    assert_eq!(explorer.cursor_row(), 2usize);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// Sends `keys` without quitting at the end, as `test_key_sequences` does.
async fn send_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    drop(tx);
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

/// Lets the file operations running off the UI thread finish until `done` holds.
async fn wait_until(
    app: &mut Application,
    what: &str,
    done: impl Fn(&Application) -> bool,
) -> anyhow::Result<()> {
    for _ in 0..100 {
        if done(app) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(
            tokio::sync::mpsc::unbounded_channel().1,
        );
        app.event_loop_until_idle(&mut stream).await;
    }
    anyhow::bail!("timed out waiting for {what}")
}

fn explorer(app: &Application) -> &Explorer {
    app.compositor
        .get_id::<Explorer>(EXPLORER_ID)
        .expect("the explorer is open")
}

/// Moves the tree's cursor to the entry named `name`.
async fn goto(app: &mut Application, name: &str) -> anyhow::Result<()> {
    send_keys(app, "gg").await?;
    for _ in 0..50 {
        let here = explorer(app)
            .cursor_path()
            .and_then(|path| path.file_name())
            .is_some_and(|file| file == name);
        if here {
            return Ok(());
        }
        send_keys(app, "j").await?;
    }
    anyhow::bail!("no row named {name}")
}

/// The names of the marked entries.
fn marked(app: &Application) -> Vec<String> {
    explorer(app)
        .marked()
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn panel_rows(app: &Application) -> Vec<String> {
    app.rendered_rows()
        .iter()
        .map(|row| row.chars().take(PANEL_WIDTH).collect())
        .collect()
}

/// A folder with `a.txt` to `d.txt` and the folder `dest`.
fn four_files() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    std::fs::create_dir(root.join("dest"))?;
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        std::fs::write(root.join(name), format!("{name}\n"))?;
    }
    Ok((dir, root))
}

/// `m` marks entries that are not next to each other; the marked ones show a `●` and the
/// header counts them; `d` deletes them all after one confirmation.
#[tokio::test(flavor = "multi_thread")]
async fn marked_entries_are_shown_and_deleted_together() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);

    goto(&mut app, "a.txt").await?;
    // `m` moves down: a marked, b skipped, c marked
    send_keys(&mut app, "mjm").await?;
    assert_eq!(marked(&app), vec!["a.txt".to_string(), "c.txt".to_string()]);
    let cursor: Option<String> = explorer(&app)
        .cursor_path()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned());
    assert_eq!(cursor.as_deref(), Some("d.txt"), "m moved down after c");
    {
        let rows = panel_rows(&app);
        let text = rows.join("\n");
        assert!(row(&rows, "a.txt").contains("● a.txt"), "{text}");
        assert!(row(&rows, "c.txt").contains("● c.txt"), "{text}");
        assert!(!row(&rows, "b.txt").contains('●'), "{text}");
        assert!(rows[0].contains("Explorer · 2 marked"), "{text}");
    }

    // `m` on a marked entry unmarks it, then mark it back
    goto(&mut app, "c.txt").await?;
    send_keys(&mut app, "m").await?;
    assert_eq!(marked(&app), vec!["a.txt".to_string()]);
    goto(&mut app, "c.txt").await?;
    send_keys(&mut app, "m").await?;
    assert_eq!(marked(&app).len(), 2_usize);

    // the confirmation lists both, whatever is under the cursor
    goto(&mut app, "b.txt").await?;
    send_keys(&mut app, "d").await?;
    {
        let text = app.rendered_rows().join("\n");
        assert!(text.contains("Delete 2 entries"), "{text}");
        assert!(
            text.contains("- a.txt") && text.contains("- c.txt"),
            "{text}"
        );
        assert!(
            !text.contains("- b.txt"),
            "the cursor's entry is not deleted:\n{text}"
        );
    }
    send_keys(&mut app, "y").await?;
    wait_until(&mut app, "the delete", |_| {
        !root.join("a.txt").exists() && !root.join("c.txt").exists()
    })
    .await?;
    assert!(root.join("b.txt").is_file(), "b.txt was not marked");
    assert!(root.join("d.txt").is_file(), "d.txt was not marked");
    wait_until(&mut app, "the marks to go", |app| marked(app).is_empty()).await?;
    assert_eq!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("Deleted 2 entries")
    );
    Ok(())
}

/// `M` marks the rows from the last one marked to the cursor; `x` cuts them (the marks go,
/// a `→` shows what will move) and `p` moves them all into the folder under the cursor.
#[tokio::test(flavor = "multi_thread")]
async fn a_range_is_marked_and_moved_together() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);

    goto(&mut app, "a.txt").await?;
    send_keys(&mut app, "m").await?;
    goto(&mut app, "c.txt").await?;
    send_keys(&mut app, "M").await?;
    assert_eq!(
        marked(&app),
        vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "c.txt".to_string()
        ]
    );

    // the root can't be marked
    send_keys(&mut app, "ggj").await?;
    assert_eq!(explorer(&app).cursor_path(), Some(root.as_path()));
    send_keys(&mut app, "m").await?;
    assert_eq!(marked(&app).len(), 3_usize, "the root was marked");

    send_keys(&mut app, "x").await?;
    assert!(marked(&app).is_empty(), "the marks went into the clipboard");
    let clip = app.editor.explorer_clipboard.clone().expect("a clipboard");
    assert!(clip.cut);
    assert_eq!(clip.paths.len(), 3_usize);
    {
        let rows = panel_rows(&app);
        let text = rows.join("\n");
        assert!(row(&rows, "b.txt").contains("→ b.txt"), "{text}");
        assert!(!row(&rows, "d.txt").contains('→'), "{text}");
        assert!(rows[0].contains("Explorer · 3 to move"), "{text}");
    }

    goto(&mut app, "dest").await?;
    send_keys(&mut app, "p").await?;
    wait_until(&mut app, "the move", |app| {
        app.editor.explorer_clipboard.is_none()
    })
    .await?;
    for name in ["a.txt", "b.txt", "c.txt"] {
        assert!(root.join("dest").join(name).is_file(), "{name} moved");
        assert!(!root.join(name).exists(), "{name} left");
    }
    assert!(root.join("d.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join("dest/b.txt"))?,
        "b.txt\n".to_string()
    );
    Ok(())
}

/// Copied entries that would overwrite something are confirmed once, together.
#[tokio::test(flavor = "multi_thread")]
async fn pasting_over_several_entries_asks_once() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    std::fs::write(root.join("dest/a.txt"), "old a\n")?;
    std::fs::write(root.join("dest/b.txt"), "old b\n")?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);

    goto(&mut app, "a.txt").await?;
    send_keys(&mut app, "mmm").await?;
    send_keys(&mut app, "c").await?;
    {
        let rows = panel_rows(&app);
        assert!(
            row(&rows, "c.txt").contains("+ c.txt"),
            "{}",
            rows.join("\n")
        );
    }
    goto(&mut app, "dest").await?;
    send_keys(&mut app, "p").await?;
    {
        let text = app.rendered_rows().join("\n");
        assert!(text.contains("These 2 entries already exist"), "{text}");
    }
    send_keys(&mut app, "y").await?;
    wait_until(&mut app, "the copy", |_| root.join("dest/c.txt").is_file()).await?;
    assert_eq!(
        std::fs::read_to_string(root.join("dest/a.txt"))?,
        "a.txt\n".to_string()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("dest/b.txt"))?,
        "b.txt\n".to_string()
    );
    assert!(root.join("a.txt").is_file(), "a copy keeps the original");
    assert!(
        app.editor.explorer_clipboard.is_some(),
        "a copy can be pasted again"
    );
    Ok(())
}

/// `r` with marked entries asks for each new name in turn, with the cursor on the entry; an
/// unchanged name skips it, and Esc stops, leaving the rest marked.
#[tokio::test(flavor = "multi_thread")]
async fn marked_entries_are_renamed_one_after_the_other() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);

    goto(&mut app, "a.txt").await?;
    send_keys(&mut app, "mmmm").await?;
    assert_eq!(marked(&app).len(), 4_usize);

    send_keys(&mut app, "r").await?;
    {
        let text = app.rendered_rows().join("\n");
        assert!(text.contains("Rename 1 of 4: file a.txt"), "{text}");
    }
    send_keys(&mut app, "<C-u>x.txt<ret>").await?;
    wait_until(&mut app, "the second popup", |app| {
        app.rendered_rows()
            .join("\n")
            .contains("Rename 2 of 4: file b.txt")
    })
    .await?;
    assert!(root.join("x.txt").is_file() && !root.join("a.txt").exists());
    let on_b = explorer(&app)
        .cursor_path()
        .is_some_and(|path| path.ends_with("b.txt"));
    assert!(on_b, "the cursor is on the entry being renamed");

    // unchanged: skipped
    send_keys(&mut app, "<ret>").await?;
    wait_until(&mut app, "the third popup", |app| {
        app.rendered_rows()
            .join("\n")
            .contains("Rename 3 of 4: file c.txt")
    })
    .await?;
    // a name that exists asks again
    send_keys(&mut app, "<C-u>d.txt<ret>").await?;
    wait_until(&mut app, "the same popup again", |app| {
        app.rendered_rows()
            .join("\n")
            .contains("Rename 3 of 4: file c.txt")
    })
    .await?;
    assert!(
        root.join("c.txt").is_file(),
        "c.txt was not renamed over d.txt"
    );
    send_keys(&mut app, "<C-u>z.txt<ret>").await?;
    wait_until(&mut app, "the last popup", |app| {
        app.rendered_rows()
            .join("\n")
            .contains("Rename 4 of 4: file d.txt")
    })
    .await?;

    // Esc: d.txt stays as it is, and marked
    send_keys(&mut app, "<esc>").await?;
    wait_until(&mut app, "the marks", |app| {
        marked(app) == vec!["d.txt".to_string()]
    })
    .await?;
    assert_eq!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("Renamed 2 of 4 entries: stopped")
    );
    for name in ["x.txt", "b.txt", "z.txt", "d.txt"] {
        assert!(root.join(name).is_file(), "{name}");
    }
    for name in ["a.txt", "c.txt"] {
        assert!(!root.join(name).exists(), "{name}");
    }

    // `u` unmarks everything
    send_keys(&mut app, "u").await?;
    assert!(marked(&app).is_empty());
    Ok(())
}

/// Esc unmarks before it clears a filter or leaves the panel.
#[tokio::test(flavor = "multi_thread")]
async fn escape_unmarks_first() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let mut app = helpers::AppBuilder::new().with_config(config("")).build()?;
    open_explorer(&mut app, &root);
    goto(&mut app, "a.txt").await?;
    send_keys(&mut app, "m<esc>").await?;
    assert!(marked(&app).is_empty());
    assert!(
        app.editor.explorer_panel.focused,
        "the tree kept the keyboard"
    );
    send_keys(&mut app, "<esc>").await?;
    assert!(!app.editor.explorer_panel.focused);
    Ok(())
}

/// A config with the explorer's keys of `tree` and of `filter` (key, action).
fn remapped(tree: &[(&str, &str)], filter: &[(&str, &str)]) -> Config {
    use helix_view::component_keys::Component;
    let table = |pairs: &[(&str, &str)]| -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, action)| (key.to_string(), action.to_string()))
            .collect()
    };
    let mut config = config("");
    config
        .editor
        .component_keys
        .extend(Component::Explorer, &table(tree))
        .unwrap();
    config
        .editor
        .component_keys
        .extend(Component::ExplorerFilter, &table(filter))
        .unwrap();
    config
}

/// `[keys.explorer]` moves the tree's own keys: a key bound to an action does it, `no_op` takes
/// a key away (it reaches neither the tree nor the buffer), and the hints show the new keys.
#[tokio::test(flavor = "multi_thread")]
async fn the_tree_keys_can_be_remapped() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let config = remapped(
        &[
            ("space", "toggle_mark"),
            ("m", "no_op"),
            ("D", "delete"),
            ("d", "no_op"),
            ("C-f", "filter"),
            ("/", "no_op"),
        ],
        &[],
    );
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    open_explorer(&mut app, &root);
    goto(&mut app, "a.txt").await?;
    {
        let rows = panel_rows(&app);
        assert!(
            rows[1].contains("> C-f to search (*.rs)"),
            "{}",
            rows.join("\n")
        );
    }

    send_keys(&mut app, "<space>").await?;
    assert_eq!(marked(&app), vec!["a.txt".to_string()]);
    assert_eq!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("Explorer: marked (r rename, D delete, x cut, c copy, u unmark)")
    );

    // `m` and `d` are taken away: nothing marked, no confirmation, the buffer untouched
    send_keys(&mut app, "md").await?;
    assert_eq!(marked(&app), vec!["a.txt".to_string()]);
    assert!(
        !app.rendered_rows().join("\n").contains("Delete file"),
        "d still deletes"
    );
    assert!(app.editor.explorer_panel.focused);
    assert_eq!(app.editor.mode, helix_view::document::Mode::Normal);

    send_keys(&mut app, "D").await?;
    {
        let text = app.rendered_rows().join("\n");
        assert!(text.contains("Delete file a.txt?"), "{text}");
    }
    send_keys(&mut app, "y").await?;
    wait_until(&mut app, "the delete", |_| !root.join("a.txt").exists()).await?;

    // `/` is taken away, `C-f` starts a filter
    send_keys(&mut app, "/").await?;
    assert!(!panel_rows(&app)[1].contains("name or *.rs"));
    send_keys(&mut app, "<C-f>").await?;
    let rows = panel_rows(&app);
    assert!(rows[1].contains("> name or *.rs"), "{}", rows.join("\n"));
    Ok(())
}

/// `[keys.explorer-filter]` moves the filter box's keys.
#[tokio::test(flavor = "multi_thread")]
async fn the_filter_box_keys_can_be_remapped() -> anyhow::Result<()> {
    let (_dir, root) = four_files()?;
    let config = remapped(
        &[],
        &[
            ("C-j", "next_result"),
            ("C-k", "prev_result"),
            ("C-g", "clear"),
            ("esc", "no_op"),
        ],
    );
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    open_explorer(&mut app, &root);

    send_keys(&mut app, "/txt").await?;
    app.compositor
        .find_id::<Explorer>(EXPLORER_ID)
        .expect("the explorer")
        .wait_for_scan(&app.editor);
    send_keys(&mut app, "<C-j>").await?;
    let first = explorer(&app).cursor_row();
    send_keys(&mut app, "<C-j>").await?;
    assert_eq!(explorer(&app).cursor_row(), first + 1, "C-j moves down");
    send_keys(&mut app, "<C-k>").await?;
    assert_eq!(explorer(&app).cursor_row(), first, "C-k moves up");

    // Esc is taken away: still typing the filter
    send_keys(&mut app, "<esc>").await?;
    assert!(panel_rows(&app)[1].contains("> txt"));
    send_keys(&mut app, "<C-g>").await?;
    let rows = panel_rows(&app);
    assert!(!rows[1].contains("> txt"), "{}", rows.join("\n"));
    Ok(())
}
