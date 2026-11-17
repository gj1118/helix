//! Folds on the screen: the fold column (`▾` where a block starts, `▸` on a closed fold), the
//! closed fold's label, a click in the column, and `z o` / `z a` / `z O`.

use super::helpers::{self, AppBuilder};

use helix_term::application::Application;
use helix_view::{current_ref, input::parse_macro};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
#[cfg(not(windows))]
use termina::event::{
    Event, KeyEvent, Modifiers as KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

const TEXT: &str = "fn outer() {\n    if a {\n        one();\n    }\n    two();\n}\nlast();\n";

async fn send_events(app: &mut Application, events: impl IntoIterator<Item = Event>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for event in events {
        tx.send(Ok(event)).unwrap();
    }
    assert!(
        app.event_loop_until_idle(&mut UnboundedReceiverStream::new(rx))
            .await
    );
}

async fn send_keys(app: &mut Application, keys: &str) {
    send_events(
        app,
        parse_macro(keys)
            .unwrap()
            .into_iter()
            .map(|key| Event::Key(KeyEvent::from(key))),
    )
    .await;
}

/// A click in the fold column of the focused window, on the row of `line` (0-based, the
/// window not scrolled and nothing folded above it).
async fn click_fold_column(app: &mut Application, line: u16) {
    let (view, doc) = current_ref!(app.editor);
    let inner = view.inner_area(doc);
    // the fold column is the last gutter, next to the text
    let column = inner.x - 1;
    let row = inner.y + line;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        send_events(
            app,
            [Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::empty(),
            })],
        )
        .await;
    }
}

fn folds(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    doc.fold_container(view.id).map_or(0, |c| c.len())
}

/// The rows of the text area, from the fold column on.
fn text_rows(app: &Application) -> Vec<String> {
    let (view, doc) = current_ref!(app.editor);
    let inner = view.inner_area(doc);
    let rows = app.rendered_rows();
    rows[inner.y as usize..(inner.y as usize + 7)]
        .iter()
        // (left of where the notifications show)
        .map(|row| {
            row.chars()
                .skip(inner.x as usize - 1)
                .take(40)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn app() -> anyhow::Result<(Application, tempfile::NamedTempFile)> {
    let file = helpers::temp_file_with_contents(TEXT)?;
    let app = AppBuilder::new().with_file(file.path(), None).build()?;
    Ok((app, file))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fold_column_shows_blocks_and_closed_folds() -> anyhow::Result<()> {
    let (mut app, _file) = app()?;
    send_keys(&mut app, "").await;
    let rows = text_rows(&app);
    // `▾` where the next line is more indented
    assert_eq!(rows[0], "▾fn outer() {");
    assert_eq!(rows[1], "▾    if a {");
    assert_eq!(rows[2], "         one();");
    assert_eq!(rows[6], " last();");

    // `z i` on `if a`: `▸` on its line, the label after it, the line on the fold's background
    send_keys(&mut app, "jzi").await;
    assert_eq!(folds(&app), 1_usize);
    let rows = text_rows(&app);
    assert_eq!(rows[1], "▸    if a { ▸ 1 line · block");
    assert_eq!(rows[2], "     }");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_click_in_the_fold_column_folds_and_opens() -> anyhow::Result<()> {
    let (mut app, _file) = app()?;
    send_keys(&mut app, "").await;

    // on `▾` of `fn outer`: folded, the whole body
    click_fold_column(&mut app, 0).await;
    assert_eq!(folds(&app), 1_usize);
    let rows = text_rows(&app);
    assert_eq!(rows[0], "▸fn outer() { ▸ 4 lines · block");
    assert_eq!(rows[1], " }");
    assert_eq!(rows[2], " last();");

    // on `▸`: open again
    click_fold_column(&mut app, 0).await;
    assert_eq!(folds(&app), 0_usize);
    assert_eq!(text_rows(&app)[1], "▾    if a {");

    // on a line where nothing starts: nothing happens
    click_fold_column(&mut app, 6).await;
    assert_eq!(folds(&app), 0_usize);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn z_o_z_a_and_z_capital_o_work_anywhere_on_the_line() -> anyhow::Result<()> {
    let (mut app, _file) = app()?;
    send_keys(&mut app, "").await;

    // `z a` folds the block of the line (no text objects for a plain file), then opens it
    send_keys(&mut app, "za").await;
    assert_eq!(folds(&app), 1_usize);
    send_keys(&mut app, "za").await;
    assert_eq!(folds(&app), 0_usize);

    // every block (`z M`): the outer one and the one inside it. The cursor at column 0 of
    // the outer line, before its text, still opens it
    send_keys(&mut app, "zM").await;
    assert_eq!(folds(&app), 2_usize);
    send_keys(&mut app, "gh").await;
    send_keys(&mut app, "zo").await;
    // the outer one only: the inner one stays closed
    assert_eq!(folds(&app), 1_usize);

    send_keys(&mut app, "zM").await;
    assert_eq!(folds(&app), 2_usize);
    // `z O`: the folds inside it too
    send_keys(&mut app, "zO").await;
    assert_eq!(folds(&app), 0_usize);
    Ok(())
}

/// `:fold` and `:unfold` without arguments do what `z f` and `z o` do, rather than saying
/// "The list of text objects is empty."; with `--document`, what `z M` and `z R` do.
#[tokio::test(flavor = "multi_thread")]
async fn fold_and_unfold_without_arguments_work_like_their_keys() -> anyhow::Result<()> {
    let (mut app, _file) = app()?;
    send_keys(&mut app, "").await;

    // on `if a`: its block
    send_keys(&mut app, "j:fold<ret>").await;
    assert_eq!(folds(&app), 1_usize);
    assert!(app
        .editor
        .get_status()
        .is_none_or(|(_, severity)| { *severity != helix_view::editor::Severity::Error }));
    send_keys(&mut app, ":unfold<ret>").await;
    assert_eq!(folds(&app), 0_usize);

    // every block, then everything open again
    send_keys(&mut app, ":fold --document<ret>").await;
    assert_eq!(folds(&app), 2_usize);
    send_keys(&mut app, ":unfold -d<ret>").await;
    assert_eq!(folds(&app), 0_usize);
    Ok(())
}

/// The same in a language with text objects: `:fold` folds the function at the cursor.
#[tokio::test(flavor = "multi_thread")]
async fn fold_without_arguments_folds_the_text_object_at_the_cursor() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
    std::fs::write(
        file.path(),
        "fn one() {\n    let a = 1;\n    let b = 2;\n}\n\nfn two() {}\n",
    )?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_lang_loader(helpers::test_syntax_loader(None))
        .build()?;
    send_keys(&mut app, "j:fold<ret>").await;
    assert_eq!(folds(&app), 1_usize);
    let status = app.editor.get_status().map(|(text, _)| text.to_string());
    assert!(
        status
            .as_deref()
            .is_none_or(|text| !text.contains("is empty")),
        "{status:?}"
    );
    let (view, doc) = current_ref!(app.editor);
    let container = doc.fold_container(view.id).unwrap();
    let fold = container.start_points()[0].fold(container);
    assert_eq!(fold.object().to_string(), "function".to_string());
    send_keys(&mut app, ":unfold<ret>").await;
    assert_eq!(folds(&app), 0_usize);
    Ok(())
}

const GO: &str = r#"package main

import (
	"fmt"
	"os"

	packageprovider "harneet/internal/modules/provider"
	"harneet/styles"
)

func main() {
	fmt.Println(os.Args, packageprovider.X, styles.Y)
}
"#;

/// Go's text objects have no import block: `z f`, `z a` and `:fold` inside one fold its
/// lines (the block of more indented lines), with no language server.
#[tokio::test(flavor = "multi_thread")]
async fn a_go_import_block_folds_though_no_text_object_covers_it() -> anyhow::Result<()> {
    for keys in ["zf", "za", ":fold<ret>"] {
        let file = tempfile::Builder::new().suffix(".go").tempfile()?;
        std::fs::write(file.path(), GO)?;
        let mut app = AppBuilder::new()
            .with_file(file.path(), None)
            .with_lang_loader(helpers::test_syntax_loader(None))
            .build()?;
        // the cursor on the `packageprovider` line, as in an import block being edited
        send_keys(&mut app, &format!("7gg{keys}")).await;
        assert_eq!(
            folds(&app),
            1_usize,
            "{keys}: {:?}",
            app.editor.get_status()
        );
        let (view, doc) = current_ref!(app.editor);
        let container = doc.fold_container(view.id).unwrap();
        let fold = container.start_points()[0].fold(container);
        // the import lines (0-based 3 to 7) hidden, `import (` and `)` shown
        assert_eq!(
            (fold.start.line, fold.end.line),
            (3_usize, 7_usize),
            "{keys}"
        );
        let screen = app.rendered_rows().join("\n");
        assert!(
            screen.contains("import ( ▸ 5 lines · block"),
            "{keys}: {screen}"
        );
        // and `z a` opens it again
        send_keys(&mut app, "za").await;
        assert_eq!(folds(&app), 0_usize, "{keys}");
    }
    Ok(())
}

/// The same in other languages: a construct their text objects do not cover (a `use` list,
/// a parenthesized import, a literal, a table) folds its indented lines. Each case: the file
/// extension, its text with the construct on lines 1 to 3 (0-based, its header on line 0),
/// and the line the cursor is on.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_no_text_object_covers_folds_in_any_language() -> anyhow::Result<()> {
    let cases: [(&str, &str); 10] = [
        (
            "rs",
            "use std::{\n    fmt,\n    io,\n    mem,\n};\n\nfn main() {}\n",
        ),
        (
            "py",
            "from os import (\n    path,\n    sep,\n    getcwd,\n)\n\nprint(path)\n",
        ),
        (
            "js",
            "import {\n  alpha,\n  beta,\n  gamma,\n} from \"x\";\n\nalpha();\n",
        ),
        (
            "ts",
            "import {\n  alpha,\n  beta,\n  gamma,\n} from \"x\";\n\nconst n: number = 1;\n",
        ),
        (
            "c",
            "int values[] = {\n    1,\n    2,\n    3,\n};\n\nint main() { return 0; }\n",
        ),
        (
            "lua",
            "local t = {\n  a = 1,\n  b = 2,\n  c = 3,\n}\n\nprint(t)\n",
        ),
        ("rb", "VALUES = [\n  1,\n  2,\n  3,\n]\n\nputs VALUES\n"),
        ("json", "[\n  1,\n  2,\n  3\n]\n"),
        (
            "toml",
            "deps = [\n  \"a\",\n  \"b\",\n  \"c\",\n]\n\nname = \"x\"\n",
        ),
        ("yaml", "list:\n  - a\n  - b\n  - c\nname: x\n"),
    ];
    for (extension, text) in cases {
        for keys in ["zf", "za", ":fold<ret>"] {
            let file = tempfile::Builder::new()
                .suffix(&format!(".{extension}"))
                .tempfile()?;
            std::fs::write(file.path(), text)?;
            let mut app = AppBuilder::new()
                .with_file(file.path(), None)
                .with_lang_loader(helpers::test_syntax_loader(None))
                .build()?;
            {
                let (_, doc) = current_ref!(app.editor);
                assert!(
                    doc.syntax().is_some(),
                    "{extension}: a syntax tree, so its text objects are looked at"
                );
            }
            // the cursor on the middle line of the construct
            send_keys(&mut app, &format!("3gg{keys}")).await;
            assert_eq!(
                folds(&app),
                1_usize,
                "{extension} {keys}: {:?}",
                app.editor.get_status()
            );
            let (view, doc) = current_ref!(app.editor);
            let container = doc.fold_container(view.id).unwrap();
            let fold = container.start_points()[0].fold(container);
            // its inner lines hidden, the header shown
            assert_eq!(
                (fold.start.line, fold.end.line),
                (1_usize, 3_usize),
                "{extension} {keys}"
            );
            send_keys(&mut app, "za").await;
            assert_eq!(folds(&app), 0_usize, "{extension} {keys}");
        }
    }
    Ok(())
}
