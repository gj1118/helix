//! The merge conflict resolver: a file git could not merge in three windows, your version on
//! the left (git's `:2:`), the result in the middle (the file itself, with git's conflict
//! markers, edited as usual) and their version on the right (`:3:`), scrolled together. The
//! conflicts are resolved by taking one side, the other or both, or by editing the result;
//! saving a result without conflicts left marks the file resolved (`git add`).

use std::sync::Arc;
use std::time::Duration;

use helix_core::diff_hunks::{diff_hunks, map_line, DiffHunk, DiffOptions, Side};
use helix_core::merge_conflicts::{find_conflicts, Conflict};

use crate::background::Background;
use crate::diff_view::{OldVersion, Scroll};
use crate::{DocumentId, Editor, ViewId};

/// The three windows of a merge (see the module).
#[derive(Debug, Clone)]
pub struct MergeSession {
    pub local_view: ViewId,
    pub result_view: ViewId,
    pub remote_view: ViewId,
    pub local_doc: DocumentId,
    pub result_doc: DocumentId,
    pub remote_doc: DocumentId,
    /// The repository, and the file's path in it (`/`-separated).
    pub root: std::path::PathBuf,
    pub relative: String,
    /// The conflicts left in the result.
    pub conflicts: Arc<[Conflict]>,
    /// The changes from your version to the result, and from theirs: what lines up the
    /// windows.
    pub local_hunks: Arc<[DiffHunk]>,
    pub remote_hunks: Arc<[DiffHunk]>,
    /// The result's version the conflicts are of.
    version: Option<i32>,
    /// The result was typed into since (see [`Editor::refresh_merge`]).
    stale: bool,
    /// The merge being worked out on a thread of its own (a long file), of the result at a
    /// version.
    pending: Option<(i32, Background<WorkedOut>)>,
    /// Where the three windows were scrolled to when last in step.
    scrolled: Option<[Scroll; 3]>,
}

/// What working out a merge found.
struct WorkedOut {
    conflicts: Vec<Conflict>,
    local_hunks: Vec<DiffHunk>,
    remote_hunks: Vec<DiffHunk>,
}

/// Merges of up to this many lines in all (the three versions) are worked out between two
/// frames; longer ones on a thread of their own (see [`Background`]).
const INLINE_MAX_LINES: usize = 20_000;
/// How long a frame waits for a merge started on its own thread.
const BACKGROUND_WAIT: Duration = Duration::from_millis(20);

/// One of the windows of a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Local,
    Result,
    Remote,
}

impl MergeSession {
    /// The window `view` is, if it is one of the merge's.
    pub fn pane_of(&self, view: ViewId) -> Option<Pane> {
        if view == self.local_view {
            Some(Pane::Local)
        } else if view == self.result_view {
            Some(Pane::Result)
        } else if view == self.remote_view {
            Some(Pane::Remote)
        } else {
            None
        }
    }

    pub fn view(&self, pane: Pane) -> ViewId {
        match pane {
            Pane::Local => self.local_view,
            Pane::Result => self.result_view,
            Pane::Remote => self.remote_view,
        }
    }

    pub fn doc(&self, pane: Pane) -> DocumentId {
        match pane {
            Pane::Local => self.local_doc,
            Pane::Result => self.result_doc,
            Pane::Remote => self.remote_doc,
        }
    }

    /// Whether the result was typed into since the conflicts were worked out.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Whether it is being worked out on a thread of its own.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The line of the result matching `line` of `pane`.
    pub fn to_result(&self, pane: Pane, line: usize) -> usize {
        pane_line(
            [&self.local_hunks, &self.remote_hunks],
            pane,
            line,
            Side::Right,
        )
    }

    /// The line of `pane` matching `line` of the result.
    pub fn from_result(&self, pane: Pane, line: usize) -> usize {
        pane_line(
            [&self.local_hunks, &self.remote_hunks],
            pane,
            line,
            Side::Left,
        )
    }
}

const PANES: [Pane; 3] = [Pane::Local, Pane::Result, Pane::Remote];

/// `line` mapped through the hunks of `pane` (`[yours, theirs]` against the result): to the
/// result (`Side::Right`), or from it to the pane (`Side::Left`).
fn pane_line(hunks: [&[DiffHunk]; 2], pane: Pane, line: usize, to: Side) -> usize {
    match pane {
        Pane::Local => map_line(hunks[0], line, to),
        Pane::Remote => map_line(hunks[1], line, to),
        Pane::Result => line,
    }
}

impl MergeSession {
    fn set(&mut self, worked_out: WorkedOut) {
        self.conflicts = worked_out.conflicts.into();
        self.local_hunks = worked_out.local_hunks.into();
        self.remote_hunks = worked_out.remote_hunks.into();
    }
}

/// The conflicts of the result, and the changes from yours and from theirs to it, of `texts`
/// (yours, the result, theirs).
fn work_out_merge([local, result, remote]: &[helix_core::Rope; 3]) -> WorkedOut {
    let options = DiffOptions::default();
    WorkedOut {
        conflicts: find_conflicts(result.slice(..)),
        local_hunks: diff_hunks(local.slice(..), result.slice(..), options),
        remote_hunks: diff_hunks(remote.slice(..), result.slice(..), options),
    }
}

impl Editor {
    /// The merge `view` is a window of, if any.
    pub fn merge_of(&self, view: ViewId) -> Option<&MergeSession> {
        self.merge
            .as_ref()
            .filter(|merge| merge.pane_of(view).is_some())
    }

    /// Shows `result` (a file with conflicts, of the repository `root` at `relative`) as a
    /// merge: in the focused window, your version (`local`) in a new window on its left, theirs
    /// (`remote`) on its right. A diff or merge shown in the focused window goes first.
    pub fn open_merge(
        &mut self,
        local: OldVersion,
        remote: OldVersion,
        result: DocumentId,
        root: std::path::PathBuf,
        relative: String,
    ) {
        use crate::{editor::Action, tree::Direction};
        self.close_merge();
        let focused = self.tree.focus;
        self.close_diff(focused);
        self.switch(result, Action::Replace);
        let result_view = self.tree.focus;
        let local_doc = self.read_only_version(local, "Your version is read-only");
        let local_doc = self.new_file_from_document(Action::VerticalSplit, local_doc);
        let local_view = self.tree.focus;
        self.swap_split_in_direction(Direction::Left);
        self.focus(result_view);
        let remote_doc = self.read_only_version(remote, "Their version is read-only");
        let remote_doc = self.new_file_from_document(Action::VerticalSplit, remote_doc);
        let remote_view = self.tree.focus;
        for doc in [local_doc, remote_doc] {
            if let Some(doc) = self.documents.get_mut(&doc) {
                doc.reset_modified();
            }
        }
        self.focus(result_view);
        self.merge = Some(MergeSession {
            local_view,
            result_view,
            remote_view,
            local_doc,
            result_doc: result,
            remote_doc,
            root,
            relative,
            conflicts: Arc::from([]),
            local_hunks: Arc::from([]),
            remote_hunks: Arc::from([]),
            version: None,
            stale: false,
            pending: None,
            scrolled: None,
        });
        // worked out now whatever its length (opening it is waited for): its first conflict
        // is shown
        self.recompute_merge();
        self.refresh_merge();
        // the first conflict in view
        let first = self
            .merge
            .as_ref()
            .and_then(|merge| merge.conflicts.first())
            .map(|conflict| conflict.lines.start);
        if let Some(line) = first {
            self.show_merge_line(line, 3);
        }
    }

    /// A read-only document of `version` (its text, title and highlighting), `why` said when
    /// an edit is refused.
    fn read_only_version(&mut self, version: OldVersion, why: &str) -> crate::Document {
        let mut doc = crate::Document::from(
            version.text,
            None,
            Arc::clone(&self.config),
            Arc::clone(&self.syn_loader),
        );
        doc.title = Some(version.title);
        doc.readonly = true;
        doc.locked = Some(why.to_string());
        if let Some(path) = &version.syntax_path {
            let loader = self.syn_loader.load();
            if let Some(language) = loader.language_for_filename(path) {
                doc.set_language(Some(loader.language(language).config().clone()), &loader);
            }
        }
        doc
    }

    /// Shows `line` of the result at `row` of its window, with the cursor; the other windows
    /// line up with it.
    pub fn show_merge_line(&mut self, line: usize, row: usize) {
        let Some(merge) = self.merge.clone() else {
            return;
        };
        self.show_diff_line(merge.result_view, merge.result_doc, line, row);
        for pane in [Pane::Local, Pane::Remote] {
            let other = merge.from_result(pane, line);
            self.show_diff_line(merge.view(pane), merge.doc(pane), other, row);
        }
        if let Some(merge) = self.merge.as_mut() {
            merge.scrolled = None;
        }
    }

    /// Brings the merge up to date with the result, and ends it when one of its windows or
    /// documents is gone. While the result is typed into, it is worked out again on a pause
    /// (see [`Editor::recompute_stale_diffs`]); then the windows scroll together.
    pub fn refresh_merge(&mut self) {
        let Some(merge) = &self.merge else {
            return;
        };
        let shown = PANES.iter().all(|pane| {
            self.tree.contains(merge.view(*pane))
                && self.tree.get(merge.view(*pane)).doc == merge.doc(*pane)
        });
        if !shown {
            self.close_merge();
            return;
        }
        let Some(version) = self.document(merge.result_doc).map(|doc| doc.version()) else {
            self.close_merge();
            return;
        };
        self.poll_merge(Duration::ZERO);
        let Some(merge) = &self.merge else {
            return;
        };
        let typing = self.mode == crate::document::Mode::Insert;
        let changed = merge.version != Some(version);
        if changed && typing {
            if let Some(merge) = self.merge.as_mut() {
                merge.stale = true;
                merge.version = Some(version);
            }
        } else if changed || (merge.stale && !typing) {
            self.request_merge();
        }
        self.sync_merge_scrolling();
    }

    /// The texts of the merge's documents: yours, the result and theirs.
    fn merge_texts(&self) -> Option<[helix_core::Rope; 3]> {
        let merge = self.merge.as_ref()?;
        let text = |doc: DocumentId| self.document(doc).map(|doc| doc.text().clone());
        Some([
            text(merge.local_doc)?,
            text(merge.result_doc)?,
            text(merge.remote_doc)?,
        ])
    }

    /// The merge worked out again: now when short, else on a thread of its own (one at a
    /// time), picked up by [`Editor::refresh_merge`] once done.
    pub(crate) fn request_merge(&mut self) {
        let Some(texts) = self.merge_texts() else {
            return;
        };
        if texts.iter().map(|text| text.len_lines()).sum::<usize>() <= INLINE_MAX_LINES {
            self.recompute_merge();
            return;
        }
        let merge = self.merge.as_ref().expect("a merge");
        if merge.pending.is_some() {
            return;
        }
        let Some(version) = self.document(merge.result_doc).map(|doc| doc.version()) else {
            return;
        };
        let work = Background::spawn(move || work_out_merge(&texts));
        if let Some(merge) = self.merge.as_mut() {
            merge.pending = Some((version, work));
        }
        self.poll_merge(BACKGROUND_WAIT);
    }

    /// Takes the merge worked out on its own thread, if done (waiting up to `wait`): shown
    /// when the result didn't change since it started, else asked again when due.
    fn poll_merge(&mut self, wait: Duration) {
        let Some(merge) = &self.merge else {
            return;
        };
        let Some((version, outcome)) = merge
            .pending
            .as_ref()
            .and_then(|(version, work)| Some((*version, work.take(wait)?)))
        else {
            return;
        };
        let current = self.document(merge.result_doc).map(|doc| doc.version());
        let merge = self.merge.as_mut().expect("a merge");
        merge.pending = None;
        match outcome {
            Ok(worked_out) if current == Some(version) => {
                merge.set(worked_out);
                merge.version = Some(version);
                merge.stale = false;
            }
            Ok(_) => merge.stale = true,
            Err(_) => log::error!("working out the merge failed"),
        }
    }

    /// The conflicts and the alignment of the windows worked out now from the documents.
    pub(crate) fn recompute_merge(&mut self) {
        let Some(texts) = self.merge_texts() else {
            return;
        };
        let version = self
            .merge
            .as_ref()
            .and_then(|merge| self.document(merge.result_doc))
            .map(|doc| doc.version());
        let merge = self.merge.as_mut().expect("a merge");
        merge.set(work_out_merge(&texts));
        merge.pending = None;
        merge.version = version;
        merge.stale = false;
    }

    /// The windows of the merge scrolled together: one that scrolled since (the focused one
    /// leads when several did) takes the others along, through the result.
    fn sync_merge_scrolling(&mut self) {
        let Some(merge) = &self.merge else {
            return;
        };
        let scrolls = |editor: &Self, merge: &MergeSession| -> Option<[Scroll; 3]> {
            let [a, b, c] = PANES.map(|pane| editor.diff_scroll(merge.view(pane), merge.doc(pane)));
            Some([a?, b?, c?])
        };
        let Some(now) = scrolls(self, merge) else {
            return;
        };
        let Some(before) = merge.scrolled else {
            if let Some(merge) = self.merge.as_mut() {
                merge.scrolled = Some(now);
            }
            return;
        };
        let mut moved = PANES
            .iter()
            .zip(now.iter().zip(before.iter()))
            .filter(|(_, (now, before))| now != before)
            .map(|(pane, _)| *pane);
        let leader = match (moved.next(), moved.next()) {
            (None, _) => return,
            (Some(pane), None) => pane,
            _ => merge.pane_of(self.tree.focus).unwrap_or(Pane::Result),
        };
        // what following needs (the views, documents and hunks), not the whole merge
        let (views, docs) = (
            PANES.map(|pane| merge.view(pane)),
            PANES.map(|pane| merge.doc(pane)),
        );
        let (local, remote) = (
            Arc::clone(&merge.local_hunks),
            Arc::clone(&merge.remote_hunks),
        );
        let at = |pane: Pane| PANES.iter().position(|p| *p == pane).expect("a pane");
        if let Some((line, row)) = self.scroll_reference(views[at(leader)], docs[at(leader)]) {
            let result_line = pane_line([&local, &remote], leader, line, Side::Right);
            for pane in PANES.into_iter().filter(|pane| *pane != leader) {
                let other = pane_line([&local, &remote], pane, result_line, Side::Left);
                self.scroll_diff_side(views[at(pane)], docs[at(pane)], other, row);
            }
        }
        let scrolled = self.merge.as_ref().and_then(|merge| scrolls(self, merge));
        if let Some(merge) = self.merge.as_mut() {
            merge.scrolled = scrolled;
        }
    }

    /// Ends the merge: your and their versions' windows and documents go, the result stays.
    pub fn close_merge(&mut self) -> bool {
        let Some(merge) = self.merge.take() else {
            return false;
        };
        for (view, doc) in [
            (merge.local_view, merge.local_doc),
            (merge.remote_view, merge.remote_doc),
        ] {
            if self.tree.contains(view) && self.tree.get(view).doc == doc {
                self.close(view);
            }
            if self.documents.contains_key(&doc)
                && !self.tree.views().any(|(view, _)| view.doc == doc)
            {
                if self.tree.views().next().is_some() {
                    let _ = self.close_document(doc, true);
                } else {
                    self.documents.remove(&doc);
                }
            }
        }
        true
    }
}
