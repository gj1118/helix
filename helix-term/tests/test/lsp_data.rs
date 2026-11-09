//! What the language servers computed for a document is dropped when they no longer serve it
//! (helix-editor/helix#15777): `:lsp-stop`, `:lang` and the like. A fake language server
//! (`fixtures/fake_lsp_data_ls.py`) serves document links, color swatches and code lenses.

use super::*;

use helix_term::application::Application;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// `rust` files served by the fake servers named in `servers`: `fake-full` (links, colors and
/// lenses) and `fake-links` (links only).
fn lang_conf(log: &Path, servers: &[&str]) -> String {
    let server = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/test/fixtures/fake_lsp_data_ls.py"
    );
    let servers = servers
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
        [language-server.fake-full]
        command = "python3"
        args = ["{server}", "{log}", "full"]

        [language-server.fake-links]
        command = "python3"
        args = ["{server}", "{log}", "links"]

        [[language]]
        name = "rust"
        language-servers = [{servers}]
        "#,
        log = log.display()
    )
}

fn config() -> Config {
    let raw: helix_term::config::ConfigRaw = toml::from_str(
        r#"
        [editor.lsp]
        display-code-lenses = true
        display-color-swatches = true
        "#,
    )
    .unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

struct Session {
    app: Application,
    keys: tokio::sync::mpsc::UnboundedSender<std::io::Result<Event>>,
    stream: UnboundedReceiverStream<std::io::Result<Event>>,
    _file: tempfile::NamedTempFile,
    _log: tempfile::NamedTempFile,
}

impl Session {
    fn new(servers: &[&str]) -> anyhow::Result<Self> {
        let file = tempfile::Builder::new().suffix(".rs").tempfile()?;
        std::fs::write(file.path(), "fn main() {}\n")?;
        let log = tempfile::NamedTempFile::new()?;
        let app = helpers::AppBuilder::new()
            .with_file(file.path(), None)
            .with_config(config())
            .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf(
                log.path(),
                servers,
            ))))
            .build()?;
        let (keys, rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(Self {
            app,
            keys,
            stream: UnboundedReceiverStream::new(rx),
            _file: file,
            _log: log,
        })
    }

    async fn send(&mut self, keys: &str) -> anyhow::Result<()> {
        for key in helix_view::input::parse_macro(keys)? {
            self.keys.send(Ok(Event::Key(KeyEvent::from(key))))?;
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.app.event_loop_until_idle(&mut self.stream),
        )
        .await;
        Ok(())
    }

    /// Runs the editor until `done` holds (true) or a few seconds pass (false).
    async fn until(&mut self, mut done: impl FnMut(&Application) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if done(&self.app) {
                return true;
            }
            let _ = tokio::time::timeout(
                Duration::from_millis(100),
                self.app.event_loop_until_idle(&mut self.stream),
            )
            .await;
        }
        done(&self.app)
    }

    async fn quit(mut self) -> anyhow::Result<()> {
        helpers::test_key_sequence(&mut self.app, Some(":qa!<ret>"), None, true).await
    }
}

fn link_targets(app: &Application) -> Vec<String> {
    let doc = helix_view::doc!(app.editor);
    let mut targets: Vec<_> = doc
        .document_links
        .iter()
        .filter_map(|link| link.link.target.as_ref().map(|url| url.to_string()))
        .collect();
    targets.sort();
    targets
}

fn has_swatches(app: &Application) -> bool {
    let doc = helix_view::doc!(app.editor);
    doc.color_swatches
        .as_ref()
        .is_some_and(|swatches| !swatches.color_swatches.is_empty())
}

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

fn full_data_shown(app: &Application) -> bool {
    link_targets(app).contains(&"file:///full".to_string())
        && has_swatches(app)
        && !helix_view::doc!(app.editor).code_lenses.is_empty()
        && screen(app).contains("Run full")
}

/// `:lsp-stop` drops the stopped server's links, color swatches and code lenses (the lens
/// label disappears from the screen).
#[tokio::test(flavor = "multi_thread")]
async fn lsp_stop_drops_what_the_server_computed() -> anyhow::Result<()> {
    let mut session = Session::new(&["fake-full"])?;
    assert!(
        session.until(full_data_shown).await,
        "the server's data never arrived:\n{}",
        screen(&session.app)
    );

    session.send(":lsp-stop<ret>").await?;
    let app = &session.app;
    assert_eq!(link_targets(app), Vec::<String>::new());
    assert!(!has_swatches(app));
    assert!(helix_view::doc!(app.editor).code_lenses.is_empty());
    assert!(!screen(app).contains("Run full"), "{}", screen(app));
    session.quit().await
}

/// Stopping one of two servers keeps the other's data: it is asked again for its links, which
/// were cleared with the stopped server's.
#[tokio::test(flavor = "multi_thread")]
async fn lsp_stop_asks_the_remaining_servers_again() -> anyhow::Result<()> {
    let mut session = Session::new(&["fake-full", "fake-links"])?;
    assert!(
        session
            .until(|app| full_data_shown(app)
                && link_targets(app) == ["file:///full", "file:///links"])
            .await,
        "the servers' data never arrived: {:?}",
        link_targets(&session.app)
    );

    session.send(":lsp-stop fake-full<ret>").await?;
    assert!(
        session
            .until(|app| link_targets(app) == ["file:///links"])
            .await,
        "the remaining server's links: {:?}",
        link_targets(&session.app)
    );
    let app = &session.app;
    assert!(!has_swatches(app));
    assert!(!screen(app).contains("Run full"), "{}", screen(app));
    session.quit().await
}

/// Switching the language to one without the server (`:lang`) drops the server's data too.
#[tokio::test(flavor = "multi_thread")]
async fn changing_the_language_drops_the_old_servers_data() -> anyhow::Result<()> {
    let mut session = Session::new(&["fake-full"])?;
    assert!(
        session.until(full_data_shown).await,
        "the server's data never arrived:\n{}",
        screen(&session.app)
    );

    session.send(":lang text<ret>").await?;
    let app = &session.app;
    assert_eq!(link_targets(app), Vec::<String>::new());
    assert!(!has_swatches(app));
    assert!(helix_view::doc!(app.editor).code_lenses.is_empty());
    assert!(!screen(app).contains("Run full"), "{}", screen(app));
    session.quit().await
}
