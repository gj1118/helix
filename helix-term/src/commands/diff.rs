//! Side-by-side diffs: the commands that open them (`:diff`, `:diff-buffers`, `:diff-git`,
//! `hx --diff`), the actions inside them (next change, snap, apply a change from one side to the
//! other...) and the keys of the diff views (`[editor.diff.keys]`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _};
use helix_core::diff_hunks::{hunk_at, line_count, DiffHunk, Side};
use helix_core::{Tendril, Transaction};
use helix_view::diff_view::{DiffConfig, GitFile, OldVersion, ReviewFile, ReviewQueue, Stage};
use helix_view::input::KeyEvent;
use helix_view::{DocumentId, Editor, ViewId};

use super::git::{git, repo_root};
use super::Context;

// --- opening ----------------------------------------------------------------

/// Opens a diff of `old` (a file read now, on the left) and `new` (opened as a buffer, on the
/// right).
pub fn diff_files(editor: &mut Editor, old: &Path, new: &Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(old).with_context(|| format!("cannot read {}", old.display()))?;
    // at startup (`hx --diff`) there is no window yet
    let action = if editor.tree.views().next().is_some() {
        helix_view::editor::Action::Replace
    } else {
        helix_view::editor::Action::VerticalSplit
    };
    let right = editor
        .open(new, action)
        .map_err(|err| anyhow!("cannot open {}: {err}", new.display()))?;
    editor.open_diff(
        OldVersion {
            text: text.into(),
            title: format!("{} (old)", old.display()),
            syntax_path: Some(old.to_path_buf()),
        },
        right,
    );
    Ok(())
}

/// Opens a diff of the buffer `left` (a snapshot of it now) and the buffer `right`.
pub fn diff_buffers(
    editor: &mut Editor,
    left: DocumentId,
    right: DocumentId,
) -> anyhow::Result<()> {
    let doc = editor
        .document(left)
        .ok_or_else(|| anyhow!("no such buffer"))?;
    let old = OldVersion {
        // the buffer's rope, shared
        text: doc.text().clone(),
        title: format!("{} (snapshot)", doc.display_name()),
        syntax_path: doc.path().map(Path::to_path_buf),
    };
    editor.open_diff(old, right);
    Ok(())
}

/// The repository of `path`, and the path of the file in it (`/`-separated).
async fn repo_path(path: &Path) -> anyhow::Result<(PathBuf, String)> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", path.display()))?;
    let root = repo_root(dir).await?;
    // git names the repository by its real path: the file's path too, its links resolved
    // (on macOS, temporary folders are under `/var`, a link to `/private/var`)
    let real = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|_| helix_stdx::path::canonicalize(path))
    };
    let root = real(&root);
    // a deleted file has no real path: its folder's then
    let file = match (std::fs::canonicalize(path), path.parent(), path.file_name()) {
        (Ok(file), _, _) => file,
        (Err(_), Some(parent), Some(name)) => real(parent).join(name),
        _ => real(path),
    };
    let relative = file.strip_prefix(&root).map_err(|_| {
        anyhow!(
            "{} is not in the repository {}",
            path.display(),
            root.display()
        )
    })?;
    let relative = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok((root, relative))
}

/// The file `relative` of the repository `root` at `rev` (`None`: in the index); `None` when
/// it isn't there (or there is no commit yet).
async fn show_file(
    root: &Path,
    relative: &str,
    rev: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let spec = format!("{}:{relative}", rev.unwrap_or(""));
    match git(root, &[OsString::from("show"), OsString::from(&spec)]).await {
        Ok(out) => Ok(Some(String::from_utf8_lossy(&out).into_owned())),
        Err(err) => {
            let message = err.to_string();
            let missing = [
                "does not exist",
                "exists on disk, but not in",
                "invalid object name",
            ];
            if missing.iter().any(|missing| message.contains(missing)) {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

/// The version of `path` in git: in the index (`rev` `None`) or at `rev`; empty when the file
/// is not there (a new file). With the title of its buffer. Against the index or `HEAD`, also
/// what staging its changes needs (see [`GitFile`]): `None` for a file git can't stage from
/// here (another revision, a merge conflict).
pub async fn git_version(
    path: &Path,
    rev: Option<&str>,
) -> anyhow::Result<(OldVersion, Option<GitFile>)> {
    let (root, relative) = repo_path(path).await?;
    let label = match rev {
        Some(rev) => format!("{rev}:{relative}"),
        None => format!("index:{relative}"),
    };
    let text = show_file(&root, &relative, rev).await?;
    let git = match rev {
        None => {
            let head = show_file(&root, &relative, Some("HEAD"))
                .await
                .ok()
                .flatten();
            Some(GitFile::new(
                root.clone(),
                relative.clone(),
                head.map(Into::into),
                text.clone().map(Into::into),
                false,
            ))
        }
        Some("HEAD") => match show_file(&root, &relative, None).await {
            Ok(index) => Some(GitFile::new(
                root.clone(),
                relative.clone(),
                text.clone().map(Into::into),
                index.map(Into::into),
                true,
            )),
            // a merge conflict: not at stage 0 in the index
            Err(_) => None,
        },
        Some(_) => None,
    };
    let old = OldVersion {
        text: text.unwrap_or_default().into(),
        title: label,
        syntax_path: Some(PathBuf::from(&relative)),
    };
    Ok((old, git))
}

// --- reviews of changed files ----------------------------------------------

/// The changed files of `dir` (and the folders below) to review, in order: against `rev`
/// (with the files added since), or, without, as `git status` lists them, each against the
/// index when it has changes not staged (what `git diff` shows) and against `HEAD` when its
/// changes are all staged (else the diff would show nothing).
pub async fn review_files(dir: &Path, rev: Option<&str>) -> anyhow::Result<Vec<ReviewFile>> {
    let root = repo_root(dir).await?;
    let mut files: Vec<ReviewFile> = match rev {
        None => super::git::status(dir, false)
            .await?
            .into_iter()
            .map(|change| ReviewFile {
                base: Some("HEAD".to_string()),
                path: change.path,
            })
            .collect(),
        Some(rev) => {
            let changed = |out: Vec<u8>| -> Vec<PathBuf> {
                out.split(|byte| *byte == 0)
                    .filter(|path| !path.is_empty())
                    .map(|path| root.join(super::git::path_from_bytes(path)))
                    .collect()
            };
            let dir_arg = std::ffi::OsString::from(dir);
            let diffed = git(
                &root,
                &[
                    "diff".into(),
                    "--name-only".into(),
                    "-z".into(),
                    rev.into(),
                    "--".into(),
                    dir_arg.clone(),
                ],
            )
            .await?;
            let added = git(
                &root,
                &[
                    "ls-files".into(),
                    "--others".into(),
                    "--exclude-standard".into(),
                    "-z".into(),
                    "--".into(),
                    dir_arg,
                ],
            )
            .await?;
            let mut paths = changed(diffed);
            paths.extend(changed(added));
            paths
                .into_iter()
                .map(|path| ReviewFile {
                    path,
                    base: Some(rev.to_string()),
                })
                .collect()
        }
    };
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Shows file `index` of `files` in a diff, in the focused view (`action` `Replace`), or in a
/// new one (a split), and makes `files` the review (`]f`, `[f`): once its old version is read
/// from git, in the background.
pub fn open_review(
    jobs: &mut crate::job::Jobs,
    files: Vec<ReviewFile>,
    index: usize,
    action: helix_view::editor::Action,
) {
    jobs.callback(review_file(files, index, action));
}

/// What [`open_review`] does in the background: the old version of the file read, the
/// callback that shows it.
pub async fn review_file(
    files: Vec<ReviewFile>,
    index: usize,
    action: helix_view::editor::Action,
) -> anyhow::Result<crate::job::Callback> {
    let file = files
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow!("no changed file {index}"))?;
    let (old, git) = git_version(&file.path, file.base.as_deref()).await?;
    Ok(crate::job::Callback::Editor(Box::new(
        move |editor: &mut Editor| show_review_file(editor, files, index, old, git, action),
    )))
}

/// The diff of file `index` of `files` (whose old version is `old`), and the review made of
/// `files`.
fn show_review_file(
    editor: &mut Editor,
    files: Vec<ReviewFile>,
    index: usize,
    old: OldVersion,
    git: Option<GitFile>,
    action: helix_view::editor::Action,
) {
    use helix_view::editor::Action;
    let path = files[index].path.clone();
    // the diff shown in the focused view goes, its right view takes the file
    let mut action = action;
    if let Some(session) = editor.diff_session(editor.tree.focus).cloned() {
        if matches!(action, Action::Replace) {
            editor.focus(session.right_view);
            editor.close_diff(session.right_view);
        }
    } else if editor.tree.views().next().is_none() {
        action = Action::VerticalSplit;
    }
    let right = match editor.open(&path, action) {
        Ok(doc) => doc,
        Err(err) => {
            editor.set_error(format!("cannot open {}: {err}", path.display()));
            return;
        }
    };
    editor.open_diff(old, right);
    if let Some(git) = git {
        editor.set_diff_git(editor.tree.focus, git);
    }
    let count = files.len();
    editor.diff_review = Some(ReviewQueue {
        files,
        current: index,
        view: editor.tree.focus,
    });
    let shown = helix_stdx::path::get_relative_path(&path);
    editor.set_status(format!(
        "File {} of {count}: {}",
        index + 1,
        shown.display()
    ));
}

/// Whether git may be run for `dir` (see [`super::git::workspace_trusted`]), saying so when
/// not.
pub fn git_allowed(editor: &mut Editor, dir: &Path) -> bool {
    let trusted = super::git::workspace_trusted(editor, dir);
    if !trusted {
        editor.set_error("Git needs a trusted workspace (see :workspace-trust)");
    }
    trusted
}

/// Moves `count` files forward (or back) in the review of the focused diff.
fn move_in_review(cx: &mut Context, forward: bool) {
    let Some(review) = cx.editor.active_review().cloned() else {
        cx.editor
            .set_error("Not reviewing changed files (open one from Space g or :diff-review)");
        return;
    };
    let count = cx.count();
    let target = if forward {
        review
            .current
            .checked_add(count)
            .filter(|i| *i < review.files.len())
    } else {
        review.current.checked_sub(count)
    };
    let Some(index) = target else {
        cx.editor.set_status(if forward {
            "No more changed files after this one"
        } else {
            "No more changed files before this one"
        });
        return;
    };
    open_review(
        cx.jobs,
        review.files,
        index,
        helix_view::editor::Action::Replace,
    );
}

pub fn diff_next_file(cx: &mut Context) {
    move_in_review(cx, true)
}

pub fn diff_previous_file(cx: &mut Context) {
    move_in_review(cx, false)
}

// --- actions ---------------------------------------------------------------

/// The diff of the focused view, worked out from the documents as they are now:
/// `(the focused view, its side, the hunks)`.
fn focused_diff(editor: &mut Editor) -> Option<(ViewId, Side, std::sync::Arc<[DiffHunk]>)> {
    refresh(editor);
    let view = editor.tree.focus;
    let session = editor.diff_session(view)?;
    Some((view, session.side_of(view)?, session.hunks.clone()))
}

fn not_in_a_diff(cx: &mut Context) {
    cx.editor
        .set_error("Not in a diff (open one with :diff, :diff-git or hx --diff)");
}

/// The cursor's line in `view`, and its row in the window.
fn cursor_line_and_row(editor: &Editor, view: ViewId) -> (usize, usize) {
    let view = editor.tree.get(view);
    let doc = &editor.documents[&view.doc];
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let row = view
        .screen_coords_at_pos(doc, text, cursor)
        .map_or(0, |pos| pos.row);
    (text.char_to_line(cursor), row)
}

/// Moves to the `count`th next (or previous) change of the focused side; the other side
/// follows with `align-on-jump`.
fn jump_to_change(cx: &mut Context, forward: bool) {
    let Some((view, side, hunks)) = focused_diff(cx.editor) else {
        return not_in_a_diff(cx);
    };
    let (line, _) = cursor_line_and_row(cx.editor, view);
    let count = cx.count();
    let starts: Vec<usize> = hunks.iter().map(|hunk| hunk.lines(side).start).collect();
    let mut target = None;
    let mut at = line;
    for _ in 0..count {
        let next = if forward {
            starts.iter().position(|start| *start > at)
        } else {
            starts.iter().rposition(|start| *start < at)
        };
        match next {
            Some(index) => {
                at = starts[index];
                target = Some(index);
            }
            None => break,
        }
    }
    let Some(index) = target else {
        cx.editor.set_status(if forward {
            "No more changes after this one"
        } else {
            "No more changes before this one"
        });
        return;
    };
    let hunk = &hunks[index];
    let doc_id = cx.editor.tree.get(view).doc;
    // keep the row of the cursor, as far as the window allows
    let (_, row) = cursor_line_and_row(cx.editor, view);
    let height = view_height(cx.editor, view);
    let row = row.min(height.saturating_sub(1) / 3);
    cx.editor
        .show_diff_line(view, doc_id, hunk.lines(side).start, row);
    // where the scroll-off leaves it, so that the other side is lined up with that (and the
    // alignment stays)
    cx.editor.ensure_cursor_in_view(view);
    if cx.editor.config().diff.align_on_jump {
        // the same change on the other side, at the same height
        let session = cx.editor.diff_session(view).cloned().expect("in a diff");
        let other = side.other();
        let (_, row) = cursor_line_and_row(cx.editor, view);
        cx.editor.show_diff_line(
            session.view(other),
            session.doc(other),
            hunk.lines(other).start,
            row,
        );
        cx.editor.mark_diff_aligned(view);
    }
}

pub fn diff_next_change(cx: &mut Context) {
    jump_to_change(cx, true)
}

pub fn diff_previous_change(cx: &mut Context) {
    jump_to_change(cx, false)
}

/// Lines the other side up with the cursor's line of this one.
pub fn diff_snap(cx: &mut Context) {
    let Some((view, _, _)) = focused_diff(cx.editor) else {
        return not_in_a_diff(cx);
    };
    cx.editor.align_diff(view);
}

/// Both sides to their first (or last) line.
fn align_end(cx: &mut Context, top: bool) {
    let Some((view, _, _)) = focused_diff(cx.editor) else {
        return not_in_a_diff(cx);
    };
    let session = cx.editor.diff_session(view).cloned().expect("in a diff");
    for side in [Side::Left, Side::Right] {
        let (view, doc) = (session.view(side), session.doc(side));
        let Some(document) = cx.editor.document(doc) else {
            continue;
        };
        let lines = line_count(document.text().slice(..));
        let height = view_height(cx.editor, view);
        if top {
            cx.editor.show_diff_line(view, doc, 0, 0);
        } else {
            let row = height.saturating_sub(1).min(lines.saturating_sub(1));
            cx.editor
                .show_diff_line(view, doc, lines.saturating_sub(1), row);
        }
    }
    cx.editor.mark_diff_aligned(view);
}

pub fn diff_align_top(cx: &mut Context) {
    align_end(cx, true)
}

pub fn diff_align_bottom(cx: &mut Context) {
    align_end(cx, false)
}

/// Replaces the lines of the change under the cursor on the right side (the document) with
/// those of the left side (the old version, which is read-only), from either side.
fn apply_change(cx: &mut Context) {
    let to = Side::Right;
    let Some((view, side, hunks)) = focused_diff(cx.editor) else {
        return not_in_a_diff(cx);
    };
    let (line, _) = cursor_line_and_row(cx.editor, view);
    let (index, inside) = hunk_at(&hunks, line, side);
    // the cursor on a change, or right where the lines of one would be
    let hunk = if inside {
        &hunks[index]
    } else {
        match hunks.get(index) {
            Some(hunk) if hunk.lines(side).is_empty() && hunk.lines(side).start == line => hunk,
            _ => {
                cx.editor.set_status("No change under the cursor");
                return;
            }
        }
    };
    let session = cx.editor.diff_session(view).cloned().expect("in a diff");
    let (from_doc, to_doc) = (session.doc(to.other()), session.doc(to));
    let to_view = session.view(to);
    let from_text = cx.editor.documents[&from_doc].text().clone();
    let doc = cx
        .editor
        .documents
        .get_mut(&to_doc)
        .expect("a diff's document");
    let text = doc.text().slice(..);
    let line_char = |text: helix_core::RopeSlice, line: usize| {
        if line >= text.len_lines() {
            text.len_chars()
        } else {
            text.line_to_char(line)
        }
    };
    let (from_lines, to_lines) = (hunk.lines(to.other()), hunk.lines(to));
    let replacement: Tendril = from_text
        .slice(
            line_char(from_text.slice(..), from_lines.start)
                ..line_char(from_text.slice(..), from_lines.end),
        )
        .to_string()
        .into();
    let mut replacement = replacement;
    let start = line_char(text, to_lines.start);
    let end = line_char(text, to_lines.end);
    // at the end of a text without a final line ending, the lines put there need one first
    if start == text.len_chars()
        && start > 0
        && !replacement.is_empty()
        && !matches!(text.char(start - 1), '\n' | '\r')
    {
        replacement.insert_str(0, doc.line_ending.as_str());
    }
    let transaction =
        Transaction::change(doc.text(), [(start, end, Some(replacement))].into_iter());
    doc.apply(&transaction, to_view);
    let to_view = cx.editor.tree.get_mut(to_view);
    let doc = cx
        .editor
        .documents
        .get_mut(&to_doc)
        .expect("a diff's document");
    doc.append_changes_to_history(to_view);
    refresh(cx.editor);
}

/// The change under the cursor: the left side's lines into the right side.
pub fn diff_apply_left_to_right(cx: &mut Context) {
    apply_change(cx)
}

/// The hunk under the cursor of `view` (on `side`): the one with the cursor's line, or the
/// addition or removal right at it.
fn hunk_under_cursor(
    editor: &Editor,
    view: ViewId,
    side: Side,
    hunks: &[DiffHunk],
) -> Option<usize> {
    let (line, _) = cursor_line_and_row(editor, view);
    let (index, inside) = hunk_at(hunks, line, side);
    if inside {
        return Some(index);
    }
    hunks
        .get(index)
        .filter(|hunk| hunk.lines(side).is_empty() && hunk.lines(side).start == line)
        .map(|_| index)
}

/// Stages the change under the cursor of a diff against git, or unstages it when it is
/// staged (a partly staged one is staged whole): the index takes the change's lines of the
/// document (as shown, saved or not), or back those of `HEAD`.
pub fn diff_stage(cx: &mut Context) {
    let Some((view, side, hunks)) = focused_diff(cx.editor) else {
        return not_in_a_diff(cx);
    };
    let session = cx.editor.diff_session(view).cloned().expect("in a diff");
    let Some(git) = session.git.clone() else {
        cx.editor.set_error(
            "Staging needs a diff against git: a review (Space g, :diff-review) or :diff-git",
        );
        return;
    };
    if !git_allowed(cx.editor, &git.root) {
        return;
    }
    let Some(index) = hunk_under_cursor(cx.editor, view, side, &hunks) else {
        cx.editor.set_status("No change under the cursor");
        return;
    };
    let hunk = &hunks[index];
    let document = cx.editor.documents[&session.right_doc].text().clone();
    let staging = git.stage(index) != Some(Stage::Staged);
    let new_index = if staging {
        // the index takes the document's lines of this change
        helix_core::diff_hunks::splice_hunks(
            GitFile::text(&git.index),
            document.slice(..),
            git.unstaged_of(hunk)
                .map(|unstaged| (unstaged.left.clone(), unstaged.right.clone())),
        )
    } else {
        // the index takes back `HEAD`'s lines of this change
        helix_core::diff_hunks::splice_hunks(
            GitFile::text(&git.index),
            GitFile::text(&git.head),
            git.staged_of(hunk)
                .map(|staged| (staged.right.clone(), staged.left.clone())),
        )
    };
    // the file goes out of the index: a new file unstaged whole, a deleted one staged
    let gone = new_index.is_empty()
        && if staging {
            session
                .right_doc_path(cx.editor)
                .is_some_and(|path| !path.exists())
        } else {
            git.head.is_none()
        };
    let right_view = session.right_view;
    let (root, relative) = (git.root.clone(), git.relative.clone());
    let done = if staging { "Staged" } else { "Unstaged" };
    cx.jobs.callback(async move {
        write_index(&root, &relative, &new_index, gone).await?;
        let new_index = (!gone).then(|| helix_core::Rope::from(new_index));
        let call: crate::job::Callback =
            crate::job::Callback::Editor(Box::new(move |editor: &mut Editor| {
                editor.diff_index_changed(right_view, new_index);
                editor.set_status(format!("{done} the change"));
            }));
        Ok(call)
    });
}

/// The index entry of `relative` (in the repository `root`) set to `content`: a new blob, the
/// file's mode kept (`100644` for a file new to the index); or the entry removed when `gone`.
async fn write_index(root: &Path, relative: &str, content: &str, gone: bool) -> anyhow::Result<()> {
    if gone {
        git(
            root,
            &[
                "update-index".into(),
                "--force-remove".into(),
                "--".into(),
                relative.into(),
            ],
        )
        .await?;
        return Ok(());
    }
    let staged = git(
        root,
        &["ls-files".into(), "-s".into(), "--".into(), relative.into()],
    )
    .await?;
    let staged = String::from_utf8_lossy(&staged);
    let mode = staged
        .split_whitespace()
        .next()
        .filter(|mode| mode.len() == 6)
        .map_or_else(|| file_mode(&root.join(relative)), str::to_string);
    // as `git add` would write it: through the file's filters (line endings...)
    let blob = super::git::git_with_input(
        root,
        &[
            "hash-object".into(),
            "-w".into(),
            "--stdin".into(),
            format!("--path={relative}").into(),
        ],
        content.as_bytes(),
    )
    .await?;
    let blob = String::from_utf8_lossy(&blob).trim().to_string();
    git(
        root,
        &[
            "update-index".into(),
            "--add".into(),
            "--cacheinfo".into(),
            format!("{mode},{blob},{relative}").into(),
        ],
    )
    .await?;
    Ok(())
}

/// The git mode of a file new to the index: executable or not.
fn file_mode(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0) {
            return "100755".to_string();
        }
    }
    let _ = path;
    "100644".to_string()
}

pub fn diff_close(cx: &mut Context) {
    let view = cx.editor.tree.focus;
    if !cx.editor.close_diff(view) {
        not_in_a_diff(cx);
    }
}

/// The rows of text of `view`.
fn view_height(editor: &Editor, view: ViewId) -> usize {
    let view = editor.tree.get(view);
    view.inner_height(&editor.documents[&view.doc])
}

/// The diffs up to date with the documents: worked out again where they only followed edits.
fn refresh(editor: &mut Editor) {
    editor.refresh_diff_sessions();
    editor.recompute_stale_diffs();
}

// --- keys ------------------------------------------------------------------

/// What keys typed in a diff view are to its keymap.
pub enum KeyMatch {
    /// The keys of an action.
    Action(fn(&mut Context)),
    /// The start of the keys of an action: wait for more.
    Prefix,
    /// Not keys of the diff view.
    None,
}

/// The action of `action` (a name of `[editor.diff.keys]`).
fn action(name: &str) -> Option<fn(&mut Context)> {
    Some(match name {
        "next-change" => diff_next_change,
        "previous-change" => diff_previous_change,
        "next-file" => diff_next_file,
        "previous-file" => diff_previous_file,
        "snap" => diff_snap,
        "align-top" => diff_align_top,
        "align-bottom" => diff_align_bottom,
        "apply-left-to-right" => diff_apply_left_to_right,
        "stage" => diff_stage,
        "close" => diff_close,
        _ => return None,
    })
}

/// What `keys` are to the keymap of the diff views of `config`. The file keys (`]f`, `[f`)
/// only in a review: elsewhere they keep their usual meaning (the next function...).
pub fn match_keys(config: &DiffConfig, keys: &[KeyEvent], reviewing: bool) -> KeyMatch {
    let mut prefix = false;
    for (name, sequence) in &config.keys {
        if !reviewing && matches!(name.as_str(), "next-file" | "previous-file") {
            continue;
        }
        let Some(action) = action(name) else {
            continue;
        };
        let Ok(sequence) = helix_view::input::parse_macro(sequence) else {
            continue;
        };
        if sequence.is_empty() {
            continue;
        }
        if sequence == keys {
            return KeyMatch::Action(action);
        }
        if sequence.len() > keys.len() && sequence.starts_with(keys) {
            prefix = true;
        }
    }
    if prefix {
        KeyMatch::Prefix
    } else {
        KeyMatch::None
    }
}
