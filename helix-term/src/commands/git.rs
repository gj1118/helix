//! Git integration beyond the diff gutter: a changed-files picker with separate
//! staged and unstaged columns, and stage / unstage / stash actions.
//!
//! Everything here shells out to the `git` executable. Because that can run
//! repository-configured programs (filters, hooks), it is only used in trusted
//! workspaces; otherwise the picker falls back to the built-in status listing.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context as _};
use helix_core::hashmap;
use helix_event::status;
use helix_vcs::FileChange;
use helix_view::{icons::ICONS, theme::Style};
use tui::{
    text::{Span, Spans},
    widgets::Cell,
};

use super::Context;
use crate::{
    alt,
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, PickerColumn, Prompt, PromptEvent},
};

// --- status parsing --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflict,
}

impl ChangeKind {
    /// How much a change deserves to be seen: the folder of two changes shows the stronger one.
    fn weight(self) -> u8 {
        match self {
            Self::Conflict => 6,
            Self::Deleted => 5,
            Self::Modified | Self::TypeChanged => 4,
            Self::Renamed => 3,
            Self::Added | Self::Copied => 2,
            Self::Untracked => 1,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            b'A' => Self::Added,
            b'M' => Self::Modified,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'T' => Self::TypeChanged,
            b'U' => Self::Conflict,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type changed",
            Self::Untracked => "untracked",
            Self::Conflict => "conflict",
        }
    }
}

/// One file in `git status`, with what is staged and what is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitChange {
    pub path: PathBuf,
    /// The old path of a rename or copy.
    pub from: Option<PathBuf>,
    pub staged: Option<ChangeKind>,
    pub unstaged: Option<ChangeKind>,
}

impl GitChange {
    /// Every path a stage or unstage has to name (both sides of a rename).
    fn paths(&self) -> Vec<&Path> {
        let mut paths = vec![self.path.as_path()];
        paths.extend(self.from.as_deref());
        paths
    }
}

/// A path as git printed it. On Unix paths are bytes and need not be UTF-8: a lossy
/// conversion would name a file that doesn't exist.
pub(super) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Parses `git status --porcelain=v1 -z`. Paths are relative to `repo_root`.
pub fn parse_status(output: &[u8], repo_root: &Path) -> Vec<GitChange> {
    let mut changes = Vec::new();
    let mut entries = output.split(|&b| b == 0).filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        let to = repo_root.join(path_from_bytes(&entry[3..]));
        // Renames and copies are followed by the path they came from.
        let from = if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            entries
                .next()
                .map(|from| repo_root.join(path_from_bytes(from)))
        } else {
            None
        };

        let (staged, unstaged) = match (x, y) {
            (b'?', b'?') => (None, Some(ChangeKind::Untracked)),
            (b'!', b'!') => continue,
            // Unmerged paths: either side U, or both added / deleted.
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => {
                (Some(ChangeKind::Conflict), Some(ChangeKind::Conflict))
            }
            _ => (ChangeKind::from_code(x), ChangeKind::from_code(y)),
        };
        changes.push(GitChange {
            path: to,
            from,
            staged,
            unstaged,
        });
    }
    changes
}

// --- running git -----------------------------------------------------------

/// Runs `git -C dir args...`, returning stdout or the error git printed.
pub(super) async fn git(dir: &Path, args: &[OsString]) -> anyhow::Result<Vec<u8>> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        // Paths come back verbatim, not C-quoted: a quoted `"caf\303\251.rs"` would
        // otherwise be taken as the file name of a non-ASCII path.
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("failed to run git (is it on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {}: {}",
            args.first()
                .map(|a| a.to_string_lossy())
                .unwrap_or_default(),
            stderr.trim()
        );
    }
    Ok(output.stdout)
}

pub(super) fn args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

pub(super) async fn repo_root(dir: &Path) -> anyhow::Result<PathBuf> {
    let out = git(dir, &args(["rev-parse", "--show-toplevel"])).await?;
    // Only the line break goes: a folder's name may end in a space.
    Ok(path_from_bytes(strip_newline(&out)))
}

/// `out` without the line break git ends its output with.
pub(super) fn strip_newline(out: &[u8]) -> &[u8] {
    let out = out.strip_suffix(b"\n").unwrap_or(out);
    out.strip_suffix(b"\r").unwrap_or(out)
}

/// `git status` for `dir` (the whole repository when `whole_repo`).
async fn status(dir: &Path, whole_repo: bool) -> anyhow::Result<Vec<GitChange>> {
    let root = repo_root(dir).await?;
    let mut a = args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    if !whole_repo {
        a.push("--".into());
        a.push(dir.into());
    }
    let out = git(dir, &a).await?;
    Ok(parse_status(&out, &root))
}

async fn stage(root: &Path, paths: &[&Path]) -> anyhow::Result<()> {
    let mut a = args(["add", "-A", "--"]);
    a.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    git(root, &a).await.map(drop)
}

async fn unstage(root: &Path, paths: &[&Path]) -> anyhow::Result<()> {
    let has_head = git(root, &args(["rev-parse", "--verify", "-q", "HEAD"]))
        .await
        .is_ok();
    // Before the first commit there is no HEAD to restore from.
    let mut a = if has_head {
        args(["restore", "--staged", "--"])
    } else {
        args(["rm", "--cached", "-r", "-q", "--"])
    };
    a.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    git(root, &a).await.map(drop)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
}

async fn stash_list(root: &Path) -> anyhow::Result<Vec<Stash>> {
    let out = git(root, &args(["stash", "list", "-n", "500", "--format=%gs"])).await?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .enumerate()
        .map(|(index, message)| Stash {
            index,
            message: message.to_owned(),
        })
        .collect())
}

/// The changes under `root` for the explorer: with `git status` in a trusted workspace (so the
/// staged state is known), with the built-in listing otherwise.
pub(crate) async fn changes_under(
    root: PathBuf,
    trusted: bool,
    providers: helix_vcs::DiffProviderRegistry,
) -> anyhow::Result<Vec<GitChange>> {
    if trusted {
        return status(&root, false).await;
    }
    // The built-in listing runs on a blocking thread and is done when it drops the closure, which
    // closes the channel.
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    providers.for_each_changed_file(root.clone(), false, move |change| match change {
        Ok(change) => sender.send(from_file_change(change)).is_ok(),
        Err(err) => {
            log::debug!("git status for the explorer: {err}");
            true
        }
    });
    let mut changes = Vec::new();
    while let Some(change) = receiver.recv().await {
        if change.path.starts_with(&root) {
            changes.push(change);
        }
    }
    Ok(changes)
}

/// The git state of the paths of a tree: what is changed, and which folders contain changes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StatusMap {
    files: std::collections::HashMap<PathBuf, ChangeKind>,
    folders: std::collections::HashMap<PathBuf, ChangeKind>,
}

impl StatusMap {
    /// `root` is where the explorer starts: folders above it are of no interest. Git reports
    /// paths below the real path of the repository, `root` may reach it through a symlink
    /// (`/tmp` on macOS): those paths are moved under `root`.
    pub fn new(changes: &[GitChange], root: &Path) -> Self {
        let real_root = std::fs::canonicalize(root).ok().filter(|real| real != root);
        let mut map = Self::default();
        for change in changes {
            // What is not staged is what the file looks like in the working tree; a file that
            // is only staged is shown as the staged change.
            let Some(kind) = change.unstaged.or(change.staged) else {
                continue;
            };
            let path = match &real_root {
                Some(real) if !change.path.starts_with(root) => {
                    match change.path.strip_prefix(real) {
                        Ok(relative) => root.join(relative),
                        Err(_) => continue,
                    }
                }
                _ => change.path.clone(),
            };
            for folder in path.ancestors().skip(1) {
                if !folder.starts_with(root) {
                    break;
                }
                map.folders
                    .entry(folder.to_path_buf())
                    .and_modify(|seen| {
                        if kind.weight() > seen.weight() {
                            *seen = kind;
                        }
                    })
                    .or_insert(kind);
            }
            map.files.insert(path, kind);
        }
        map
    }

    pub fn file(&self, path: &Path) -> Option<ChangeKind> {
        self.files.get(path).copied()
    }

    pub fn folder(&self, path: &Path) -> Option<ChangeKind> {
        self.folders.get(path).copied()
    }
}

/// The one or two columns a change is drawn with: the glyph of the icon set (`[icons.vcs]`),
/// or a letter when the icons are off.
pub fn change_marker(kind: ChangeKind) -> String {
    let icons = ICONS.load();
    let glyph = match kind {
        ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Copied => icons.vcs().added(),
        ChangeKind::Modified | ChangeKind::TypeChanged => icons.vcs().modified(),
        ChangeKind::Deleted => icons.vcs().removed(),
        ChangeKind::Renamed => icons.vcs().renamed(),
        ChangeKind::Conflict => icons.vcs().conflict(),
    };
    if !glyph.is_empty() {
        return glyph.to_string();
    }
    match kind {
        ChangeKind::Added | ChangeKind::Copied => "A",
        ChangeKind::Untracked => "?",
        ChangeKind::Modified | ChangeKind::TypeChanged => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Conflict => "U",
    }
    .to_string()
}

/// The theme style a change is drawn with, the same as in the changed files picker.
pub fn change_style(theme: &helix_view::Theme, kind: ChangeKind) -> Style {
    ChangeStyles {
        added: theme.get("diff.plus"),
        modified: theme.get("diff.delta"),
        deleted: theme.get("diff.minus"),
        renamed: theme.get("diff.delta.moved"),
        conflict: theme.get("diff.delta.conflict"),
    }
    .style(kind)
}

/// Whether `git` may be run for the workspace containing `dir`.
pub(crate) fn workspace_trusted(editor: &helix_view::Editor, dir: &Path) -> bool {
    editor
        .workspace_trust
        .query(
            &helix_loader::find_workspace_in(dir).0,
            helix_loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted()
}

// --- picker ----------------------------------------------------------------

pub struct GitPickerData {
    cwd: PathBuf,
    /// Whether `git` may be run for this workspace.
    trusted: bool,
    styles: ChangeStyles,
}

#[derive(Clone, Copy)]
struct ChangeStyles {
    added: Style,
    modified: Style,
    deleted: Style,
    renamed: Style,
    conflict: Style,
}

impl ChangeStyles {
    fn style(&self, kind: ChangeKind) -> Style {
        match kind {
            ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Copied => self.added,
            ChangeKind::Modified | ChangeKind::TypeChanged => self.modified,
            ChangeKind::Deleted => self.deleted,
            ChangeKind::Renamed => self.renamed,
            ChangeKind::Conflict => self.conflict,
        }
    }
}

fn kind_cell(kind: Option<ChangeKind>, data: &GitPickerData) -> Cell<'static> {
    let Some(kind) = kind else {
        return Span::raw("").into();
    };
    let icons = ICONS.load();
    let icon = match kind {
        ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Copied => icons.vcs().added(),
        ChangeKind::Modified | ChangeKind::TypeChanged => icons.vcs().modified(),
        ChangeKind::Deleted => icons.vcs().removed(),
        ChangeKind::Renamed => icons.vcs().renamed(),
        ChangeKind::Conflict => icons.vcs().conflict(),
    };
    Span::styled(format!("{icon}  {}", kind.label()), data.styles.style(kind)).into()
}

fn feed_status(
    editor: &helix_view::Editor,
    cwd: PathBuf,
    trusted: bool,
    injector: ui::picker::Injector<GitChange, GitPickerData>,
) {
    if trusted {
        tokio::spawn(async move {
            match status(&cwd, false).await {
                Ok(changes) => {
                    for change in changes {
                        if injector.push(change).is_err() {
                            break;
                        }
                    }
                }
                Err(err) => status::report(err).await,
            }
        });
    } else {
        // Without trust the built-in listing is used: no staged column.
        editor.diff_providers.clone().for_each_changed_file(
            cwd,
            false,
            move |change| match change {
                Ok(change) => injector.push(from_file_change(change)).is_ok(),
                Err(err) => {
                    status::report_blocking(err);
                    true
                }
            },
        );
    }
}

fn from_file_change(change: FileChange) -> GitChange {
    let (path, from, kind) = match change {
        FileChange::Untracked { path } => (path, None, ChangeKind::Untracked),
        FileChange::Modified { path } => (path, None, ChangeKind::Modified),
        FileChange::Conflict { path } => (path, None, ChangeKind::Conflict),
        FileChange::Deleted { path } => (path, None, ChangeKind::Deleted),
        FileChange::Renamed { from_path, to_path } => {
            (to_path, Some(from_path), ChangeKind::Renamed)
        }
    };
    GitChange {
        path,
        from,
        staged: None,
        unstaged: Some(kind),
    }
}

/// Asks for a yes/no answer, running `on_yes` for "y".
fn confirm(
    cx: &mut compositor::Context,
    question: &'static str,
    on_yes: impl FnOnce(&mut compositor::Context) + Send + 'static,
) {
    cx.jobs.callback(async move {
        Ok(Callback::EditorCompositor(Box::new(
            move |_editor: &mut helix_view::Editor, compositor: &mut Compositor| {
                // The prompt is not `Send`, so it is built on the UI side.
                let mut on_yes = Some(on_yes);
                let prompt = Prompt::new(
                    format!("{question} [y/N]:").into(),
                    None,
                    ui::completers::none,
                    move |cx: &mut compositor::Context, input: &str, event: PromptEvent| {
                        if event == PromptEvent::Validate && input.trim().eq_ignore_ascii_case("y")
                        {
                            if let Some(on_yes) = on_yes.take() {
                                on_yes(cx);
                            }
                        }
                    },
                );
                compositor.push(Box::new(prompt));
            },
        )))
    });
}

/// Runs a git operation in the background and reports the outcome.
fn run_operation(
    cx: &mut compositor::Context,
    done: String,
    operation: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
) {
    cx.jobs.callback(async move {
        let result = operation.await;
        Ok(Callback::Editor(Box::new(move |editor| {
            match result {
                Ok(()) => {
                    editor.set_status(done);
                    // Files may have changed on disk (stash): pick the changes up.
                    helix_event::send_blocking(
                        &editor.handlers.auto_reload,
                        helix_view::handlers::AutoReloadEvent::CheckForChanges { after: 0 },
                    );
                }
                Err(err) => editor.set_error(err.to_string()),
            }
        })))
    });
}

fn require_trust(cx: &mut compositor::Context, data: &GitPickerData) -> bool {
    if !data.trusted {
        cx.editor
            .set_error("Git actions need a trusted workspace (see :workspace-trust)");
    }
    data.trusted
}

fn stage_selected(
    cx: &mut compositor::Context,
    change: &GitChange,
    data: Arc<GitPickerData>,
    _: u32,
) {
    if !require_trust(cx, &data) {
        return;
    }
    let (cwd, change) = (data.cwd.clone(), change.clone());
    let label = format!("Staged {}", change.path.display());
    run_operation(cx, label, async move {
        let root = repo_root(&cwd).await?;
        stage(&root, &change.paths()).await
    });
}

fn unstage_selected(
    cx: &mut compositor::Context,
    change: &GitChange,
    data: Arc<GitPickerData>,
    _: u32,
) {
    if !require_trust(cx, &data) {
        return;
    }
    let (cwd, change) = (data.cwd.clone(), change.clone());
    let label = format!("Unstaged {}", change.path.display());
    run_operation(cx, label, async move {
        let root = repo_root(&cwd).await?;
        unstage(&root, &change.paths()).await
    });
}

fn stash_all(cx: &mut compositor::Context, _: &GitChange, data: Arc<GitPickerData>, _: u32) {
    if !require_trust(cx, &data) {
        return;
    }
    if cx.editor.documents().any(|doc| doc.is_modified()) {
        cx.editor.set_error("Save modified buffers before stashing");
        return;
    }
    let cwd = data.cwd.clone();
    confirm(
        cx,
        "Stash all changes (including untracked files)?",
        move |cx| {
            run_operation(cx, "Stashed all changes".into(), async move {
                let root = repo_root(&cwd).await?;
                git(&root, &args(["stash", "push", "--include-untracked"]))
                    .await
                    .map(drop)
            });
        },
    );
}

fn browse_stashes(cx: &mut compositor::Context, _: &GitChange, data: Arc<GitPickerData>, _: u32) {
    if !require_trust(cx, &data) {
        return;
    }
    open_stash_picker_from(cx.jobs, data.cwd.clone());
}

/// The changed-files picker (`Space g`).
pub fn changed_file_picker(cx: &mut Context) {
    let cwd = helix_stdx::env::current_working_dir();
    if !cwd.exists() {
        cx.editor
            .set_error("Current working directory does not exist");
        return;
    }
    let trusted = cx
        .editor
        .workspace_trust
        .query(
            &helix_loader::find_workspace_in(&cwd).0,
            helix_loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted();
    let theme = &cx.editor.theme;
    let data = GitPickerData {
        cwd: cwd.clone(),
        trusted,
        styles: ChangeStyles {
            added: theme.get("diff.plus"),
            modified: theme.get("diff.delta"),
            deleted: theme.get("diff.minus"),
            renamed: theme.get("diff.delta.moved"),
            conflict: theme.get("diff.delta.conflict"),
        },
    };

    let columns = [
        PickerColumn::new("staged", |change: &GitChange, data: &GitPickerData| {
            kind_cell(change.staged, data)
        }),
        PickerColumn::new("unstaged", |change: &GitChange, data: &GitPickerData| {
            kind_cell(change.unstaged, data)
        }),
        PickerColumn::new("path", |change: &GitChange, data: &GitPickerData| {
            let show = |path: &Path| {
                path.strip_prefix(&data.cwd)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            };
            match &change.from {
                Some(from) => format!("{} -> {}", show(from), show(&change.path)),
                None => show(&change.path),
            }
            .into()
        }),
    ];

    let handlers: ui::picker::PickerKeyHandlers<GitChange, GitPickerData> = hashmap! {
        alt!('s') => Box::new(stage_selected) as ui::picker::PickerKeyHandler<_, _>,
        alt!('u') => Box::new(unstage_selected) as ui::picker::PickerKeyHandler<_, _>,
        alt!('z') => Box::new(stash_all) as ui::picker::PickerKeyHandler<_, _>,
        alt!('l') => Box::new(browse_stashes) as ui::picker::PickerKeyHandler<_, _>,
    };

    let picker = Picker::new(
        columns,
        2, // path
        [],
        data,
        |cx, change: &GitChange, action| {
            // A deleted file cannot be opened.
            let path = &change.path;
            if let Err(e) = cx.editor.open(path, action) {
                let err = match std::error::Error::source(&e) {
                    Some(err) => err.to_string(),
                    None => format!("unable to open \"{}\"", path.display()),
                };
                cx.editor.set_error(err);
            }
        },
    )
    .with_preview(|_editor, change| Some((change.path.as_path().into(), None)))
    .with_key_handlers(handlers)
    .with_title("Changed Files");

    feed_status(cx.editor, cwd.clone(), trusted, picker.injector());

    // Follow edits, staging and commits made while the picker is open.
    let watch = crate::fs_watch::FsWatch::workspace(&cwd);
    let picker = picker.with_live_refresh(
        move |_editor| watch.as_ref().map_or(0, |watch| watch.generation()),
        Some(Box::new(move |editor, injector| {
            feed_status(editor, cwd.clone(), trusted, injector.clone())
        })),
        |change| change.path.display().to_string(),
    );

    cx.push_layer(Box::new(overlaid(
        picker,
        cx.editor.config().fullscreen_overlay,
    )));
}

// --- blame popup -----------------------------------------------------------

/// The contents of the blame popup. Handles the copy keys; scrolling and
/// closing are the popup's.
pub struct BlameDetailsView {
    text: ui::Text,
    commit: Option<String>,
    message: String,
}

impl BlameDetailsView {
    fn new(details: &helix_vcs::BlameDetails) -> Self {
        let dim = Style::default().add_modifier(helix_view::graphics::Modifier::DIM);
        let bold = Style::default().add_modifier(helix_view::graphics::Modifier::BOLD);
        let field = |label: &'static str, value: String| {
            Spans::from(vec![
                Span::styled(format!("{label:<8}"), dim),
                Span::raw(value),
            ])
        };

        let mut lines = vec![field(
            "commit",
            details.commit.clone().unwrap_or_else(|| "unknown".into()),
        )];
        let author = match (&details.author, &details.email) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name.clone(),
            (None, Some(email)) => format!("<{email}>"),
            (None, None) => "unknown".into(),
        };
        lines.push(field("author", author));
        let date = match (&details.date, &details.time_ago) {
            (Some(date), Some(ago)) => format!("{date} ({ago})"),
            (Some(date), None) => date.clone(),
            (None, Some(ago)) => ago.clone(),
            (None, None) => "unknown".into(),
        };
        lines.push(field("date", date));
        lines.push(Spans::default());
        let message = details.message();
        let mut message_lines = message.lines();
        if let Some(title) = message_lines.next() {
            lines.push(Spans::from(Span::styled(title.to_owned(), bold)));
        }
        lines.extend(message_lines.map(|line| Spans::from(line.to_owned())));
        lines.push(Spans::default());
        lines.push(Spans::from(Span::styled(
            "y copy commit id · Y copy message · Esc close",
            dim,
        )));

        Self {
            text: tui::text::Text::from(lines).into(),
            commit: details.commit.clone(),
            message,
        }
    }

    fn copy(&self, editor: &mut helix_view::Editor, what: &str, value: String) {
        if value.is_empty() {
            editor.set_error(format!("No {what} to copy"));
            return;
        }
        match editor.registers.write('+', vec![value]) {
            Ok(()) => editor.set_status(format!("Copied the {what} to the clipboard")),
            Err(err) => editor.set_error(err.to_string()),
        }
    }
}

impl compositor::Component for BlameDetailsView {
    fn handle_event(
        &mut self,
        event: &helix_view::input::Event,
        cx: &mut compositor::Context,
    ) -> compositor::EventResult {
        use compositor::EventResult;
        let helix_view::input::Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        // Both keys close the popup once copied.
        let close: compositor::Callback = Box::new(|compositor, _| {
            compositor.remove(BLAME_POPUP_ID);
        });
        match *key {
            crate::key!('y') => {
                let commit = self.commit.clone().unwrap_or_default();
                self.copy(cx.editor, "commit id", commit);
                EventResult::Consumed(Some(close))
            }
            crate::key!('Y') => {
                let message = self.message.clone();
                self.copy(cx.editor, "commit message", message);
                EventResult::Consumed(Some(close))
            }
            _ => EventResult::Ignored(None),
        }
    }

    fn render(
        &mut self,
        area: helix_view::graphics::Rect,
        surface: &mut tui::buffer::Buffer,
        cx: &mut compositor::Context,
    ) {
        self.text.render(area, surface, cx)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.text.required_size(viewport)
    }
}

const BLAME_POPUP_ID: &str = "blame-details";

pub fn show_blame_popup(compositor: &mut Compositor, details: helix_vcs::BlameDetails) {
    let popup = ui::Popup::new(BLAME_POPUP_ID, BlameDetailsView::new(&details))
        .auto_close(false)
        .panel_border(true);
    compositor.replace_or_push(BLAME_POPUP_ID, popup);
}

// --- stash picker ----------------------------------------------------------

pub fn git_stash_picker(cx: &mut Context) {
    let cwd = helix_stdx::env::current_working_dir();
    let trusted = cx
        .editor
        .workspace_trust
        .query(
            &helix_loader::find_workspace_in(&cwd).0,
            helix_loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted();
    if !trusted {
        cx.editor
            .set_error("Git actions need a trusted workspace (see :workspace-trust)");
        return;
    }
    open_stash_picker_from(cx.jobs, cwd);
}

fn open_stash_picker_from(jobs: &mut Jobs, cwd: PathBuf) {
    jobs.callback(async move {
        let stashes = stash_list(&cwd).await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut helix_view::Editor, compositor: &mut Compositor| {
                if stashes.is_empty() {
                    editor.set_status("No stashes");
                    return;
                }
                let picker = stash_picker(stashes, cwd);
                compositor.push(Box::new(overlaid(
                    picker,
                    editor.config().fullscreen_overlay,
                )));
            },
        )))
    });
}

fn stash_picker(stashes: Vec<Stash>, cwd: PathBuf) -> Picker<Stash, PathBuf> {
    fn apply(cx: &mut compositor::Context, stash: &Stash, cwd: PathBuf, with_index: bool) {
        if cx.editor.documents().any(|doc| doc.is_modified()) {
            cx.editor
                .set_error("Save modified buffers before applying a stash");
            return;
        }
        let index = stash.index;
        confirm(cx, "Apply the stash (it is kept)?", move |cx| {
            run_operation(cx, format!("Applied stash@{{{index}}}"), async move {
                let root = repo_root(&cwd).await?;
                let mut a = args(["stash", "apply"]);
                if with_index {
                    a.push("--index".into());
                }
                a.push(format!("stash@{{{index}}}").into());
                git(&root, &a).await.map(drop)
            });
        });
    }

    let columns = [
        PickerColumn::new("stash", |stash: &Stash, _: &PathBuf| {
            format!("stash@{{{}}}", stash.index).into()
        }),
        PickerColumn::new("message", |stash: &Stash, _: &PathBuf| {
            stash.message.as_str().into()
        }),
    ];
    let handlers: ui::picker::PickerKeyHandlers<Stash, PathBuf> = hashmap! {
        alt!('s') => Box::new(|cx: &mut compositor::Context, stash: &Stash, cwd: Arc<PathBuf>, _| {
            apply(cx, stash, (*cwd).clone(), false)
        }) as ui::picker::PickerKeyHandler<_, _>,
        alt!('i') => Box::new(|cx: &mut compositor::Context, stash: &Stash, cwd: Arc<PathBuf>, _| {
            apply(cx, stash, (*cwd).clone(), true)
        }) as ui::picker::PickerKeyHandler<_, _>,
    };

    Picker::new(columns, 1, stashes, cwd, |cx, stash: &Stash, _action| {
        // Enter applies the stash, after confirmation; it is always kept.
        let cwd = helix_stdx::env::current_working_dir();
        apply(cx, stash, cwd, false);
    })
    .with_key_handlers(handlers)
    .with_title("Stashes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<GitChange> {
        parse_status(input.as_bytes(), Path::new("/repo"))
    }

    #[test]
    fn staged_unstaged_and_untracked() {
        let changes = parse("M  a.rs\0 M b.rs\0MM c.rs\0A  d.rs\0?? e.rs\0 D f.rs\0D  g.rs\0");
        let kinds: Vec<_> = changes.iter().map(|c| (c.staged, c.unstaged)).collect();
        use ChangeKind::*;
        assert_eq!(
            kinds,
            [
                (Some(Modified), None),
                (None, Some(Modified)),
                (Some(Modified), Some(Modified)),
                (Some(Added), None),
                (None, Some(Untracked)),
                (None, Some(Deleted)),
                (Some(Deleted), None),
            ]
        );
        assert_eq!(changes[0].path, Path::new("/repo/a.rs"));
    }

    #[test]
    fn renames_carry_their_old_path() {
        let changes = parse("R  new name.rs\0old name.rs\0 M other.rs\0");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].staged, Some(ChangeKind::Renamed));
        assert_eq!(changes[0].path, Path::new("/repo/new name.rs"));
        assert_eq!(
            changes[0].from.as_deref(),
            Some(Path::new("/repo/old name.rs"))
        );
        assert_eq!(changes[0].paths().len(), 2);
        // the entry after the rename is parsed normally
        assert_eq!(changes[1].unstaged, Some(ChangeKind::Modified));
    }

    #[test]
    fn conflicts_and_ignored() {
        let changes = parse("UU both.rs\0AA added.rs\0DD gone.rs\0!! ignored.rs\0");
        assert_eq!(changes.len(), 3);
        assert!(changes
            .iter()
            .all(|c| c.staged == Some(ChangeKind::Conflict)));
    }

    /// File names that are not UTF-8 come back as they are on disk.
    #[cfg(unix)]
    #[test]
    fn paths_keep_their_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let changes = parse_status(b"?? caf\xe9.rs\0R  n\xff\0o\xfe\0", Path::new("/repo"));
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path.as_os_str().as_bytes(), b"/repo/caf\xe9.rs");
        assert_eq!(changes[1].path.as_os_str().as_bytes(), b"/repo/n\xff");
        assert_eq!(
            changes[1].from.as_deref().map(|p| p.as_os_str().as_bytes()),
            Some(&b"/repo/o\xfe"[..])
        );
        assert_eq!(strip_newline(b"/a b \n"), b"/a b ");
        assert_eq!(strip_newline(b"/a\r\n"), b"/a");
    }

    #[test]
    fn garbage_is_skipped() {
        assert!(parse("").is_empty());
        assert!(parse("x\0\0M").is_empty());
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn stage_unstage_and_stash_against_a_real_repo() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "-q"]);

        // Before the first commit there is no HEAD: unstage must still work.
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        stage(&root, &[Path::new("a.txt")]).await.unwrap();
        let changes = status(&root, true).await.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].staged, Some(ChangeKind::Added));
        unstage(&root, &[Path::new("a.txt")]).await.unwrap();
        let changes = status(&root, true).await.unwrap();
        assert_eq!(
            (changes[0].staged, changes[0].unstaged),
            (None, Some(ChangeKind::Untracked))
        );

        run(&root, &["add", "a.txt"]);
        run(&root, &["commit", "-q", "-m", "init"]);

        // staged and unstaged edits to the same file show on both sides
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        stage(&root, &[Path::new("a.txt")]).await.unwrap();
        std::fs::write(root.join("a.txt"), "c\n").unwrap();
        let changes = status(&root, true).await.unwrap();
        assert_eq!(
            (changes[0].staged, changes[0].unstaged),
            (Some(ChangeKind::Modified), Some(ChangeKind::Modified))
        );
        assert_eq!(changes[0].path, root.join("a.txt"));
        unstage(&root, &[Path::new("a.txt")]).await.unwrap();
        let changes = status(&root, true).await.unwrap();
        assert_eq!(
            (changes[0].staged, changes[0].unstaged),
            (None, Some(ChangeKind::Modified))
        );

        // stash (with untracked files) and list
        std::fs::write(root.join("new.txt"), "x\n").unwrap();
        git(&root, &args(["stash", "push", "--include-untracked"]))
            .await
            .unwrap();
        assert!(status(&root, true).await.unwrap().is_empty());
        let stashes = stash_list(&root).await.unwrap();
        assert_eq!(stashes.len(), 1);
        assert_eq!(stashes[0].index, 0);
        git(&root, &args(["stash", "apply", "stash@{0}"]))
            .await
            .unwrap();
        assert_eq!(status(&root, true).await.unwrap().len(), 2);
    }

    fn change(path: &str, staged: Option<ChangeKind>, unstaged: Option<ChangeKind>) -> GitChange {
        GitChange {
            path: PathBuf::from(path),
            from: None,
            staged,
            unstaged,
        }
    }

    #[test]
    fn folders_show_the_strongest_change_below_them() {
        let root = Path::new("/repo/src");
        let map = StatusMap::new(
            &[
                change("/repo/src/a/new.rs", None, Some(ChangeKind::Untracked)),
                change("/repo/src/a/b/lib.rs", None, Some(ChangeKind::Modified)),
                change("/repo/src/c.rs", Some(ChangeKind::Added), None),
                change("/repo/elsewhere.rs", None, Some(ChangeKind::Modified)),
            ],
            root,
        );
        assert_eq!(
            map.file(Path::new("/repo/src/a/new.rs")),
            Some(ChangeKind::Untracked)
        );
        // staged only: shown as the staged change
        assert_eq!(
            map.file(Path::new("/repo/src/c.rs")),
            Some(ChangeKind::Added)
        );
        assert_eq!(
            map.folder(Path::new("/repo/src/a")),
            Some(ChangeKind::Modified)
        );
        assert_eq!(
            map.folder(Path::new("/repo/src/a/b")),
            Some(ChangeKind::Modified)
        );
        // the root itself is marked as well (the panel does not draw it), nothing above it is
        assert_eq!(map.folder(Path::new("/repo")), None);
        assert_eq!(map.file(Path::new("/repo/src/untouched.rs")), None);
    }

    /// Git reports the real path of the repository, the explorer may reach it through a link
    /// (`/tmp` is one on macOS).
    #[cfg(unix)]
    #[test]
    fn paths_of_a_repository_behind_a_link_are_moved_under_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap().join("real");
        std::fs::create_dir_all(real.join("sub")).unwrap();
        let link = dir.path().canonicalize().unwrap().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let map = StatusMap::new(
            &[change(
                real.join("sub/a.rs").to_str().unwrap(),
                None,
                Some(ChangeKind::Modified),
            )],
            &link,
        );
        assert_eq!(map.file(&link.join("sub/a.rs")), Some(ChangeKind::Modified));
        assert_eq!(map.folder(&link.join("sub")), Some(ChangeKind::Modified));
    }

    #[test]
    fn a_conflict_wins_over_other_changes_in_a_folder() {
        let root = Path::new("/repo");
        let map = StatusMap::new(
            &[
                change("/repo/d/x.rs", None, Some(ChangeKind::Conflict)),
                change("/repo/d/y.rs", None, Some(ChangeKind::Modified)),
                change("/repo/d/z.rs", None, Some(ChangeKind::Untracked)),
            ],
            root,
        );
        assert_eq!(map.folder(Path::new("/repo/d")), Some(ChangeKind::Conflict));
    }

    #[test]
    fn markers_fall_back_to_letters_without_icons() {
        // the default icon set is off
        assert_eq!(change_marker(ChangeKind::Modified), "M");
        assert_eq!(change_marker(ChangeKind::Untracked), "?");
        assert_eq!(change_marker(ChangeKind::Conflict), "U");
    }

    #[tokio::test]
    async fn changes_under_lists_a_repository_both_ways() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &args(["init", "-q"])).await.unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &args(["add", "a.txt"])).await.unwrap();
        git(
            &root,
            &args([
                "-c",
                "user.name=a",
                "-c",
                "user.email=a@b.c",
                "commit",
                "-q",
                "-m",
                "x",
            ]),
        )
        .await
        .unwrap();
        std::fs::write(root.join("a.txt"), "changed\n").unwrap();
        std::fs::write(root.join("new.txt"), "n\n").unwrap();

        let kinds = |changes: Vec<GitChange>| {
            let mut kinds: Vec<_> = changes
                .into_iter()
                .map(|c| (c.path.file_name().unwrap().to_owned(), c.unstaged))
                .collect();
            kinds.sort_by(|a, b| a.0.cmp(&b.0));
            kinds
        };
        let expected = vec![
            ("a.txt".into(), Some(ChangeKind::Modified)),
            ("new.txt".into(), Some(ChangeKind::Untracked)),
        ];
        // with git itself, and with the built-in listing of an untrusted workspace
        let trusted = changes_under(root.clone(), true, Default::default())
            .await
            .unwrap();
        assert_eq!(kinds(trusted), expected);
        let untrusted = changes_under(root, false, Default::default())
            .await
            .unwrap();
        assert_eq!(kinds(untrusted), expected);
    }

    #[tokio::test]
    async fn git_errors_carry_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let err = git(dir.path(), &args(["definitely-not-a-command"]))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("git definitely-not-a-command"));
    }
}
