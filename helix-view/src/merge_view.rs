//! The merge conflict resolver: a file git could not merge in three windows, your version on
//! the left (git's `:2:`), the result in the middle (the file itself, with git's conflict
//! markers, edited as usual) and their version on the right (`:3:`), scrolled together. The
//! conflicts are resolved by taking one side, the other or both, or by editing the result;
//! saving a result without conflicts left marks the file resolved (`git add`).

use std::sync::Arc;

use helix_core::diff_hunks::{diff_hunks, map_line, DiffHunk, DiffOptions, Side};
use helix_core::merge_conflicts::{find_conflicts, Conflict};

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
    pub conflicts: Vec<Conflict>,
    /// The changes from your version to the result, and from theirs: what lines up the
    /// windows.
    pub local_hunks: Arc<[DiffHunk]>,
    pub remote_hunks: Arc<[DiffHunk]>,
    /// The result's version the conflicts are of.
    version: Option<i32>,
    /// The result was typed into since (see [`Editor::refresh_merge`]).
    stale: bool,
    /// Where the three windows were scrolled to when last in step.
    scrolled: Option<[Scroll; 3]>,
}

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

    /// The line of the result matching `line` of `pane`.
    pub fn to_result(&self, pane: Pane, line: usize) -> usize {
        match pane {
            Pane::Local => map_line(&self.local_hunks, line, Side::Right),
            Pane::Remote => map_line(&self.remote_hunks, line, Side::Right),
            Pane::Result => line,
        }
    }

    /// The line of `pane` matching `line` of the result.
    pub fn from_result(&self, pane: Pane, line: usize) -> usize {
        match pane {
            Pane::Local => map_line(&self.local_hunks, line, Side::Left),
            Pane::Remote => map_line(&self.remote_hunks, line, Side::Left),
            Pane::Result => line,
        }
    }
}

const PANES: [Pane; 3] = [Pane::Local, Pane::Result, Pane::Remote];

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
            conflicts: Vec::new(),
            local_hunks: Arc::from([]),
            remote_hunks: Arc::from([]),
            version: None,
            stale: false,
            scrolled: None,
        });
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
        let typing = self.mode == crate::document::Mode::Insert;
        let changed = merge.version != Some(version);
        if changed && typing {
            if let Some(merge) = self.merge.as_mut() {
                merge.stale = true;
                merge.version = Some(version);
            }
        } else if changed || (merge.stale && !typing) {
            self.recompute_merge();
        }
        self.sync_merge_scrolling();
    }

    /// The conflicts and the alignment of the windows worked out from the documents.
    pub(crate) fn recompute_merge(&mut self) {
        let Some(merge) = &self.merge else {
            return;
        };
        let text = |doc: DocumentId| self.document(doc).map(|doc| doc.text().clone());
        let (Some(local), Some(result), Some(remote)) = (
            text(merge.local_doc),
            text(merge.result_doc),
            text(merge.remote_doc),
        ) else {
            return;
        };
        let version = self.document(merge.result_doc).map(|doc| doc.version());
        let options = DiffOptions::default();
        let merge = self.merge.as_mut().expect("a merge");
        merge.conflicts = find_conflicts(result.slice(..));
        merge.local_hunks = diff_hunks(local.slice(..), result.slice(..), options).into();
        merge.remote_hunks = diff_hunks(remote.slice(..), result.slice(..), options).into();
        merge.version = version;
        merge.stale = false;
    }

    /// The windows of the merge scrolled together: one that scrolled since (the focused one
    /// leads when several did) takes the others along, through the result.
    fn sync_merge_scrolling(&mut self) {
        let Some(merge) = self.merge.clone() else {
            return;
        };
        let scrolls: Option<Vec<Scroll>> = PANES
            .iter()
            .map(|pane| self.diff_scroll(merge.view(*pane), merge.doc(*pane)))
            .collect();
        let Some(now) = scrolls.and_then(|scrolls| <[Scroll; 3]>::try_from(scrolls).ok()) else {
            return;
        };
        let Some(before) = merge.scrolled else {
            if let Some(merge) = self.merge.as_mut() {
                merge.scrolled = Some(now);
            }
            return;
        };
        let moved: Vec<Pane> = PANES
            .iter()
            .zip(now.iter().zip(before.iter()))
            .filter(|(_, (now, before))| now != before)
            .map(|(pane, _)| *pane)
            .collect();
        let leader = match moved.as_slice() {
            [] => return,
            [pane] => *pane,
            _ => merge.pane_of(self.tree.focus).unwrap_or(Pane::Result),
        };
        if let Some((line, row)) = self.scroll_reference(merge.view(leader), merge.doc(leader)) {
            let result_line = merge.to_result(leader, line);
            for pane in PANES.into_iter().filter(|pane| *pane != leader) {
                let other = merge.from_result(pane, result_line);
                self.scroll_diff_side(merge.view(pane), merge.doc(pane), other, row);
            }
        }
        let scrolls: Option<Vec<Scroll>> = PANES
            .iter()
            .map(|pane| self.diff_scroll(merge.view(*pane), merge.doc(*pane)))
            .collect();
        if let Some(merge) = self.merge.as_mut() {
            merge.scrolled = scrolls.and_then(|scrolls| <[Scroll; 3]>::try_from(scrolls).ok());
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
