//! Browsing git history: the commits that changed the current file, the
//! commits leading to `HEAD`, the files a commit changed, their diffs and
//! older versions. Nothing here checks anything out or touches the working
//! tree; diffs and old versions open in scratch buffers.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context as _};
use helix_core::{hashmap, Rope};
use helix_view::{current_ref, doc_mut, document::Document, editor::Action, Editor};

use super::git::{args, git, repo_root, workspace_trusted};
use crate::{
    alt,
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, PickerColumn},
};

/// Commits loaded into one picker.
const MAX_COMMITS: usize = 5000;

// --- parsing ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// The file's path in this commit (file history follows renames).
    pub path: Option<PathBuf>,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

/// Header of `git log --format=%x1e%H%x1f%P%x1f%an%x1f%ad%x1f%s`, optionally
/// followed by `--name-only` output.
const LOG_FORMAT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%ad%x1f%s";

pub fn parse_log(output: &str) -> Vec<Commit> {
    output
        .split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let (header, files) = record.split_once('\n').unwrap_or((record, ""));
            let mut fields = header.split('\x1f');
            let hash = fields.next()?.trim().to_owned();
            let parents = fields
                .next()?
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            let author = fields.next()?.to_owned();
            let date = fields.next()?.to_owned();
            let subject = fields.next()?.to_owned();
            if hash.is_empty() {
                return None;
            }
            let path = files
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(PathBuf::from);
            Some(Commit {
                hash,
                parents,
                author,
                date,
                subject,
                path,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    /// `A`dded, `M`odified, `D`eleted, `R`enamed, `C`opied, `T`ype changed.
    pub status: char,
    pub path: PathBuf,
    pub from: Option<PathBuf>,
}

/// Parses `git diff-tree --name-status -z`.
pub fn parse_name_status(output: &[u8]) -> Vec<CommitFile> {
    let mut parts = output
        .split(|&b| b == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .filter(|part| !part.is_empty());
    let mut files = Vec::new();
    while let Some(status) = parts.next() {
        let Some(code) = status.chars().next() else {
            continue;
        };
        let Some(first) = parts.next() else { break };
        if matches!(code, 'R' | 'C') {
            let Some(second) = parts.next() else { break };
            files.push(CommitFile {
                status: code,
                path: second.into(),
                from: Some(first.into()),
            });
        } else {
            files.push(CommitFile {
                status: code,
                path: first.into(),
                from: None,
            });
        }
    }
    files
}

// --- git queries -----------------------------------------------------------

async fn log(root: &Path, file: Option<&Path>) -> anyhow::Result<Vec<Commit>> {
    let mut a = args(["log", "--date=short", "--no-color"]);
    a.push(format!("-n{MAX_COMMITS}").into());
    a.push(LOG_FORMAT.into());
    if let Some(file) = file {
        // Follow renames; the name at each commit comes with --name-only.
        a.push("--follow".into());
        a.push("--name-only".into());
        a.push("--".into());
        a.push(file.into());
    }
    let out = git(root, &a).await?;
    Ok(parse_log(&String::from_utf8_lossy(&out)))
}

async fn commit_files(root: &Path, commit: &Commit) -> anyhow::Result<Vec<CommitFile>> {
    let mut a = args([
        "diff-tree",
        "-r",
        "-M",
        "--name-status",
        "-z",
        "--no-commit-id",
    ]);
    match commit.parents.first() {
        Some(parent) => a.push(parent.into()),
        None => a.push("--root".into()),
    }
    a.push((&commit.hash).into());
    Ok(parse_name_status(&git(root, &a).await?))
}

/// The commit's header and its changes to `paths` (all files when empty), against
/// the first parent.
async fn commit_diff(root: &Path, commit: &Commit, paths: &[&Path]) -> anyhow::Result<String> {
    let mut a = args([
        "show",
        "--no-color",
        "-M",
        "--format=fuller",
        "--first-parent",
        "-m",
    ]);
    a.push((&commit.hash).into());
    if !paths.is_empty() {
        a.push("--".into());
        a.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    }
    Ok(String::from_utf8_lossy(&git(root, &a).await?).into_owned())
}

/// The file as it was in `commit`.
async fn file_at(root: &Path, commit: &Commit, path: &Path) -> anyhow::Result<String> {
    let mut a = args(["show"]);
    a.push(format!("{}:{}", commit.hash, path.display()).into());
    let out = git(root, &a).await?;
    if out.contains(&0) {
        bail!("{} is a binary file at {}", path.display(), commit.short());
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// --- buffers ---------------------------------------------------------------

/// Opens `text` in a scratch buffer named `title`. `syntax_path` picks the
/// highlighting by file name; `language` by language id.
fn open_scratch(
    editor: &mut Editor,
    action: Action,
    title: String,
    text: String,
    syntax: Syntax<'_>,
) {
    let mut doc = Document::from(
        Rope::from(text),
        None,
        Arc::clone(&editor.config),
        Arc::clone(&editor.syn_loader),
    );
    doc.title = Some(title);
    doc.readonly = true;
    let loader = editor.syn_loader.load();
    match syntax {
        Syntax::Id(id) => {
            let _ = doc.set_language_by_language_id(id, &loader);
        }
        Syntax::FileName(path) => {
            if let Some(language) = loader.language_for_filename(path) {
                doc.set_language(Some(loader.language(language).config().clone()), &loader);
            }
        }
    }
    editor.new_file_from_document(action, doc);
    // A buffer nobody wrote cannot be "modified"; keep it clean.
    let doc = doc_mut!(editor);
    doc.reset_modified();
}

enum Syntax<'a> {
    Id(&'a str),
    FileName(&'a Path),
}

// --- pickers ---------------------------------------------------------------

struct HistoryData {
    root: PathBuf,
}

fn commit_columns() -> [PickerColumn<Commit, HistoryData>; 4] {
    [
        PickerColumn::new("commit", |commit: &Commit, _: &HistoryData| {
            commit.short().to_owned().into()
        }),
        PickerColumn::new("date", |commit: &Commit, _: &HistoryData| {
            commit.date.as_str().into()
        }),
        PickerColumn::new("author", |commit: &Commit, _: &HistoryData| {
            commit.author.as_str().into()
        }),
        PickerColumn::new("subject", |commit: &Commit, _: &HistoryData| {
            commit.subject.as_str().into()
        }),
    ]
}

fn yank_commit(cx: &mut compositor::Context, commit: &Commit) {
    match cx.editor.registers.write('+', vec![commit.hash.clone()]) {
        Ok(()) => cx
            .editor
            .set_status(format!("Copied commit {} to the clipboard", commit.short())),
        Err(err) => cx.editor.set_error(err.to_string()),
    }
}

/// Opens the diff of `commit` (limited to `paths`) in a scratch buffer.
fn open_commit_diff(
    jobs: &mut Jobs,
    root: PathBuf,
    commit: Commit,
    paths: Vec<PathBuf>,
    action: Action,
) {
    jobs.callback(async move {
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let text = commit_diff(&root, &commit, &refs).await?;
        let name = match paths.first() {
            Some(path) => format!("{} @ {} [diff]", path.display(), commit.short()),
            None => format!("{} [diff]", commit.short()),
        };
        Ok(Callback::Editor(Box::new(move |editor| {
            open_scratch(editor, action, name, text, Syntax::Id("diff"));
        })))
    });
}

/// Opens `path` as it was in `commit` in a scratch buffer.
fn open_old_version(jobs: &mut Jobs, root: PathBuf, commit: Commit, path: PathBuf, action: Action) {
    jobs.callback(async move {
        let text = file_at(&root, &commit, &path).await?;
        let name = format!("{} @ {}", path.display(), commit.short());
        Ok(Callback::Editor(Box::new(move |editor| {
            open_scratch(editor, action, name, text, Syntax::FileName(&path));
        })))
    });
}

/// The commits picker. With `file`, the commits that changed it (a path
/// relative to `root`); without, the repository's history.
fn commit_picker(
    commits: Vec<Commit>,
    root: PathBuf,
    file: Option<PathBuf>,
) -> Picker<Commit, HistoryData> {
    let title = if file.is_some() {
        "File History"
    } else {
        "History"
    };
    let callback = {
        let (root, file) = (root.clone(), file.clone());
        move |cx: &mut compositor::Context, commit: &Commit, action: Action| match &file {
            // File history: the commit's change to the file.
            Some(file) => {
                let path = commit.path.clone().unwrap_or_else(|| file.clone());
                open_commit_diff(cx.jobs, root.clone(), commit.clone(), vec![path], action);
            }
            // Repository history: the files the commit changed.
            None => open_commit_files(cx.jobs, root.clone(), commit.clone()),
        }
    };
    let picker = Picker::new(commit_columns(), 3, commits, HistoryData { root }, callback);

    let mut handlers: ui::picker::PickerKeyHandlers<Commit, HistoryData> = hashmap! {
        alt!('y') => Box::new(
            |cx: &mut compositor::Context, commit: &Commit, _: Arc<HistoryData>, _| {
                yank_commit(cx, commit)
            },
        ) as ui::picker::PickerKeyHandler<_, _>,
    };
    if let Some(file) = file {
        // Alt-o: the file as it was at the selected commit.
        handlers.insert(
            alt!('o'),
            Box::new(
                move |cx: &mut compositor::Context, commit: &Commit, data: Arc<HistoryData>, _| {
                    let path = commit.path.clone().unwrap_or_else(|| file.clone());
                    open_old_version(
                        cx.jobs,
                        data.root.clone(),
                        commit.clone(),
                        path,
                        Action::Replace,
                    );
                },
            ),
        );
    }
    picker.with_key_handlers(handlers).with_title(title)
}

/// The files `commit` changed; Enter shows one file's diff.
fn open_commit_files(jobs: &mut Jobs, root: PathBuf, commit: Commit) {
    jobs.callback(async move {
        let files = commit_files(&root, &commit).await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                if files.is_empty() {
                    editor.set_status(format!("Commit {} changed no files", commit.short()));
                    return;
                }
                let picker = files_picker(files, root, commit);
                compositor.push(Box::new(overlaid(
                    picker,
                    editor.config().fullscreen_overlay,
                )));
            },
        )))
    });
}

struct FilesData {
    root: PathBuf,
    commit: Commit,
}

fn files_picker(
    files: Vec<CommitFile>,
    root: PathBuf,
    commit: Commit,
) -> Picker<CommitFile, FilesData> {
    let columns = [
        PickerColumn::new("status", |file: &CommitFile, _: &FilesData| {
            match file.status {
                'A' => "added",
                'M' => "modified",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'T' => "type changed",
                _ => "changed",
            }
            .into()
        }),
        PickerColumn::new("path", |file: &CommitFile, _: &FilesData| {
            match &file.from {
                Some(from) => format!("{} -> {}", from.display(), file.path.display()),
                None => file.path.display().to_string(),
            }
            .into()
        }),
    ];

    let handlers: ui::picker::PickerKeyHandlers<CommitFile, FilesData> = hashmap! {
        // Alt-o: the file as it was in this commit.
        alt!('o') => Box::new(
            |cx: &mut compositor::Context, file: &CommitFile, data: Arc<FilesData>, _| {
                if file.status == 'D' {
                    cx.editor.set_error("The file was deleted in this commit");
                    return;
                }
                open_old_version(
                    cx.jobs,
                    data.root.clone(),
                    data.commit.clone(),
                    file.path.clone(),
                    Action::Replace,
                );
            },
        ) as ui::picker::PickerKeyHandler<_, _>,
        // Alt-h: back to the commits.
        alt!('h') => Box::new(
            |cx: &mut compositor::Context, _: &CommitFile, data: Arc<FilesData>, _| {
                spawn_history(cx.jobs, data.root.clone(), None);
            },
        ) as ui::picker::PickerKeyHandler<_, _>,
    };

    let callback = {
        let (root, commit) = (root.clone(), commit.clone());
        move |cx: &mut compositor::Context, file: &CommitFile, action: Action| {
            let mut paths = vec![file.path.clone()];
            paths.extend(file.from.clone());
            open_commit_diff(cx.jobs, root.clone(), commit.clone(), paths, action);
        }
    };
    Picker::new(columns, 1, files, FilesData { root, commit }, callback)
        .with_key_handlers(handlers)
        .with_title("Commit Files")
}

// --- entry points ----------------------------------------------------------

/// Loads the history of `dir`'s repository (or of one file in it) and opens
/// the picker.
fn spawn_history(jobs: &mut Jobs, dir: PathBuf, file: Option<PathBuf>) {
    jobs.callback(async move {
        let root = repo_root(&dir).await?;
        // The file relative to the repository root, whatever way `dir` reaches it.
        let file = match file {
            Some(file) => {
                let parent = file.parent().unwrap_or(&dir);
                let prefix = git(parent, &args(["rev-parse", "--show-prefix"])).await?;
                let prefix = String::from_utf8_lossy(&prefix).trim().to_owned();
                let name = file.file_name().context("the file has no name")?;
                Some(PathBuf::from(prefix).join(name))
            }
            None => None,
        };
        let commits = log(&root, file.as_deref()).await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                if commits.is_empty() {
                    editor.set_status("No commits");
                    return;
                }
                let picker = commit_picker(commits, root, file);
                compositor.push(Box::new(overlaid(
                    picker,
                    editor.config().fullscreen_overlay,
                )));
            },
        )))
    });
}

/// The repository's commits, from the current file's repository.
pub fn git_history(editor: &mut Editor, jobs: &mut Jobs) {
    let dir = history_dir(editor);
    if !workspace_trusted(editor, &dir) {
        editor.set_error("Git history needs a trusted workspace (see :workspace-trust)");
        return;
    }
    spawn_history(jobs, dir, None);
}

/// The commits that changed the current file.
pub fn git_file_history(editor: &mut Editor, jobs: &mut Jobs) {
    let (_, doc) = current_ref!(editor);
    let Some(path) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("The current buffer is not a file");
        return;
    };
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    if !workspace_trusted(editor, &dir) {
        editor.set_error("Git history needs a trusted workspace (see :workspace-trust)");
        return;
    }
    spawn_history(jobs, dir, Some(path));
}

fn history_dir(editor: &Editor) -> PathBuf {
    let (_, doc) = current_ref!(editor);
    doc.path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(helix_stdx::env::current_working_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_with_and_without_names() {
        let log = "\x1eaaaa1111\x1fbbbb2222 cccc3333\x1fAda\x1f2024-01-02\x1fMerge it\n\x1ebbbb2222\x1f\x1fGrace\x1f2023-12-31\x1fRoot: with colon\n";
        let commits = parse_log(log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["bbbb2222", "cccc3333"]);
        assert_eq!(commits[0].short(), "aaaa1111");
        assert_eq!(commits[0].path, None);
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "Root: with colon");

        let named = "\x1eaaaa1111\x1f\x1fAda\x1f2024-01-02\x1fMove\n\nnew/name.rs\n\x1ebbbb2222\x1f\x1fAda\x1f2024-01-01\x1fAdd\n\nold/name.rs\n";
        let commits = parse_log(named);
        assert_eq!(commits[0].path.as_deref(), Some(Path::new("new/name.rs")));
        assert_eq!(commits[1].path.as_deref(), Some(Path::new("old/name.rs")));
    }

    #[test]
    fn parses_name_status() {
        let files = parse_name_status(b"M\0a.rs\0R100\0old.rs\0new.rs\0D\0gone.rs\0A\0sp ace.rs\0");
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].status, 'M');
        assert_eq!(files[1].status, 'R');
        assert_eq!(files[1].from.as_deref(), Some(Path::new("old.rs")));
        assert_eq!(files[1].path, Path::new("new.rs"));
        assert_eq!(files[3].path, Path::new("sp ace.rs"));
        assert!(parse_name_status(b"").is_empty());
        // a truncated record is dropped, not misread
        assert_eq!(parse_name_status(b"M\0a.rs\0R100\0old.rs").len(), 1);
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Ada",
                "-c",
                "user.email=ada@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn history_follows_renames_and_shows_diffs_and_old_versions() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "-q", "-b", "main"]);

        std::fs::write(root.join("old.txt"), "one\ntwo\nthree\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "Add old"]);
        run(&root, &["mv", "old.txt", "new.txt"]);
        run(&root, &["commit", "-q", "-m", "Rename"]);
        std::fs::write(root.join("new.txt"), "one\nTWO\nthree\n").unwrap();
        run(&root, &["commit", "-q", "-am", "Shout two"]);

        // the file's history follows the rename and records the name at each commit
        let commits = log(&root, Some(Path::new("new.txt"))).await.unwrap();
        let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["Shout two", "Rename", "Add old"]);
        assert_eq!(commits[0].path.as_deref(), Some(Path::new("new.txt")));
        assert_eq!(commits[2].path.as_deref(), Some(Path::new("old.txt")));

        // the repository history has every commit and no file names
        let all = log(&root, None).await.unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|c| c.path.is_none()));

        // the diff of a commit for a path
        let diff = commit_diff(&root, &commits[0], &[Path::new("new.txt")])
            .await
            .unwrap();
        assert!(diff.contains("Shout two"));
        assert!(diff.contains("-two") && diff.contains("+TWO"));

        // old versions
        let old = file_at(&root, &commits[2], Path::new("old.txt"))
            .await
            .unwrap();
        assert_eq!(old, "one\ntwo\nthree\n");
        assert!(file_at(&root, &commits[0], Path::new("old.txt"))
            .await
            .is_err());

        // changed files: a rename, and a root commit (no parent)
        let files = commit_files(&root, &commits[1]).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, 'R');
        assert_eq!(files[0].from.as_deref(), Some(Path::new("old.txt")));
        let files = commit_files(&root, &commits[2]).await.unwrap();
        assert_eq!(files[0].status, 'A');
        assert!(commit_diff(&root, &commits[2], &[])
            .await
            .unwrap()
            .contains("+one"));
    }

    #[tokio::test]
    async fn merges_show_changes_against_the_first_parent() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        run(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "base"]);
        run(&root, &["checkout", "-q", "-b", "side"]);
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "side change"]);
        run(&root, &["checkout", "-q", "main"]);
        std::fs::write(root.join("a.txt"), "a\nmain\n").unwrap();
        run(&root, &["commit", "-q", "-am", "main change"]);
        run(
            &root,
            &["merge", "-q", "--no-ff", "-m", "Merge side", "side"],
        );

        let commits = log(&root, None).await.unwrap();
        let merge = &commits[0];
        assert_eq!(merge.subject, "Merge side");
        assert_eq!(merge.parents.len(), 2);

        // against the first parent the merge brought in b.txt only
        let files = commit_files(&root, merge).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("b.txt"));
        let diff = commit_diff(&root, merge, &[]).await.unwrap();
        assert!(diff.contains("+b"), "{diff}");
        assert!(!diff.contains("+main"), "{diff}");
    }
}
