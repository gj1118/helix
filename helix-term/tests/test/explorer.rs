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

    // the folder on the first row, then a blank row (not a rule) before the tree
    assert!(
        rows[0].contains("/tmp") || rows[0].contains(".tmp"),
        "{text}"
    );
    assert!(rows[1].trim().is_empty(), "{text}");
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
async fn ctrl_h_opens_then_moves_between_tree_and_buffer_repeatedly() -> anyhow::Result<()> {
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
