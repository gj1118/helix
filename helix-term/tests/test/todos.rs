use super::*;

use helix_term::application::Application;

const SOURCE: &str = "fn main() {}\n// TODO: later\n// FIXME: now\n";

/// The focused view's primary selection, as the text it selects.
fn selected(app: &Application) -> String {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.selection(view.id)
        .primary()
        .fragment(doc.text().slice(..))
        .into_owned()
}

/// The focused view's primary selection, as a character range.
fn selected_range(app: &Application) -> (usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let range = doc.selection(view.id).primary();
    (range.from(), range.to())
}

/// The character range of `needle` in [`SOURCE`] (all ASCII).
fn range_of(needle: &str) -> (usize, usize) {
    let start = SOURCE.find(needle).unwrap();
    (start, start + needle.len())
}

fn app_with_source() -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("tags.rs");
    std::fs::write(&file, SOURCE)?;
    let app = AppBuilder::new().with_file(&file, None).build()?;
    Ok((dir, app))
}

/// Picking a tag of the current file selects its keyword. The current file's
/// tags are listed in document order: TODO first, then FIXME.
#[tokio::test(flavor = "multi_thread")]
async fn picking_a_tag_selects_its_keyword() -> anyhow::Result<()> {
    let (_dir, mut app) = app_with_source()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>t<ret>"),
                Some(&|app| {
                    assert_eq!(selected(app), "TODO");
                    assert_eq!(selected_range(app), range_of("TODO"));
                    assert_eq!(app.editor.tree.views().count(), 1);
                }),
            ),
            (
                Some("<space>tnow<ret>"),
                Some(&|app| {
                    assert_eq!(selected(app), "FIXME");
                    assert_eq!(selected_range(app), range_of("FIXME"));
                }),
            ),
        ],
        false,
    )
    .await
}

/// The split keys of the picker open the tag's document in a split, like for
/// the other pickers, instead of selecting in the current view.
#[tokio::test(flavor = "multi_thread")]
async fn picking_a_tag_in_a_split_opens_the_split() -> anyhow::Result<()> {
    let (_dir, mut app) = app_with_source()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>tnow<C-v>"),
                Some(&|app| {
                    assert_eq!(app.editor.tree.views().count(), 2);
                    assert_eq!(selected(app), "FIXME");
                    assert_eq!(selected_range(app), range_of("FIXME"));
                }),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}
