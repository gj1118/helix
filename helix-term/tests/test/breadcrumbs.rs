use std::fs;

use helix_term::{application::Application, config::Config};
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

    let mut config = Config::default();
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
