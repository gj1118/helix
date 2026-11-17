use super::*;

use helix_term::application::Application;
use std::io::{Read, Write};

/// Serves `body` (with `content_type`) to every connection on a local port, on a thread;
/// returns the port. A path ending in `/missing` gets a 404.
fn serve(body: &'static str, content_type: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let response = if request.starts_with("GET /missing") {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// Lets the fetch (curl, in the background) finish until `done` holds.
async fn wait_until(
    app: &mut Application,
    done: impl Fn(&Application) -> bool,
) -> anyhow::Result<()> {
    for _ in 0..200 {
        if done(app) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(
            tokio::sync::mpsc::unbounded_channel().1,
        );
        app.event_loop_until_idle(&mut stream).await;
    }
    anyhow::bail!("timed out")
}

async fn send(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    drop(tx);
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    Ok(())
}

fn shown(app: &Application) -> (String, String, Option<String>) {
    let (_, doc) = helix_view::current_ref!(app.editor);
    (
        doc.display_name().to_string(),
        doc.text().to_string(),
        doc.language_name().map(String::from),
    )
}

/// `:open http://...` fetches the page into a buffer titled with the URL, in the language
/// its content type says; a failed fetch is an error and opens nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_url_opens_in_a_buffer_of_its_own() -> anyhow::Result<()> {
    // the local server is not behind the proxy
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    std::env::set_var("no_proxy", "127.0.0.1,localhost");
    let port = serve("{\"name\": \"helix\"}\n", "application/json; charset=utf-8");
    let url = format!("http://127.0.0.1:{port}/api/thing");

    let mut app = helpers::AppBuilder::new()
        .with_file(helpers::temp_file_with_contents("local\n")?.path(), None)
        .build()?;
    send(&mut app, &format!(":open {url}<ret>")).await?;
    let wanted = url.clone();
    wait_until(&mut app, move |app| shown(app).0 == wanted).await?;
    let (name, text, language) = shown(&app);
    assert_eq!(name, url);
    assert_eq!(text, "{\"name\": \"helix\"}\n".to_string());
    assert_eq!(language.as_deref(), Some("json"));
    assert_eq!(
        app.editor.get_status().map(|(s, _)| s.to_string()),
        Some(format!("Opened {url}"))
    );
    let (_, doc) = helix_view::current_ref!(app.editor);
    assert!(doc.path().is_none(), "nothing on disk: :w needs a path");

    // a 404 is an error, and the buffer stays
    let missing = format!("http://127.0.0.1:{port}/missing");
    send(&mut app, &format!(":open {missing}<ret>")).await?;
    let prefix = format!("Failed to open {missing}: ");
    wait_until(&mut app, move |app| {
        app.editor
            .get_status()
            .is_some_and(|(status, _)| status.starts_with(&prefix))
    })
    .await?;
    assert_eq!(shown(&app).0, url);
    Ok(())
}
