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

/// Parses `git status --porcelain=v1 -z`. Paths are relative to `repo_root`.
pub fn parse_status(output: &[u8], repo_root: &Path) -> Vec<GitChange> {
    let mut changes = Vec::new();
    let mut entries = output.split(|&b| b == 0).filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        let to = repo_root.join(String::from_utf8_lossy(&entry[3..]).as_ref());
        // Renames and copies are followed by the path they came from.
        let from = if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            entries
                .next()
                .map(|from| repo_root.join(String::from_utf8_lossy(from).as_ref()))
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
async fn git(dir: &Path, args: &[OsString]) -> anyhow::Result<Vec<u8>> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
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

fn args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

async fn repo_root(dir: &Path) -> anyhow::Result<PathBuf> {
    let out = git(dir, &args(["rev-parse", "--show-toplevel"])).await?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out).trim_end()))
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

    #[tokio::test]
    async fn git_errors_carry_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let err = git(dir.path(), &args(["definitely-not-a-command"]))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("git definitely-not-a-command"));
    }
}
