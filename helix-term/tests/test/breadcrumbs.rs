use std::fs;

use helix_term::application::Application;
use helix_view::{doc, editor::BreadcrumbPathOptions};

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn breadcrumb_tree_sitter_fallback_trail() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::with_suffix(".rs")?;
    fs::write(
        file.path(),
        "\
fn outer() {
    fn inner() {
        let x = 1;
    }
}
",
    )?;

    let mut config = helpers::test_config();
    config.editor.breadcrumb.enable = true;
    config.editor.breadcrumb.path = BreadcrumbPathOptions::None;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let assertion = |app: &Application| {
        // Move the cursor into `inner` happened via the key sequence; the
        // trail must now contain it.
        let view_id = app.editor.tree.focus;
        let doc = doc!(app.editor);

        let breadcrumb = doc
            .breadcrumbs
            .get(&view_id)
            .expect("breadcrumb trail should exist for the focused view");

        assert!(!breadcrumb.is_empty());
        let names: Vec<_> = breadcrumb
            .iter()
            .map(|crumb| crumb.name.to_string())
            .collect();
        assert!(
            names.iter().any(|name| name == "inner"),
            "expected the trail to contain `inner`, got {names:?}"
        );
    };

    test_key_sequence(&mut app, Some("jj"), Some(&assertion), false).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn breadcrumb_disabled_by_default() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::with_suffix(".rs")?;
    fs::write(file.path(), "fn main() {}\n")?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    let assertion = |app: &Application| {
        let doc = doc!(app.editor);
        assert!(
            doc.breadcrumbs.is_empty(),
            "no trails should be tracked while breadcrumbs are disabled"
        );
    };

    test_key_sequence(&mut app, Some("j"), Some(&assertion), false).await?;

    Ok(())
}

/// One step of [`test_key_sequences`]: keys, then an assertion.
type Step<'a> = (Option<&'a str>, Option<&'a dyn Fn(&Application)>);

/// The tree-sitter symbols are computed in the background after an edit: a function added
/// while editing shows up in the trail once the computation is done.
#[tokio::test(flavor = "multi_thread")]
async fn breadcrumb_tree_sitter_symbols_follow_edits() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::with_suffix(".rs")?;
    fs::write(file.path(), "fn outer() {\n    let x = 1;\n}\n")?;

    let mut config = helpers::test_config();
    config.editor.breadcrumb.enable = true;
    config.editor.breadcrumb.path = BreadcrumbPathOptions::None;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let names = |app: &Application| -> Vec<String> {
        let view_id = app.editor.tree.focus;
        doc!(app.editor)
            .breadcrumbs
            .get(&view_id)
            .map(|trail| trail.iter().map(|crumb| crumb.name.to_string()).collect())
            .unwrap_or_default()
    };
    let has_added = move |app: &Application| {
        let names = names(app);
        assert!(
            names.iter().any(|name| name == "added"),
            "expected the trail to contain `added`, got {names:?}"
        );
    };

    let mut steps: Vec<Step> = vec![(Some("jofn added() { let y = 2;<esc>"), None)];
    // idle rounds while the symbols are computed after the debounce
    steps.extend(std::iter::repeat_n((Some("<esc>"), None), 20));
    steps.push((Some("<esc>"), Some(&has_added)));
    test_key_sequences(&mut app, steps, false).await?;

    Ok(())
}
