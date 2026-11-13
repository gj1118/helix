//! The merge conflict resolver (see [`helix_view::merge_view`]): opening it (`:diff-merge`, a
//! conflicted file from the changed files list or a review), its actions (the next conflict,
//! taking your side, theirs or both) and its keys (`[editor.diff.merge-keys]`); saving the
//! result marks the file resolved.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use helix_core::merge_conflicts::{conflict_at, find_conflicts, resolve, Resolution};
use helix_core::Transaction;
use helix_view::diff_view::{DiffConfig, OldVersion};
use helix_view::input::KeyEvent;
use helix_view::{DocumentId, Editor};

use super::diff::{
    cursor_line_and_row, git_allowed, match_table, repo_path, show_file, view_height, KeyMatch,
};
use super::git::git;
use super::Context;

// --- opening -----------------------------------------------------------------

/// Opens the merge of `path`, a file with conflicts, in the focused window: once its two sides
/// are read from the index (git's stages 2 and 3), in the background.
pub fn open_merge(editor: &mut Editor, jobs: &mut crate::job::Jobs, path: PathBuf) {
    let Some(dir) = path.parent() else {
        return;
    };
    if !git_allowed(editor, dir, "Resolving merge conflicts") {
        return;
    }
    jobs.callback(merge_of(path, None));
}

/// What [`open_merge`] does in the background, then (`review`) makes it file `index` of a
/// review of the changed files.
pub async fn merge_of(
    path: PathBuf,
    review: Option<(Vec<helix_view::diff_view::ReviewFile>, usize)>,
) -> anyhow::Result<crate::job::Callback> {
    let (root, relative) = repo_path(&path).await?;
    let ours = show_file(&root, &relative, Some(":2")).await?;
    let theirs = show_file(&root, &relative, Some(":3")).await?;
    if ours.is_none() && theirs.is_none() {
        return Err(anyhow!(
            "{relative} has no conflict to resolve (nothing at stages 2 and 3 of the index)"
        ));
    }
    let version = |text: Option<String>, side: &str| OldVersion {
        text: text.unwrap_or_default().into(),
        title: format!("{side}:{relative}"),
        syntax_path: Some(PathBuf::from(&relative)),
    };
    let (local, remote) = (
        version(ours, "local (ours)"),
        version(theirs, "remote (theirs)"),
    );
    Ok(crate::job::Callback::Editor(Box::new(
        move |editor: &mut Editor| {
            show_merge(editor, path, local, remote, root, relative, review);
        },
    )))
}

fn show_merge(
    editor: &mut Editor,
    path: PathBuf,
    local: OldVersion,
    remote: OldVersion,
    root: PathBuf,
    relative: String,
    review: Option<(Vec<helix_view::diff_view::ReviewFile>, usize)>,
) {
    use helix_view::editor::Action;
    // the diff or merge of the focused window goes, its result window taking the file
    if let Some(merge) = editor.merge_of(editor.tree.focus).cloned() {
        editor.focus(merge.result_view);
        editor.close_merge();
    } else if let Some(session) = editor.diff_session(editor.tree.focus).cloned() {
        editor.focus(session.right_view);
        editor.close_diff(session.right_view);
    }
    let action = if editor.tree.views().next().is_some() {
        Action::Replace
    } else {
        Action::VerticalSplit
    };
    let result = match editor.open(&path, action) {
        Ok(doc) => doc,
        Err(err) => {
            editor.set_error(format!("cannot open {}: {err}", path.display()));
            return;
        }
    };
    editor.open_merge(local, remote, result, root, relative);
    let conflicts = editor
        .merge
        .as_ref()
        .map_or(0, |merge| merge.conflicts.len());
    if let Some((files, index)) = review {
        let count = files.len();
        editor.diff_review = Some(helix_view::diff_view::ReviewQueue {
            files,
            current: index,
            view: editor.tree.focus,
        });
        editor.set_status(format!(
            "File {} of {count}: {} conflicts in {}",
            index + 1,
            conflicts,
            helix_stdx::path::get_relative_path(&path).display()
        ));
    } else {
        editor.set_status(format!(
            "{conflicts} conflicts: >> yours, << theirs, gb both; :w when done"
        ));
    }
}

// --- actions -----------------------------------------------------------------

fn not_in_a_merge(cx: &mut Context) {
    cx.editor
        .set_error("Not in a merge (open one with :diff-merge on a file with conflicts)");
}

/// The merge up to date with its result, and the line of the result of the cursor of the
/// focused window, with its row.
fn focused_merge(cx: &mut Context) -> Option<(helix_view::merge_view::MergeSession, usize, usize)> {
    // exact, whatever its length: the actions take sides of the conflicts by it
    cx.editor.settle_diffs();
    let view = cx.editor.tree.focus;
    let merge = cx.editor.merge_of(view)?.clone();
    let pane = merge.pane_of(view)?;
    let (line, row) = cursor_line_and_row(cx.editor, view);
    Some((merge.clone(), merge.to_result(pane, line), row))
}

/// The `count`th next (or previous) conflict, all the windows lined up with it.
fn jump_to_conflict(cx: &mut Context, forward: bool) {
    let Some((merge, line, row)) = focused_merge(cx) else {
        return not_in_a_merge(cx);
    };
    if merge.conflicts.is_empty() {
        cx.editor
            .set_status("No conflicts left: :w marks the file resolved");
        return;
    }
    let starts: Vec<usize> = merge.conflicts.iter().map(|c| c.lines.start).collect();
    let mut at = line;
    let mut target = None;
    for _ in 0..cx.count() {
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
            "No more conflicts after this one"
        } else {
            "No more conflicts before this one"
        });
        return;
    };
    let height = view_height(cx.editor, merge.result_view);
    let row = row.min(height.saturating_sub(1) / 3).max(1.min(height));
    cx.editor.show_merge_line(starts[index], row);
    cx.editor.ensure_cursor_in_view(cx.editor.tree.focus);
}

pub fn merge_next_conflict(cx: &mut Context) {
    jump_to_conflict(cx, true)
}

pub fn merge_previous_conflict(cx: &mut Context) {
    jump_to_conflict(cx, false)
}

/// The conflict under the cursor resolved with `resolution`: its block in the result replaced.
fn take(cx: &mut Context, resolution: Resolution) {
    let Some((merge, line, _)) = focused_merge(cx) else {
        return not_in_a_merge(cx);
    };
    let Some(conflict) = conflict_at(&merge.conflicts, line).cloned() else {
        cx.editor
            .set_status("No conflict under the cursor (]c goes to the next one)");
        return;
    };
    let doc = cx
        .editor
        .documents
        .get_mut(&merge.result_doc)
        .expect("the merge's result");
    let (range, replacement) = resolve(doc.text().slice(..), &conflict, resolution);
    let transaction = Transaction::change(
        doc.text(),
        [(range.start, range.end, Some(replacement.into()))].into_iter(),
    );
    doc.apply(&transaction, merge.result_view);
    let view = cx.editor.tree.get_mut(merge.result_view);
    let doc = cx
        .editor
        .documents
        .get_mut(&merge.result_doc)
        .expect("the merge's result");
    doc.append_changes_to_history(view);
    let left = find_conflicts(doc.text().slice(..)).len();
    cx.editor.settle_diffs();
    let taken = match resolution {
        Resolution::Ours => "yours",
        Resolution::Theirs => "theirs",
        Resolution::Both => "both",
    };
    cx.editor.set_status(match left {
        0 => format!("Took {taken}: no conflicts left, :w marks the file resolved"),
        1 => format!("Took {taken}: 1 conflict left"),
        left => format!("Took {taken}: {left} conflicts left"),
    });
}

pub fn merge_take_local(cx: &mut Context) {
    take(cx, Resolution::Ours)
}

pub fn merge_take_remote(cx: &mut Context) {
    take(cx, Resolution::Theirs)
}

pub fn merge_take_both(cx: &mut Context) {
    take(cx, Resolution::Both)
}

/// The other windows lined up with the cursor's line of this one.
pub fn merge_snap(cx: &mut Context) {
    let Some((_, line, row)) = focused_merge(cx) else {
        return not_in_a_merge(cx);
    };
    cx.editor.show_merge_line(line, row);
}

pub fn merge_close(cx: &mut Context) {
    let view = cx.editor.tree.focus;
    if let Some(result) = cx.editor.merge_of(view).map(|merge| merge.result_view) {
        cx.editor.focus(result);
        cx.editor.close_merge();
    } else {
        not_in_a_merge(cx);
    }
}

// --- saving --------------------------------------------------------------------

/// After `doc` was saved: when it is the result of the merge and has no conflicts left, the
/// file marked resolved (`git add`, in the background); else, how many are left.
pub fn merge_on_save(editor: &mut Editor, jobs: &mut crate::job::Jobs, doc: DocumentId) {
    let Some(merge) = editor.merge.clone().filter(|merge| merge.result_doc == doc) else {
        return;
    };
    let Some(text) = editor.document(doc).map(|doc| doc.text().clone()) else {
        return;
    };
    let left = find_conflicts(text.slice(..)).len();
    if left > 0 {
        editor.set_status(format!(
            "Saved with {left} conflict{} left: not marked resolved yet",
            if left == 1 { "" } else { "s" }
        ));
        return;
    }
    if !git_allowed(editor, &merge.root, "Marking the file resolved") {
        return;
    }
    let (root, relative) = (merge.root.clone(), merge.relative.clone());
    let job = crate::job::Job::with_callback(async move {
        git(&root, &["add".into(), "--".into(), relative.clone().into()]).await?;
        let call: crate::job::Callback =
            crate::job::Callback::Editor(Box::new(move |editor: &mut Editor| {
                editor.set_status(format!("Resolved {relative} (git add)"));
            }));
        Ok(call)
    })
    .wait_before_exiting();
    jobs.add(job);
}

// --- keys --------------------------------------------------------------------

/// The action of `name` (of `[editor.diff.merge-keys]`).
fn action(name: &str) -> Option<fn(&mut Context)> {
    Some(match name {
        "next-conflict" => merge_next_conflict,
        "previous-conflict" => merge_previous_conflict,
        "take-local" => merge_take_local,
        "take-remote" => merge_take_remote,
        "take-both" => merge_take_both,
        "snap" => merge_snap,
        "close" => merge_close,
        _ => return None,
    })
}

/// What `keys` are to the keymap of the merge views: the merge keys, and in a review of the
/// changed files the file keys of the diff views (`]f`, `[f`).
pub fn match_keys(config: &DiffConfig, keys: &[KeyEvent], reviewing: bool) -> KeyMatch {
    let merge = match_table(&config.merge_keys, keys, action, |_| false);
    if let KeyMatch::Action(_) = merge {
        return merge;
    }
    let files = if reviewing {
        match_table(&config.keys, keys, super::diff::action, |name| {
            !matches!(name, "next-file" | "previous-file")
        })
    } else {
        KeyMatch::None
    };
    match (merge, files) {
        (_, KeyMatch::Action(action)) => KeyMatch::Action(action),
        (KeyMatch::Prefix, _) | (_, KeyMatch::Prefix) => KeyMatch::Prefix,
        _ => KeyMatch::None,
    }
}

/// The path of the current file, for `:diff-merge`.
pub fn current_path(editor: &Editor) -> anyhow::Result<PathBuf> {
    let (_, doc) = helix_view::current_ref!(editor);
    doc.path()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("the buffer has no file to merge"))
}
