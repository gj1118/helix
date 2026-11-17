use super::*;

use helix_term::application::Application;
use helix_term::remote::{send, OpenHow, Request, Response};
use std::path::PathBuf;

/// Runs the editor's loop until `task` (a client talking to it) is done, and returns what
/// the task returned.
async fn serve_until<T: Send + 'static>(
    app: &mut Application,
    task: tokio::task::JoinHandle<T>,
) -> anyhow::Result<T> {
    for _ in 0..200 {
        if task.is_finished() {
            return Ok(task.await?);
        }
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(
            tokio::sync::mpsc::unbounded_channel().1,
        );
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            app.event_loop_until_idle(&mut stream),
        )
        .await
        .ok();
    }
    anyhow::bail!("the client got no answer")
}

fn current_path(app: &Application) -> Option<PathBuf> {
    helix_view::current_ref!(app.editor)
        .1
        .path()
        .map(|path| path.to_path_buf())
}

/// `hx --remote file:line` opens the file in the running Helix at that line, and
/// `--remote-command` runs a `:` command there; both are answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_helix_opens_files_and_runs_commands_for_remote() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("hx-test.sock");
    let file = helpers::temp_file_with_contents("one\ntwo\nthree\n")?;
    let path = helix_stdx::path::canonicalize(file.path());

    let mut app = helpers::AppBuilder::new().build()?;
    app.listen(&socket)?;
    assert_eq!(app.remote_socket(), Some(socket.as_path()));
    assert_eq!(
        std::env::var_os(helix_term::remote::ENV_VAR),
        Some(socket.clone().into_os_string()),
        "programs Helix starts are told where it listens"
    );

    // open at line 3 (row 2)
    let request = Request::Open {
        files: vec![(path.clone(), vec![(2, 0)])],
        how: OpenHow::Replace,
    };
    let client_socket = socket.clone();
    let response: Response = serve_until(
        &mut app,
        tokio::spawn(async move { send(&client_socket, &request).await }),
    )
    .await??;
    assert!(response.ok, "{}", response.message);
    assert_eq!(response.message, format!("Opened {}", path.display()));
    assert_eq!(current_path(&app), Some(path.clone()));
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(doc.text().char_to_line(cursor), 2_usize);

    // a command: split the window
    let views_before = app.editor.tree.views().count();
    let client_socket = socket.clone();
    let response: Response = serve_until(
        &mut app,
        tokio::spawn(async move {
            send(
                &client_socket,
                &Request::Command {
                    line: "vsplit".into(),
                },
            )
            .await
        }),
    )
    .await??;
    assert!(response.ok, "{}", response.message);
    assert_eq!(app.editor.tree.views().count(), views_before + 1);

    // an unknown command is an error, answered as one
    let client_socket = socket.clone();
    let response: Response = serve_until(
        &mut app,
        tokio::spawn(async move {
            send(
                &client_socket,
                &Request::Command {
                    line: "no-such-command".into(),
                },
            )
            .await
        }),
    )
    .await??;
    assert!(!response.ok);
    assert!(
        response.message.contains("no-such-command"),
        "{}",
        response.message
    );

    // a folder is not opened
    let client_socket = socket.clone();
    let folder = dir.path().to_path_buf();
    let response: Response = serve_until(
        &mut app,
        tokio::spawn(async move {
            send(
                &client_socket,
                &Request::Open {
                    files: vec![(folder, vec![])],
                    how: OpenHow::Replace,
                },
            )
            .await
        }),
    )
    .await??;
    assert!(!response.ok);
    assert!(
        response.message.ends_with("is a folder"),
        "{}",
        response.message
    );

    drop(app);
    assert!(!socket.exists(), "the socket goes with the editor");
    Ok(())
}
