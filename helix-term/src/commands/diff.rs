//! Side-by-side diffs: the commands that open them (`:diff`, `:diff-buffers`, `:diff-git`,
//! `hx --diff`), the actions inside them (next change, snap, apply a change from one side to the
//! other...) and the keys of the diff views (`[editor.diff.keys]`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _};
use helix_core::diff_hunks::{hunk_at, line_count, DiffHunk, Side};
use helix_core::{Tendril, Transaction};
use helix_view::diff_view::{DiffConfig, OldVersion};
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
            text,
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
        text: doc.text().to_string(),
        title: format!("{} (snapshot)", doc.display_name()),
        syntax_path: doc.path().map(Path::to_path_buf),
    };
    editor.open_diff(old, right);
    Ok(())
}

/// The version of `path` in git: in the index (`rev` `None`) or at `rev`; empty when the file
/// is not there (a new file). With the title of its buffer.
pub async fn git_version(path: &Path, rev: Option<&str>) -> anyhow::Result<OldVersion> {
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
    let relative = real(path)
        .strip_prefix(&root)
        .map(Path::to_path_buf)
        .map_err(|_| {
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
    let spec = format!("{}:{relative}", rev.unwrap_or(""));
    let label = match rev {
        Some(rev) => format!("{rev}:{relative}"),
        None => format!("index:{relative}"),
    };
    let text = match git(&root, &[OsString::from("show"), OsString::from(&spec)]).await {
        Ok(out) => String::from_utf8_lossy(&out).into_owned(),
        // not in that version: everything is new
        Err(err)
            if err.to_string().contains("does not exist")
                || err.to_string().contains("exists on disk, but not in") =>
        {
            String::new()
        }
        Err(err) => return Err(err),
    };
    Ok(OldVersion {
        text,
        title: label,
        syntax_path: Some(PathBuf::from(&relative)),
    })
}

// --- actions ---------------------------------------------------------------

/// The diff of the focused view: `(the focused side, its hunks)`.
fn focused_diff(editor: &Editor) -> Option<(ViewId, Side, Vec<DiffHunk>)> {
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
    refresh(cx.editor);
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
    refresh(cx.editor);
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

/// Replaces the lines of the change under the cursor on the `to` side with those of the other
/// side.
fn apply_change(cx: &mut Context, to: Side) {
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
    apply_change(cx, Side::Right)
}

/// The change under the cursor: the right side's lines into the left side.
pub fn diff_apply_right_to_left(cx: &mut Context) {
    apply_change(cx, Side::Left)
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

fn refresh(editor: &mut Editor) {
    editor.refresh_diff_sessions();
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
        "snap" => diff_snap,
        "align-top" => diff_align_top,
        "align-bottom" => diff_align_bottom,
        "apply-left-to-right" => diff_apply_left_to_right,
        "apply-right-to-left" => diff_apply_right_to_left,
        "close" => diff_close,
        _ => return None,
    })
}

/// What `keys` are to the keymap of the diff views of `config`.
pub fn match_keys(config: &DiffConfig, keys: &[KeyEvent]) -> KeyMatch {
    let mut prefix = false;
    for (name, sequence) in &config.keys {
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
