use super::*;

use std::path::Path;
use std::sync::Arc;

use helix_view::snippets::{parse_file, Snippets};

fn snippets(toml: &str) -> Arc<Snippets> {
    Arc::new(Snippets::from_defs(
        parse_file(Path::new("test.toml"), toml, false).unwrap(),
    ))
}

fn app_with(toml: &str, input: &str) -> anyhow::Result<helix_term::application::Application> {
    let mut app = helpers::AppBuilder::new()
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;
    app.editor.snippets = snippets(toml);
    let (view, doc) = helix_view::current!(app.editor);
    let (text, selection) = helix_core::test::print(input);
    let transaction =
        helix_core::Transaction::change_by_selection(doc.text(), doc.selection(view.id), |_| {
            (0, doc.text().len_chars(), Some((&text).into()))
        })
        .with_selection(selection);
    doc.apply(&transaction, view.id);
    Ok(app)
}

/// The text with a `|` where each cursor (the place typed text would go) is. Helix selections
/// are always at least one character wide, so the selection markup isn't a good way to compare.
fn state(app: &helix_term::application::Application) -> String {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let mut cursors: Vec<usize> = doc
        .selection(view.id)
        .iter()
        .map(|range| range.cursor(text))
        .collect();
    cursors.sort_unstable();
    let mut out = text.to_string();
    for cursor in cursors.into_iter().rev() {
        let byte = text.char_to_byte(cursor);
        out.insert(byte, '|');
    }
    out
}

/// Run `keys` in a document with `input` (selection markup) and check the result.
async fn check(toml: &str, input: &str, keys: &str, expected: &str) -> anyhow::Result<()> {
    let mut app = app_with(toml, input)?;
    let expected = expected.to_owned();
    test_key_sequence(
        &mut app,
        Some(keys),
        Some(&move |app| assert_eq!(state(app), expected)),
        false,
    )
    .await
}

const SNIPPETS: &str = r#"
[[snippet]]
name = "call"
body = "call($1, $2)$0"
prefix = "call"

[[snippet]]
name = "wrap"
body = "(${TM_SELECTED_TEXT:nothing})$0"

[[snippet]]
name = "number"
body = "$CURSOR_NUMBER:$TM_LINE_NUMBER:$0"
"#;

#[tokio::test(flavor = "multi_thread")]
async fn insert_at_cursor_and_walk_the_tabstops() -> anyhow::Result<()> {
    check(
        SNIPPETS,
        "#[|]#",
        ":snippet-insert call<ret>a<tab>b<tab>",
        "call(a, b)|",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_is_replaced_and_can_be_wrapped() -> anyhow::Result<()> {
    check(
        SNIPPETS,
        "#[foo|]# bar",
        ":snippet-insert wrap<ret>",
        "(foo)| bar",
    )
    .await?;
    // nothing selected: a single character in normal mode is just the cursor
    check(
        SNIPPETS,
        "#[f|]#oo",
        ":snippet-insert wrap<ret>",
        "(nothing)|foo",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_cursor_gets_its_own_variables() -> anyhow::Result<()> {
    check(
        SNIPPETS,
        "#[a|]#\nb\n#(c|)#",
        ":snippet-insert number<ret>",
        "1:1:|a\nb\n2:3:|c",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insertion_is_one_undo_step() -> anyhow::Result<()> {
    check(SNIPPETS, "#[x|]#", ":snippet-insert call<ret><esc>u", "|x").await
}

#[tokio::test(flavor = "multi_thread")]
async fn file_variables() -> anyhow::Result<()> {
    let file = tempfile::Builder::new()
        .prefix("snip")
        .suffix(".txt")
        .tempfile()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;
    app.editor.snippets = snippets(
        r#"
        [[snippet]]
        name = "file"
        body = "$TM_FILENAME|$TM_FILENAME_BASE|$TM_DIRECTORY|$UUID|$CURRENT_YEAR"
        "#,
    );
    let name = file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let base = file
        .path()
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let dir = file.path().parent().unwrap().to_string_lossy().into_owned();
    test_key_sequence(
        &mut app,
        Some(":snippet-insert file<ret>"),
        Some(&move |app| {
            let (_, doc) = helix_view::current_ref!(app.editor);
            let text = doc.text().to_string();
            let parts: Vec<&str> = text.trim_end().split('|').collect();
            assert_eq!(
                &parts[..3],
                [name.as_str(), base.as_str(), dir.as_str()],
                "{text}"
            );
            assert_eq!(parts[3].len(), 36, "{text}");
            assert!(parts[4].parse::<u32>().unwrap() >= 2024, "{text}");
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_snippet_reports_an_error() -> anyhow::Result<()> {
    let mut app = app_with(SNIPPETS, "#[|]#")?;
    test_key_sequence(
        &mut app,
        Some(":snippet-insert nope<ret>"),
        Some(&|app| {
            assert_eq!(state(app), "|");
            let (message, _) = app.editor.get_status().unwrap();
            assert!(message.contains("No snippet named \"nope\""), "{message}");
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_inserts_the_selected_snippet() -> anyhow::Result<()> {
    let mut app = app_with(SNIPPETS, "#[|]#")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>i"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("call") && rows.contains("wrap"), "{rows}");
                }),
            ),
            (Some("call<ret>"), None),
            (
                Some("x<tab>y"),
                Some(&|app| assert_eq!(state(app), "call(x, y|)")),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_completion_replaces_the_typed_prefix() -> anyhow::Result<()> {
    let mut app = app_with(SNIPPETS, "#[|]#")?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("ica"), None),
            (
                Some("<C-x>"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("call"), "completion menu missing:\n{rows}");
                }),
            ),
            (Some("<C-n><ret>"), None),
            (
                Some("x<tab>y"),
                Some(&|app| assert_eq!(state(app), "call(x, y|)")),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_can_be_disabled() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_config({
            let mut config = helpers::test_config();
            config.editor.snippet_completion = false;
            config
        })
        .with_file(tempfile::NamedTempFile::new()?.path(), None)
        .build()?;
    app.editor.snippets = snippets(SNIPPETS);
    test_key_sequences(
        &mut app,
        vec![
            (Some("ica"), None),
            (
                Some("<C-x>"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(!rows.contains("call"), "{rows}");
                }),
            ),
        ],
        false,
    )
    .await
}
