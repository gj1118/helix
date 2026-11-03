use super::*;

/// The tests never load plugins: the default plugin directories are the user's own, and
/// every test would otherwise run with (and load again) whatever plugins are installed.
#[tokio::test(flavor = "multi_thread")]
async fn tests_do_not_load_the_users_plugins() -> anyhow::Result<()> {
    let app = helpers::AppBuilder::new().build()?;
    assert!(!app.plugins_enabled());
    Ok(())
}
