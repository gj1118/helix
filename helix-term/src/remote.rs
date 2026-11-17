//! Remote control of a running Helix: each `hx` listens on a Unix socket of its own (in a
//! folder only its user can read), whose path it puts in `HELIX_LISTEN` for the programs it
//! starts. `hx --remote file:line` then opens files in it, and `hx --remote-command ':cmd'`
//! runs a `:` command, from a terminal, a file manager or a git tool.
//!
//! The protocol is one JSON request per connection, on one line, answered by one JSON line.

use std::path::{Path, PathBuf};

use helix_core::Position;
use serde::{Deserialize, Serialize};

/// The environment variable a running Helix sets to its socket's path.
pub const ENV_VAR: &str = "HELIX_LISTEN";

/// What a client asks of a running Helix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// Open files (absolute paths), at the positions given (0-based rows and columns).
    Open {
        files: Vec<(PathBuf, Vec<(usize, usize)>)>,
        how: OpenHow,
    },
    /// Run a `:` command line (without the `:`).
    Command { line: String },
    /// Whether it is there.
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenHow {
    Replace,
    VerticalSplit,
    HorizontalSplit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
        }
    }
}

/// The positions of a file argument as the protocol sends them.
pub fn positions(positions: &Option<Vec<Position>>) -> Vec<(usize, usize)> {
    positions
        .iter()
        .flatten()
        .map(|position| (position.row, position.col))
        .collect()
}

/// The folder the sockets are made in: `helix` in `$XDG_RUNTIME_DIR`, else `helix-<user id>`
/// in the temporary folder.
pub fn socket_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join("helix");
    }
    #[cfg(unix)]
    let user = rustix_uid();
    #[cfg(not(unix))]
    let user = "user".to_string();
    std::env::temp_dir().join(format!("helix-{user}"))
}

#[cfg(unix)]
fn rustix_uid() -> String {
    // SAFETY: getuid cannot fail and has no preconditions.
    unsafe { libc::getuid() }.to_string()
}

/// The default socket of this process.
pub fn default_socket() -> PathBuf {
    socket_dir().join(format!("hx-{}.sock", std::process::id()))
}

/// A request that came in, and where its answer goes.
pub struct Incoming {
    pub request: Request,
    pub reply: tokio::sync::oneshot::Sender<Response>,
}

#[cfg(unix)]
pub use unix::*;

/// Without Unix sockets there is no server; this one never gets a request.
#[cfg(not(unix))]
pub struct Server {
    pub requests: tokio::sync::mpsc::UnboundedReceiver<Incoming>,
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::{mpsc, oneshot};

    /// The longest request read: a request is a line of JSON.
    const MAX_REQUEST: usize = 1 << 20;

    /// A listening socket; its file is removed when this is dropped.
    pub struct Server {
        path: PathBuf,
        pub requests: mpsc::UnboundedReceiver<Incoming>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Server {
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Makes `dir` (and its parents) with permissions for its user only.
    fn private_dir(dir: &Path) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let metadata = std::fs::metadata(dir)?;
        // A folder made by someone else (in a shared temporary folder) is not used.
        // SAFETY: getuid cannot fail and has no preconditions.
        let uid = unsafe { libc::getuid() };
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} belongs to another user", dir.display()),
            ));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Listens on `path`. A stale socket there (no one answering) is replaced; a live one is
    /// an error.
    pub fn listen(path: &Path) -> io::Result<Server> {
        if let Some(dir) = path.parent() {
            private_dir(dir)?;
        }
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another Helix listens on {}", path.display()),
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        let (sender, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let sender = sender.clone();
                tokio::spawn(async move {
                    if let Err(err) = serve(stream, sender).await {
                        log::debug!("remote request failed: {err}");
                    }
                });
            }
        });
        Ok(Server {
            path: path.to_path_buf(),
            requests,
            task,
        })
    }

    async fn serve(stream: UnixStream, sender: mpsc::UnboundedSender<Incoming>) -> io::Result<()> {
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        let mut reader = BufReader::new(read).take(MAX_REQUEST as u64);
        reader.read_line(&mut line).await?;
        let response = match serde_json::from_str::<Request>(&line) {
            Err(err) => Response::error(format!("bad request: {err}")),
            Ok(Request::Ping) => Response::ok("pong"),
            Ok(request) => {
                let (reply, answer) = oneshot::channel();
                if sender.send(Incoming { request, reply }).is_err() {
                    Response::error("Helix is closing")
                } else {
                    answer
                        .await
                        .unwrap_or_else(|_| Response::error("Helix is closing"))
                }
            }
        };
        let mut bytes = serde_json::to_vec(&response).map_err(io::Error::other)?;
        bytes.push(b'\n');
        write.write_all(&bytes).await?;
        write.shutdown().await
    }

    /// Sends `request` to the Helix listening on `path` and waits for its answer.
    pub async fn send(path: &Path, request: &Request) -> io::Result<Response> {
        let stream = UnixStream::connect(path).await?;
        let (read, mut write) = stream.into_split();
        let mut bytes = serde_json::to_vec(request).map_err(io::Error::other)?;
        bytes.push(b'\n');
        write.write_all(&bytes).await?;
        let mut line = String::new();
        BufReader::new(read)
            .take(MAX_REQUEST as u64)
            .read_line(&mut line)
            .await?;
        serde_json::from_str(&line).map_err(io::Error::other)
    }

    /// The Helix to talk to: the socket given (`--server`), else `$HELIX_LISTEN` (set for the
    /// programs a Helix runs), else the most recently started one that answers.
    pub async fn find_server(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(path) = explicit {
            return Some(path.to_path_buf());
        }
        if let Some(path) = std::env::var_os(ENV_VAR).filter(|path| !path.is_empty()) {
            return Some(PathBuf::from(path));
        }
        let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(socket_dir())
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let is_socket = path
                    .file_name()?
                    .to_str()
                    .is_some_and(|name| name.starts_with("hx-") && name.ends_with(".sock"));
                let modified = entry.metadata().ok()?.modified().ok()?;
                is_socket.then_some((modified, path))
            })
            .collect();
        sockets.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in sockets {
            if matches!(send(&path, &Request::Ping).await, Ok(response) if response.ok) {
                return Some(path);
            }
        }
        None
    }
}

/// `hx --remote` and `hx --remote-command`: talks to a running Helix. `None` when there is
/// none to open the files in, and this Helix should open them itself; else the exit code.
pub async fn run_client(args: &crate::args::Args) -> Option<i32> {
    #[cfg(not(unix))]
    {
        let _ = args;
        eprintln!("--remote and --remote-command are only supported on Unix");
        return Some(1);
    }
    #[cfg(unix)]
    {
        let server = find_server(args.server.as_deref()).await;
        if let Some(line) = &args.remote_command {
            let Some(server) = server else {
                eprintln!("No running Helix to send the command to (start one, or pass --server)");
                return Some(1);
            };
            let line = line.trim().trim_start_matches(':').to_string();
            return Some(report(
                send(&server, &Request::Command { line }).await,
                &server,
            ));
        }
        if args.files.is_empty() {
            return None;
        }
        let server = match server {
            Some(server) => server,
            // nothing running: open them here
            None => return None,
        };
        let how = match args.split {
            Some(helix_view::tree::Layout::Vertical) => OpenHow::VerticalSplit,
            Some(helix_view::tree::Layout::Horizontal) => OpenHow::HorizontalSplit,
            None => OpenHow::Replace,
        };
        let files = args
            .files
            .iter()
            .map(|(path, at)| (path.clone(), positions(at)))
            .collect();
        match send(&server, &Request::Open { files, how }).await {
            // the Helix that set HELIX_LISTEN is gone: open them here
            Err(_) if args.server.is_none() => None,
            result => Some(report(result, &server)),
        }
    }
}

#[cfg(unix)]
fn report(result: std::io::Result<Response>, server: &Path) -> i32 {
    match result {
        Ok(response) if response.ok => {
            if !response.message.is_empty() {
                println!("{}", response.message);
            }
            0
        }
        Ok(response) => {
            eprintln!("{}", response.message);
            1
        }
        Err(err) => {
            eprintln!("Could not reach the Helix at {}: {err}", server.display());
            1
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_is_answered_and_the_socket_goes_with_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("hx-test.sock");
        let mut server = listen(&path).unwrap();
        assert!(path.exists());

        // a second Helix cannot take a live socket
        let err = listen(&path).err().map(|err| err.kind());
        assert_eq!(err, Some(std::io::ErrorKind::AddrInUse));

        let answering = tokio::spawn(async move {
            let incoming = server.requests.recv().await.unwrap();
            let line = match &incoming.request {
                Request::Command { line } => line.clone(),
                other => panic!("unexpected {other:?}"),
            };
            incoming
                .reply
                .send(Response::ok(format!("ran {line}")))
                .unwrap();
            server
        });
        let response = send(
            &path,
            &Request::Command {
                line: "write".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(response, Response::ok("ran write"));
        assert_eq!(
            send(&path, &Request::Ping).await.unwrap(),
            Response::ok("pong")
        );

        let server = answering.await.unwrap();
        // only its user may enter the folder
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700_u32);
        drop(server);
        assert!(!path.exists(), "the socket file is removed");
        assert!(send(&path, &Request::Ping).await.is_err());
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hx-stale.sock");
        // a socket file nobody listens on, as a crashed Helix leaves it
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let server = listen(&path).unwrap();
        let pong: Response = send(&path, &Request::Ping).await.unwrap();
        assert!(pong.ok, "{}", pong.message);
        drop(server);
    }

    #[tokio::test]
    async fn a_bad_request_gets_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hx-bad.sock");
        let _server = listen(&path).unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        stream.write_all(b"{\"nope\":1}\n").await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        let response: Response = serde_json::from_str(&answer).unwrap();
        assert!(!response.ok);
        assert!(
            response.message.starts_with("bad request"),
            "{}",
            response.message
        );
    }

    #[test]
    fn requests_are_one_json_line() {
        let request = Request::Open {
            files: vec![(PathBuf::from("/tmp/a.rs"), vec![(4, 2)])],
            how: OpenHow::VerticalSplit,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"open":{"files":[["/tmp/a.rs",[[4,2]]]],"how":"vertical_split"}}"#
        );
        assert!(!json.contains('\n'));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, request);
    }
}
