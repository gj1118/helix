//! `:open https://...`: the response of a URL in a buffer of its own, titled with the URL,
//! like Neovim's `:edit https://...`. The page is fetched with `curl` in the background.

use std::path::Path;
use std::time::Duration;

use helix_core::Rope;
use helix_view::{editor::Action, Document, Editor};

use crate::job::{self, Jobs};

/// The largest response read, in bytes.
const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// How long a fetch may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Whether `arg` is a URL `:open` fetches (rather than a file path).
pub fn is_url(arg: &str) -> bool {
    let lower = arg.get(..8).unwrap_or(arg).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// The language of a response: from the URL's file extension, else from its content type.
pub fn language_id(
    loader: &helix_core::syntax::Loader,
    url: &str,
    content_type: &str,
) -> Option<String> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let name = path.rsplit('/').next().unwrap_or("");
    if name.contains('.') {
        if let Some(language) = loader.language_for_filename(Path::new(name)) {
            return Some(loader.language(language).config().language_id.clone());
        }
    }
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let id = match mime.as_str() {
        "application/json" | "application/ld+json" | "application/geo+json" => "json",
        "text/html" | "application/xhtml+xml" => "html",
        "text/markdown" | "text/x-markdown" => "markdown",
        "text/css" => "css",
        "text/javascript" | "application/javascript" | "application/x-javascript" => "javascript",
        "application/xml" | "text/xml" | "application/rss+xml" | "application/atom+xml" => "xml",
        "application/yaml" | "application/x-yaml" | "text/yaml" | "text/x-yaml" => "yaml",
        "application/toml" | "text/x-toml" => "toml",
        "text/x-python" | "application/x-python" => "python",
        "text/x-rust" => "rust",
        "application/x-sh" | "text/x-shellscript" => "bash",
        "text/csv" => "csv",
        mime if mime.ends_with("+json") => "json",
        mime if mime.ends_with("+xml") => "xml",
        _ => return None,
    };
    loader
        .language_for_name(id.to_string())
        .map(|language| loader.language(language).config().language_id.clone())
}

/// Splits what `curl -w '\n%{content_type}'` printed into the body and the content type.
fn split_body(mut output: Vec<u8>) -> (Vec<u8>, String) {
    match output.iter().rposition(|byte| *byte == b'\n') {
        Some(at) => {
            let content_type = String::from_utf8_lossy(&output[at + 1..]).into_owned();
            output.truncate(at);
            (output, content_type)
        }
        None => (output, String::new()),
    }
}

/// Fetches `url` in the background, then shows it in a new buffer (with `action`).
pub fn open(editor: &mut Editor, jobs: &mut Jobs, url: String, action: Action) {
    editor.set_status(format!("Fetching {url}…"));
    jobs.callback(async move {
        let output = tokio::time::timeout(
            TIMEOUT + Duration::from_secs(2),
            tokio::process::Command::new("curl")
                .args([
                    "--silent",
                    "--show-error",
                    "--location",
                    "--fail",
                    "--max-time",
                    &TIMEOUT.as_secs().to_string(),
                    "--max-filesize",
                    &MAX_BYTES.to_string(),
                    "--user-agent",
                    concat!("helix/", env!("CARGO_PKG_VERSION")),
                    "--write-out",
                    "\n%{content_type}",
                    "--",
                    &url,
                ])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        let result: Result<(Vec<u8>, String), String> = match output {
            Err(_) => Err("the server took too long".into()),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                Err("curl was not found: it is needed to open URLs".into())
            }
            Ok(Err(err)) => Err(err.to_string()),
            Ok(Ok(output)) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim().trim_start_matches("curl: ");
                Err(if message.is_empty() {
                    format!("curl failed ({})", output.status)
                } else {
                    message.to_string()
                })
            }
            Ok(Ok(output)) => Ok(split_body(output.stdout)),
        };
        let call: job::Callback = job::Callback::Editor(Box::new(move |editor: &mut Editor| {
            let (body, content_type) = match result {
                Ok(fetched) => fetched,
                Err(err) => {
                    editor.set_error(format!("Failed to open {url}: {err}"));
                    return;
                }
            };
            if body.contains(&0) {
                editor.set_error(format!("{url} is not text ({content_type})"));
                return;
            }
            let text = String::from_utf8_lossy(&body);
            let mut doc = Document::from(
                Rope::from(text.as_ref()),
                None,
                editor.config.clone(),
                editor.syn_loader.clone(),
            );
            doc.title = Some(url.clone());
            let loader = editor.syn_loader.load();
            if let Some(language) = language_id(&loader, &url, &content_type) {
                let _ = doc.set_language_by_language_id(&language, &loader);
            }
            drop(loader);
            editor.new_file_from_document(action, doc);
            editor.set_status(format!("Opened {url}"));
        }));
        Ok(call)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_told_from_paths() {
        assert!(is_url("https://example.com/a.rs"));
        assert!(is_url("HTTP://example.com"));
        assert!(!is_url("src/https.rs"));
        assert!(!is_url("ftp://example.com"));
        assert!(!is_url("https:"));
        assert!(!is_url(""));
    }

    #[test]
    fn the_content_type_comes_after_the_body() {
        let (body, content_type) = split_body(b"{\"a\":1}\n\napplication/json".to_vec());
        assert_eq!(body, b"{\"a\":1}\n".to_vec());
        assert_eq!(content_type, "application/json".to_string());
        let (body, content_type) = split_body(b"plain".to_vec());
        assert_eq!((body, content_type), (b"plain".to_vec(), String::new()));
    }

    #[test]
    fn the_language_comes_from_the_extension_then_the_content_type() {
        let loader = helix_core::config::default_lang_loader();
        let id = |url: &str, content_type: &str| language_id(&loader, url, content_type);
        assert_eq!(
            id("https://example.com/src/main.rs?raw=1", "text/plain"),
            Some("rust".to_string())
        );
        assert_eq!(
            id(
                "https://api.github.com/repos/x/y",
                "application/json; charset=utf-8"
            ),
            Some("json".to_string())
        );
        assert_eq!(
            id("https://example.com/", "application/vnd.api+json"),
            Some("json".to_string())
        );
        assert_eq!(id("https://example.com/", "text/plain"), None);
    }
}
