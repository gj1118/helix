//! Side-by-side diffs: two views next to each other, the old version on the left (a read-only
//! snapshot) and the document on the right (editable as usual), the changed lines and words
//! highlighted on both sides and a connector between them that links each change on the left
//! to the same change on the right. Each side keeps its own lines: nothing is inserted to line
//! them up; the connector bends instead.

use std::collections::HashMap;

use std::ops::Range;
use std::sync::Arc;

use helix_core::diff_hunks::{
    diff_hunks, hunk_at, hunk_words, DiffHunk, DiffOptions, HunkKind, HunkWords, Side,
};
use serde::{Deserialize, Serialize};

use crate::{
    editor::{GutterConfig, GutterType},
    graphics::{Color, Modifier, Style},
    DocumentId, Editor, Theme, ViewId,
};

/// `[editor.diff]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct DiffConfig {
    /// Lines that differ only in whitespace are not changes.
    pub ignore_whitespace: bool,
    /// Columns of the connector between the two sides.
    pub connector_width: u16,
    /// Jumping to a change (`]c`, `[c`) scrolls the other side to the same change.
    pub align_on_jump: bool,
    /// Scrolling one side scrolls the other with it, to the same place: the line matching the
    /// cursor's line of the scrolled side at the same height. Off, each side scrolls on its
    /// own (and `]s` lines them up).
    pub scroll_sync: bool,
    /// Opening a file from the changed-files list (`Space g`, the picker or the quickfix
    /// panel) shows its diff against git, and the other changed files after it with `]f`;
    /// off, the file opens as a plain buffer.
    pub open_changed_files_in_diff: bool,
    /// The marks of the staged, not staged and partly staged changes, in front of the right
    /// side of a diff against git.
    pub stage_markers: StageMarkers,
    /// The keys of the diff views, by action (see [`ACTIONS`]); the actions not set keep
    /// their default keys, and an empty string turns an action's keys off.
    #[serde(deserialize_with = "deserialize_keys")]
    pub keys: HashMap<String, String>,
    /// The keys of the merge conflict resolver, by action (see [`MERGE_ACTIONS`]), as `keys`.
    #[serde(deserialize_with = "deserialize_merge_keys")]
    pub merge_keys: HashMap<String, String>,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            connector_width: 9,
            align_on_jump: true,
            scroll_sync: true,
            open_changed_files_in_diff: true,
            stage_markers: StageMarkers::default(),
            keys: default_keys(),
            merge_keys: default_merge_keys(),
        }
    }
}

/// `[editor.diff.stage-markers]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct StageMarkers {
    pub staged: String,
    pub unstaged: String,
    pub partial: String,
}

impl Default for StageMarkers {
    fn default() -> Self {
        Self {
            staged: "▣".to_string(),
            unstaged: "□".to_string(),
            partial: "◩".to_string(),
        }
    }
}

/// The actions of the diff views, with their default keys.
pub const ACTIONS: [(&str, &str); 11] = [
    ("next-change", "]c"),
    ("previous-change", "[c"),
    ("next-file", "]f"),
    ("previous-file", "[f"),
    ("snap", "]s"),
    ("align-top", "[d"),
    ("align-bottom", "]d"),
    ("apply-left-to-right", "<gt><gt>"),
    ("stage", "-"),
    ("commit", "C"),
    ("close", "q"),
];

/// Actions there were once, still accepted in `[editor.diff.keys]` (and ignored) so that a
/// configuration setting them still loads.
const REMOVED_ACTIONS: [&str; 1] = ["apply-right-to-left"];

/// The default keys of the diff views.
pub fn default_keys() -> HashMap<String, String> {
    ACTIONS
        .into_iter()
        .map(|(action, keys)| (action.to_string(), keys.to_string()))
        .collect()
}

/// `[editor.diff.keys]`: the defaults, with the actions set replaced; an unknown action or
/// keys that don't parse are an error.
fn deserialize_keys<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_action_keys(deserializer, "keys", &ACTIONS, &REMOVED_ACTIONS)
}

/// `[editor.diff.merge-keys]`, as [`deserialize_keys`].
fn deserialize_merge_keys<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_action_keys(deserializer, "merge-keys", &MERGE_ACTIONS, &[])
}

/// A table of keys by action (`[editor.diff.<table>]`): the defaults of `actions`, with those
/// set replaced; actions of `removed` are ignored (a configuration setting them still loads).
fn deserialize_action_keys<'de, D>(
    deserializer: D,
    table: &str,
    actions: &[(&str, &str)],
    removed: &[&str],
) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let set = HashMap::<String, String>::deserialize(deserializer)?;
    let mut keys = keys_of(actions);
    for (action, sequence) in set {
        if removed.contains(&action.as_str()) {
            log::warn!("[editor.diff.{table}]: {action} is no more");
            continue;
        }
        if !actions.iter().any(|(name, _)| *name == action) {
            let names: Vec<&str> = actions.iter().map(|(name, _)| *name).collect();
            return Err(D::Error::custom(format!(
                "unknown action {action:?} in [editor.diff.{table}] (expected one of {})",
                names.join(", ")
            )));
        }
        if !sequence.is_empty() {
            crate::input::parse_macro(&sequence).map_err(|err| {
                D::Error::custom(format!("invalid keys {sequence:?} for {action}: {err}"))
            })?;
        }
        keys.insert(action, sequence);
    }
    Ok(keys)
}

/// The keys of `actions`, by action.
fn keys_of(actions: &[(&str, &str)]) -> HashMap<String, String> {
    actions
        .iter()
        .map(|(action, keys)| (action.to_string(), keys.to_string()))
        .collect()
}

/// The actions of the merge views (see [`crate::merge_view`]), with their default keys.
pub const MERGE_ACTIONS: [(&str, &str); 7] = [
    ("next-conflict", "]c"),
    ("previous-conflict", "[c"),
    ("take-local", "<gt><gt>"),
    ("take-remote", "<lt><lt>"),
    ("take-both", "gb"),
    ("snap", "]s"),
    ("close", "q"),
];

/// The default keys of the merge views.
pub fn default_merge_keys() -> HashMap<String, String> {
    keys_of(&MERGE_ACTIONS)
}

/// A diff shown side by side.
#[derive(Debug, Clone)]
pub struct DiffSession {
    pub left_view: ViewId,
    pub right_view: ViewId,
    /// The old version: a scratch buffer of this session.
    pub left_doc: DocumentId,
    pub right_doc: DocumentId,
    pub options: DiffOptions,
    /// The changes between the two documents (as they are now: see `refresh`). Shared with
    /// what draws them each frame, not copied.
    pub hunks: Arc<[DiffHunk]>,
    /// The changed words of the hunks, worked out for those shown (see
    /// [`Editor::prepare_diff_words`]): by hunk, `None` until then.
    words: Vec<Option<HunkWords>>,
    /// The versions of the documents the hunks are of.
    versions: Option<(i32, i32)>,
    /// The right document's text the hunks are of (a rope: shared, not copied).
    right_text: helix_core::Rope,
    /// The hunks only followed edits since the diff was last worked out (see
    /// [`Editor::refresh_diff_sessions`]).
    stale: bool,
    /// For a diff of a file against git (`HEAD` or the index): what staging needs.
    pub git: Option<GitFile>,
    /// The gutters of the right view before the connector was added to them.
    right_gutters: GutterConfig,
    /// Where the left and right views were scrolled to when they were last in step (see
    /// [`Editor::sync_diff_scrolling`]).
    scrolled: Option<[Scroll; 2]>,
}

/// How far a view is scrolled: its first line, and the rows of it above the window (of a
/// wrapped line).
pub(crate) type Scroll = (usize, usize);

impl DiffSession {
    pub fn new(
        left_view: ViewId,
        right_view: ViewId,
        left_doc: DocumentId,
        right_doc: DocumentId,
        options: DiffOptions,
        right_gutters: GutterConfig,
    ) -> Self {
        Self {
            left_view,
            right_view,
            left_doc,
            right_doc,
            options,
            hunks: Arc::from([]),
            words: Vec::new(),
            versions: None,
            right_text: helix_core::Rope::new(),
            stale: false,
            git: None,
            right_gutters,
            scrolled: None,
        }
    }

    /// The side `view` shows, if it is one of the session's.
    pub fn side_of(&self, view: ViewId) -> Option<Side> {
        if view == self.left_view {
            Some(Side::Left)
        } else if view == self.right_view {
            Some(Side::Right)
        } else {
            None
        }
    }

    pub fn view(&self, side: Side) -> ViewId {
        match side {
            Side::Left => self.left_view,
            Side::Right => self.right_view,
        }
    }

    pub fn doc(&self, side: Side) -> DocumentId {
        match side {
            Side::Left => self.left_doc,
            Side::Right => self.right_doc,
        }
    }

    /// The path of the right side's document, if it has one.
    pub fn right_doc_path<'a>(&self, editor: &'a Editor) -> Option<&'a std::path::Path> {
        editor.document(self.right_doc)?.path()
    }

    /// The gutters the right view had before the session.
    pub fn right_gutters(&self) -> &GutterConfig {
        &self.right_gutters
    }

    /// The changed words of hunk `index` on `side`, if worked out (see
    /// [`Editor::prepare_diff_words`]).
    pub fn words(&self, index: usize, side: Side) -> &[Range<usize>] {
        self.words
            .get(index)
            .and_then(Option::as_ref)
            .map_or(&[], |words| words.side(side))
    }
}

/// The hunks with lines in the window of `view` (showing `doc`, the `side` of a diff), as a
/// range of indices: a few more, if anything (it counts lines, not rows of wrapped ones).
pub fn shown_hunks(
    hunks: &[DiffHunk],
    view: &crate::View,
    doc: &crate::Document,
    side: Side,
) -> Range<usize> {
    let text = doc.text().slice(..);
    let top = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
    let bottom = top + view.inner_height(doc);
    let (start, _) = hunk_at(hunks, top, side);
    // up to the first one starting below the window
    let end = hunks.partition_point(|hunk| hunk.lines(side).start <= bottom);
    start..end.max(start)
}

/// A file compared with its version in git (`HEAD` or the index), for staging its changes: its
/// versions in `HEAD` and in the index, and how much of each change shown is staged.
#[derive(Debug, Clone)]
pub struct GitFile {
    /// The repository.
    pub root: std::path::PathBuf,
    /// The file's path in the repository, `/`-separated.
    pub relative: String,
    /// The file in `HEAD`, `None` when it isn't there (a new file).
    pub head: Option<helix_core::Rope>,
    /// The file in the index, `None` when it isn't there.
    pub index: Option<helix_core::Rope>,
    /// Whether the left side is `HEAD` (else the index: then only what isn't staged shows).
    pub left_is_head: bool,
    /// The changes not staged: from the index to the document (the right side).
    unstaged: Arc<[DiffHunk]>,
    /// The changes staged: from `HEAD` to the index.
    staged: Arc<[DiffHunk]>,
    /// How much of each hunk of the diff is staged, by hunk.
    stages: Vec<Stage>,
}

/// How much of a change shown is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Unstaged,
    Staged,
    Partial,
}

impl GitFile {
    pub fn new(
        root: std::path::PathBuf,
        relative: String,
        head: Option<helix_core::Rope>,
        index: Option<helix_core::Rope>,
        left_is_head: bool,
    ) -> Self {
        let mut file = Self {
            root,
            relative,
            head,
            index,
            left_is_head,
            unstaged: Arc::from([]),
            staged: Arc::from([]),
            stages: Vec::new(),
        };
        file.set_index(file.index.clone());
        file
    }

    /// `text`, or an empty text.
    pub fn text(text: &Option<helix_core::Rope>) -> helix_core::RopeSlice<'_> {
        match text {
            Some(text) => text.slice(..),
            None => helix_core::RopeSlice::from(""),
        }
    }

    /// The index after staging or unstaging is `index` (`None`: the file is out of it): the
    /// staged changes worked out again.
    pub fn set_index(&mut self, index: Option<helix_core::Rope>) {
        self.index = index;
        self.staged = diff_hunks(
            Self::text(&self.head),
            Self::text(&self.index),
            DiffOptions::default(),
        )
        .into();
    }

    /// The changes not staged, and how much of each hunk of the diff (from the left side to
    /// `right`, the document) is staged, worked out again.
    fn update(&mut self, hunks: &[DiffHunk], right: helix_core::RopeSlice) {
        self.unstaged = diff_hunks(Self::text(&self.index), right, DiffOptions::default()).into();
        let stages = hunks
            .iter()
            .map(|hunk| {
                if !self.left_is_head {
                    return Stage::Unstaged;
                }
                let staged = self.staged_of(hunk).next().is_some();
                let unstaged = self.unstaged_of(hunk).next().is_some();
                match (staged, unstaged) {
                    (true, false) => Stage::Staged,
                    (true, true) => Stage::Partial,
                    _ => Stage::Unstaged,
                }
            })
            .collect();
        self.stages = stages;
    }

    /// How much of hunk `index` of the diff is staged.
    pub fn stage(&self, index: usize) -> Option<Stage> {
        self.stages.get(index).copied()
    }

    /// The staged changes (`HEAD` to index) that are part of `hunk` (of the diff, its left side
    /// `HEAD`).
    pub fn staged_of<'a>(&'a self, hunk: &'a DiffHunk) -> impl Iterator<Item = &'a DiffHunk> {
        self.staged
            .iter()
            .filter(move |staged| helix_core::diff_hunks::ranges_touch(&staged.left, &hunk.left))
    }

    /// The changes not staged (index to document) that are part of `hunk` (of the diff).
    pub fn unstaged_of<'a>(&'a self, hunk: &'a DiffHunk) -> impl Iterator<Item = &'a DiffHunk> {
        self.unstaged.iter().filter(move |unstaged| {
            helix_core::diff_hunks::ranges_touch(&unstaged.right, &hunk.right)
        })
    }
}

/// The commit message being written (`C` in a diff, `:diff-commit`): saving it commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessage {
    /// The message's document (git's `COMMIT_EDITMSG`).
    pub doc: DocumentId,
    /// The repository.
    pub root: std::path::PathBuf,
    /// Whether the commit replaces the last one.
    pub amend: bool,
}

/// Changed files reviewed one after the other in the diff of a view (`]f`, `[f`): from the
/// changed-files list or `:diff-review`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewQueue {
    pub files: Vec<ReviewFile>,
    /// The file shown.
    pub current: usize,
    /// The right view of the diff showing the files.
    pub view: ViewId,
}

/// A file of a [`ReviewQueue`]: its path, and the version in git it is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFile {
    pub path: std::path::PathBuf,
    /// A revision (`HEAD`, a branch...), or `None` for the index (what `git diff` shows).
    pub base: Option<String>,
    /// The file has merge conflicts: it is shown in the merge resolver.
    pub conflict: bool,
}

impl Editor {
    /// The review the focused view shows a file of: the right side of its diff showing the
    /// queue's current file.
    pub fn active_review(&self) -> Option<&ReviewQueue> {
        let review = self.diff_review.as_ref()?;
        let file = review.files.get(review.current)?;
        // the right side of the diff, or the result of the merge, showing the file
        let (view, doc) = match (
            self.diff_session(self.tree.focus),
            self.merge_of(self.tree.focus),
        ) {
            (Some(session), _) => (session.right_view, session.right_doc),
            (None, Some(merge)) => (merge.result_view, merge.result_doc),
            (None, None) => return None,
        };
        let shown = self.document(doc)?.path()?;
        (view == review.view && same_file(shown, &file.path)).then_some(review)
    }
}

/// Whether `a` and `b` are the same file (one may have its links resolved, not the other).
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok() && a.exists()
}

/// The right view's gutters with the connector in front.
pub fn with_connector(gutters: &GutterConfig) -> GutterConfig {
    let mut gutters = gutters.clone();
    gutters
        .layout
        .retain(|gutter| *gutter != GutterType::DiffConnector);
    gutters.layout.insert(0, GutterType::DiffConnector);
    gutters
}

impl Editor {
    /// The session `view` belongs to, if any.
    pub fn diff_session(&self, view: ViewId) -> Option<&DiffSession> {
        self.diff_sessions
            .iter()
            .find(|session| session.side_of(view).is_some())
    }

    /// Brings the sessions up to date with the documents, and ends a session whose view or
    /// document is gone (the other view keeps its document; the right one gets its gutters
    /// back; the left snapshot is closed).
    ///
    /// While the right side is typed into (insert mode), its hunks only follow the edits
    /// (see [`helix_core::diff_hunks::shift_hunks`]): cheap at any size. The diff is worked
    /// out again otherwise: out of insert mode, after a pause in typing
    /// ([`Editor::recompute_stale_diffs`] on the idle timeout), and before the diff actions.
    pub fn refresh_diff_sessions(&mut self) {
        let typing = self.mode == crate::document::Mode::Insert;
        let mut ended = Vec::new();
        for index in 0..self.diff_sessions.len() {
            let session = &self.diff_sessions[index];
            let views_alive =
                self.tree.contains(session.left_view) && self.tree.contains(session.right_view);
            let (Some(left), Some(right)) = (
                self.documents.get(&session.left_doc),
                self.documents.get(&session.right_doc),
            ) else {
                ended.push(index);
                continue;
            };
            // a view showing another document left the session
            let shown = views_alive
                && self.tree.get(session.left_view).doc == session.left_doc
                && self.tree.get(session.right_view).doc == session.right_doc;
            if !shown {
                ended.push(index);
                continue;
            }
            let versions = (left.version(), right.version());
            let changed = session.versions != Some(versions);
            // edits only to the right side, followed since the diff was worked out (by this
            // session alone: another diff of the same buffer restarts the following)
            let followable = session.versions.map(|(left, _)| left) == Some(versions.0)
                && right.diff_changes().is_some()
                && self
                    .diff_sessions
                    .iter()
                    .filter(|other| other.right_doc == session.right_doc)
                    .count()
                    == 1;
            if changed && typing && followable {
                self.shift_diff(index);
            } else if changed || (session.stale && !typing) {
                self.recompute_diff(index);
            }
        }
        for index in ended.into_iter().rev() {
            let session = self.diff_sessions.remove(index);
            self.end_diff_session(session);
        }
    }

    /// Makes the diff whose right view is `right_view` a diff against git (see [`GitFile`]):
    /// its changes can be staged.
    pub fn set_diff_git(&mut self, right_view: ViewId, git: GitFile) {
        if let Some(index) = self
            .diff_sessions
            .iter()
            .position(|session| session.right_view == right_view)
        {
            self.diff_sessions[index].git = Some(git);
            self.recompute_diff(index);
        }
    }

    /// The index of the file of the diff whose right view is `right_view` became `index`
    /// (`None`: the file is out of it), after staging or unstaging: the staged changes follow,
    /// and the left side when it shows the index.
    pub fn diff_index_changed(&mut self, right_view: ViewId, index: Option<helix_core::Rope>) {
        let Some(at) = self
            .diff_sessions
            .iter()
            .position(|session| session.right_view == right_view)
        else {
            return;
        };
        let session = &mut self.diff_sessions[at];
        let Some(git) = &mut session.git else {
            return;
        };
        git.set_index(index.clone());
        if !git.left_is_head {
            if let Some(left) = self.documents.get_mut(&session.left_doc) {
                // the lock lifted for this one change
                let locked = left.locked.take();
                let new = index.unwrap_or_default();
                let change = helix_core::diff::compare_ropes(left.text(), &new);
                left.apply(&change, session.left_view);
                left.locked = locked;
            }
        }
        self.recompute_diff(at);
    }

    /// The diff whose right view is `right_view` shown again against `old` (the version in git
    /// changed: a commit), with `git` for staging.
    pub fn update_git_diff(
        &mut self,
        right_view: ViewId,
        old: helix_core::Rope,
        git: Option<GitFile>,
    ) {
        let Some(at) = self
            .diff_sessions
            .iter()
            .position(|session| session.right_view == right_view)
        else {
            return;
        };
        let session = &mut self.diff_sessions[at];
        if let Some(left) = self.documents.get_mut(&session.left_doc) {
            let locked = left.locked.take();
            let change = helix_core::diff::compare_ropes(left.text(), &old);
            left.apply(&change, session.left_view);
            left.locked = locked;
        }
        session.git = git;
        self.recompute_diff(at);
    }

    /// Works out again the diffs whose hunks only followed edits (see
    /// [`Editor::refresh_diff_sessions`]); whether there were any.
    pub fn recompute_stale_diffs(&mut self) -> bool {
        let mut any = false;
        if self.merge.as_ref().is_some_and(|merge| merge.is_stale()) {
            self.recompute_merge();
            any = true;
        }
        for index in 0..self.diff_sessions.len() {
            let session = &self.diff_sessions[index];
            let present = self.documents.contains_key(&session.left_doc)
                && self.documents.contains_key(&session.right_doc);
            if present && session.stale {
                self.recompute_diff(index);
                any = true;
            }
        }
        any
    }

    /// The diff of session `index` worked out from the documents as they are now.
    fn recompute_diff(&mut self, index: usize) {
        let session = &mut self.diff_sessions[index];
        let Some((left_text, left_version)) = self
            .documents
            .get(&session.left_doc)
            .map(|left| (left.text().clone(), left.version()))
        else {
            return;
        };
        let Some(right) = self.documents.get_mut(&session.right_doc) else {
            return;
        };
        session.hunks =
            diff_hunks(left_text.slice(..), right.text().slice(..), session.options).into();
        if let Some(git) = &mut session.git {
            git.update(&session.hunks, right.text().slice(..));
        }
        session.words = vec![None; session.hunks.len()];
        session.versions = Some((left_version, right.version()));
        session.right_text = right.text().clone();
        session.stale = false;
        right.track_diff_changes();
    }

    /// The hunks of session `index` moved along with the edits made to the right side since.
    fn shift_diff(&mut self, index: usize) {
        let session = &mut self.diff_sessions[index];
        let Some(left_version) = self
            .documents
            .get(&session.left_doc)
            .map(|left| left.version())
        else {
            return;
        };
        let Some(right) = self.documents.get_mut(&session.right_doc) else {
            return;
        };
        let Some(changes) = right.diff_changes() else {
            return;
        };
        session.hunks = helix_core::diff_hunks::shift_hunks(
            &session.hunks,
            session.right_text.slice(..),
            right.text().slice(..),
            changes,
        )
        .into();
        // the words of the hunks shown are worked out again, from the texts as they are
        session.words = vec![None; session.hunks.len()];
        session.versions = Some((left_version, right.version()));
        session.right_text = right.text().clone();
        session.stale = true;
        right.track_diff_changes();
    }

    /// Ends `session` (already out of the list): the right view gets its gutters back, the left
    /// view and its snapshot go.
    pub fn end_diff_session(&mut self, session: DiffSession) {
        let followed_elsewhere = self
            .diff_sessions
            .iter()
            .any(|other| other.right_doc == session.right_doc);
        if !followed_elsewhere {
            if let Some(doc) = self.documents.get_mut(&session.right_doc) {
                doc.untrack_diff_changes();
            }
        }
        if self.tree.contains(session.right_view) {
            self.tree.get_mut(session.right_view).gutters = session.right_gutters.clone();
        }
        if self.tree.contains(session.left_view)
            && self.tree.get(session.left_view).doc == session.left_doc
        {
            self.close(session.left_view);
        }
        if self.documents.contains_key(&session.left_doc) {
            let still_shown = self
                .tree
                .views()
                .any(|(view, _)| view.doc == session.left_doc);
            if !still_shown {
                if self.tree.views().next().is_some() {
                    let _ = self.close_document(session.left_doc, true);
                } else {
                    // no window left (the diff's last one closed: Helix quits): closing the
                    // document would open a window for another buffer
                    self.documents.remove(&session.left_doc);
                }
            }
        }
    }

    /// Ends the session of `view`, if it has one.
    pub fn close_diff(&mut self, view: ViewId) -> bool {
        let Some(index) = self
            .diff_sessions
            .iter()
            .position(|session| session.side_of(view).is_some())
        else {
            return false;
        };
        let session = self.diff_sessions.remove(index);
        self.end_diff_session(session);
        true
    }
}

/// The old version of a diff (the left side): its text, the title of its buffer, and a file
/// name for its syntax highlighting.
pub struct OldVersion {
    /// Its text: a rope, so that a snapshot of a buffer shares the buffer's (until either
    /// changes) instead of copying it.
    pub text: helix_core::Rope,
    pub title: String,
    pub syntax_path: Option<std::path::PathBuf>,
}

impl Editor {
    /// Shows `right` (a document) beside `old` (a read-only snapshot, on the left) as a diff,
    /// in the focused view and a new one on its left; the focus stays on the right side, and
    /// the left side is lined up with it. A diff in the focused view is closed first.
    pub fn open_diff(&mut self, old: OldVersion, right: DocumentId) -> ViewId {
        use crate::{editor::Action, tree::Direction, Document};
        let focused = self.tree.focus;
        self.close_diff(focused);
        self.switch(right, Action::Replace);
        let right_view = self.tree.focus;

        let mut doc = Document::from(
            old.text,
            None,
            std::sync::Arc::clone(&self.config),
            std::sync::Arc::clone(&self.syn_loader),
        );
        doc.title = Some(old.title);
        doc.readonly = true;
        doc.locked = Some("The old version is read-only".to_string());
        if let Some(path) = &old.syntax_path {
            let loader = self.syn_loader.load();
            if let Some(language) = loader.language_for_filename(path) {
                doc.set_language(Some(loader.language(language).config().clone()), &loader);
            }
        }
        let left_doc = self.new_file_from_document(Action::VerticalSplit, doc);
        if let Some(doc) = self.documents.get_mut(&left_doc) {
            doc.reset_modified();
        }
        let left_view = self.tree.focus;
        // the old version on the left
        self.swap_split_in_direction(Direction::Left);

        let right_gutters = self.tree.get(right_view).gutters.clone();
        self.tree.get_mut(right_view).gutters = with_connector(&right_gutters);
        let options = DiffOptions {
            ignore_whitespace: self.config().diff.ignore_whitespace,
        };
        self.diff_sessions.push(DiffSession::new(
            left_view,
            right_view,
            left_doc,
            right,
            options,
            right_gutters,
        ));
        self.focus(right_view);
        self.refresh_diff_sessions();
        self.align_diff(right_view);
        left_view
    }

    /// Lines the other side of `view`'s diff up with it: its cursor on the line matching the
    /// cursor's line of `view`, at the same height in the window.
    pub fn align_diff(&mut self, view: ViewId) {
        let Some(session) = self.diff_session(view) else {
            return;
        };
        let Some(side) = session.side_of(view) else {
            return;
        };
        let other_view = session.view(side.other());
        let other_doc = session.doc(side.other());
        let doc_id = session.doc(side);
        let hunks = session.hunks.clone();
        let Some(doc) = self.documents.get(&doc_id) else {
            return;
        };
        let text = doc.text().slice(..);
        let cursor = doc.selection(view).primary().cursor(text);
        let line = text.char_to_line(cursor);
        let row = self
            .tree
            .try_get(view)
            .and_then(|view| view.screen_coords_at_pos(doc, text, cursor))
            .map_or(0, |pos| pos.row);
        let other_line = helix_core::diff_hunks::map_line(&hunks, line, side.other());
        self.show_diff_line(other_view, other_doc, other_line, row);
        self.mark_diff_aligned(view);
    }

    /// Puts `line` of `doc` in `view` at `row` of the window (as near as the top of the
    /// document allows), with the cursor on it.
    pub fn show_diff_line(&mut self, view: ViewId, doc: DocumentId, line: usize, row: usize) {
        let Some(doc) = self.documents.get_mut(&doc) else {
            return;
        };
        let text = doc.text().slice(..);
        let lines = helix_core::diff_hunks::line_count(text);
        let line = line.min(lines.saturating_sub(1));
        let pos = text.line_to_char(line);
        let top = text.line_to_char(line.saturating_sub(row));
        doc.set_selection(view, helix_core::Selection::point(pos));
        doc.set_view_offset(
            view,
            crate::view::ViewPosition {
                anchor: top,
                horizontal_offset: 0,
                vertical_offset: 0,
            },
        );
    }

    /// Where `view` (showing `doc`) is scrolled to.
    pub(crate) fn diff_scroll(&self, view: ViewId, doc: DocumentId) -> Option<Scroll> {
        let doc = self.documents.get(&doc)?;
        let offset = doc.get_view_offset(view)?;
        let text = doc.text().slice(..);
        let line = text.char_to_line(offset.anchor.min(text.len_chars()));
        Some((line, offset.vertical_offset))
    }

    /// Where the left and right sides of `session` are scrolled to.
    fn diff_scrolls(&self, session: &DiffSession) -> Option<[Scroll; 2]> {
        Some([
            self.diff_scroll(session.left_view, session.left_doc)?,
            self.diff_scroll(session.right_view, session.right_doc)?,
        ])
    }

    /// Takes the two sides of `view`'s diff as in step where they are now: scrolling syncs on
    /// from there. For the actions that line the sides up themselves (`]c`, `]s`, `[d`...),
    /// so that the next scroll keeps their alignment rather than redoing it.
    pub fn mark_diff_aligned(&mut self, view: ViewId) {
        let Some(index) = self
            .diff_sessions
            .iter()
            .position(|session| session.side_of(view).is_some())
        else {
            return;
        };
        self.diff_sessions[index].scrolled = self.diff_scrolls(&self.diff_sessions[index]);
    }

    /// Keeps the two sides of each diff scrolled together (`scroll-sync`): a side that
    /// scrolled since they were last in step (by keys, the mouse wheel, a jump, a plugin...)
    /// takes the other one along, which then shows the line matching the cursor's line of the
    /// scrolled side at the same height (or, the cursor out of the window, the line in its
    /// middle). Both scrolled: the focused one leads.
    pub fn sync_diff_scrolling(&mut self) {
        if !self.config().diff.scroll_sync {
            for session in &mut self.diff_sessions {
                session.scrolled = None;
            }
            return;
        }
        for index in 0..self.diff_sessions.len() {
            let session = &self.diff_sessions[index];
            let Some(now) = self.diff_scrolls(session) else {
                continue;
            };
            let Some(before) = session.scrolled else {
                self.diff_sessions[index].scrolled = Some(now);
                continue;
            };
            let leader = match (now[0] != before[0], now[1] != before[1]) {
                (false, false) => continue,
                (true, false) => Side::Left,
                (false, true) => Side::Right,
                (true, true) => session.side_of(self.tree.focus).unwrap_or(Side::Right),
            };
            let session = session.clone();
            self.follow_diff_scroll(&session, leader);
            self.diff_sessions[index].scrolled = self.diff_scrolls(&self.diff_sessions[index]);
        }
    }

    /// Works out the changed words of the hunks shown in the views of each diff, those not
    /// done yet since the documents last changed: only what is on screen is word-diffed.
    pub fn prepare_diff_words(&mut self) {
        for session in &mut self.diff_sessions {
            let (Some(left), Some(right)) = (
                self.documents.get(&session.left_doc),
                self.documents.get(&session.right_doc),
            ) else {
                continue;
            };
            for side in [Side::Left, Side::Right] {
                let (Some(view), doc) = (
                    self.tree.try_get(session.view(side)),
                    if side == Side::Left { left } else { right },
                ) else {
                    continue;
                };
                for index in shown_hunks(&session.hunks, view, doc, side) {
                    let hunk = &session.hunks[index];
                    if session.words[index].is_none() && hunk.kind() == HunkKind::Changed {
                        session.words[index] = Some(hunk_words(
                            left.text().slice(..),
                            right.text().slice(..),
                            hunk,
                            session.options,
                        ));
                    }
                }
            }
        }
    }

    /// Scrolls the side of `session` other than `leader` to where `leader` is.
    fn follow_diff_scroll(&mut self, session: &DiffSession, leader: Side) {
        let Some((line, row)) = self.scroll_reference(session.view(leader), session.doc(leader))
        else {
            return;
        };
        let other = leader.other();
        let other_line = helix_core::diff_hunks::map_line(&session.hunks, line, other);
        self.scroll_diff_side(session.view(other), session.doc(other), other_line, row);
    }

    /// The line of `view` (showing `doc`) the views scrolled with it line up with, and its row:
    /// the cursor's, or (the cursor out of the window) the middle row's.
    pub(crate) fn scroll_reference(
        &self,
        view_id: ViewId,
        doc_id: DocumentId,
    ) -> Option<(usize, usize)> {
        let (view, doc) = (self.tree.try_get(view_id)?, self.documents.get(&doc_id)?);
        let text = doc.text().slice(..);
        let height = view.inner_height(doc);
        let cursor = doc.selection(view_id).primary().cursor(text);
        Some(match view.screen_coords_at_pos(doc, text, cursor) {
            Some(pos) if pos.row < height => (text.char_to_line(cursor), pos.row),
            _ => {
                let row = height / 2;
                let line = match view.pos_at_visual_coords(doc, row as u16, 0, false) {
                    Some(pos) => text.char_to_line(pos.min(text.len_chars())),
                    None => {
                        let anchor = doc.view_offset(view_id).anchor.min(text.len_chars());
                        text.char_to_line(anchor) + row
                    }
                };
                (line, row)
            }
        })
    }

    /// Scrolls `view` (showing `doc`) to have `line` at `row` of the window (as near as the top
    /// of the document allows). The cursor stays where it is if Helix would not scroll to it,
    /// or goes to `line`.
    pub(crate) fn scroll_diff_side(
        &mut self,
        view_id: ViewId,
        doc_id: DocumentId,
        line: usize,
        row: usize,
    ) {
        let scrolloff = self.config().scrolloff;
        let (Some(view), Some(doc)) = (self.tree.try_get(view_id), self.documents.get_mut(&doc_id))
        else {
            return;
        };
        let text = doc.text().slice(..);
        let lines = helix_core::diff_hunks::line_count(text).max(1);
        let line = line.min(lines - 1);
        let top = line.saturating_sub(row);
        let horizontal_offset = doc.view_offset(view_id).horizontal_offset;
        doc.set_view_offset(
            view_id,
            crate::view::ViewPosition {
                anchor: text.line_to_char(top),
                horizontal_offset,
                vertical_offset: 0,
            },
        );
        // the cursor where Helix would scroll the view to it (out of the window, or in its
        // scroll-off margin) goes to the matching line: else the next key or wheel step on
        // this side would scroll it back, and the other side with it
        if view.offset_coords_to_in_view(doc, scrolloff).is_some() {
            let text = doc.text().slice(..);
            let pos = text.line_to_char(line);
            doc.set_selection(view_id, helix_core::Selection::point(pos));
        }
    }
}

/// The styles of a diff, from the theme: `ui.diff.add`, `ui.diff.delete`, `ui.diff.change`
/// and `ui.diff.change.emphasis` (backgrounds), or, without them, backgrounds mixed from the
/// colors of `diff.plus`, `diff.minus` and `diff.delta` and the background of the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffStyles {
    pub add: Style,
    pub delete: Style,
    pub change: Style,
    pub change_emphasis: Style,
}

/// How much of the diff color goes into a line's background, and into a changed word's.
const LINE_BLEND: f32 = 0.2;
const EMPHASIS_BLEND: f32 = 0.42;
/// How far (in the channel that differs most, out of 255) a line's background, and a changed
/// word's, is from the editor's background at least: a dark diff color on a dark theme still
/// shows.
const LINE_CONTRAST: u8 = 28;
const EMPHASIS_CONTRAST: u8 = 64;
/// Below this spread between their channels, colors count as greys: a monochrome theme's diff
/// colors tell the kinds of change apart by brightness only, which a tint of the background
/// loses, so the default tints below are used instead.
const GREY_SPREAD: u8 = 32;
/// The tints of the kinds of change when the theme's colors are greys (or missing).
const ADD_TINT: Color = Color::Rgb(0x3f, 0xb9, 0x50);
const DELETE_TINT: Color = Color::Rgb(0xf8, 0x51, 0x49);
const CHANGE_TINT: Color = Color::Rgb(0x58, 0xa6, 0xff);

impl DiffStyles {
    pub fn new(theme: &Theme) -> Self {
        let background = theme.get("ui.background").bg;
        let known_background = matches!(background, Some(Color::Rgb(..)));
        let derived = |scope: &str, tint: Color, amount: f32, contrast: u8| -> Style {
            let color = match theme.get(scope).fg {
                Some(color) if is_grey(color) => Some(tint),
                Some(color) => Some(color),
                // no color, but a background to tint
                None if known_background => Some(tint),
                None => None,
            };
            match color {
                Some(color) => {
                    Style::default().bg(blend_visibly(color, background, amount, contrast))
                }
                None => Style::default().add_modifier(Modifier::REVERSED),
            }
        };
        let line = |scope: &str, tint: Color| derived(scope, tint, LINE_BLEND, LINE_CONTRAST);
        let add = theme
            .try_get_exact("ui.diff.add")
            .unwrap_or_else(|| line("diff.plus", ADD_TINT));
        let delete = theme
            .try_get_exact("ui.diff.delete")
            .unwrap_or_else(|| line("diff.minus", DELETE_TINT));
        let change = theme
            .try_get_exact("ui.diff.change")
            .unwrap_or_else(|| line("diff.delta", CHANGE_TINT));
        let change_emphasis = theme
            .try_get_exact("ui.diff.change.emphasis")
            .unwrap_or_else(|| {
                let style = derived("diff.delta", CHANGE_TINT, EMPHASIS_BLEND, EMPHASIS_CONTRAST);
                // without colors to mix, at least something to see
                if style.bg.is_some_and(|bg| !matches!(bg, Color::Rgb(..))) {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                }
            });
        Self {
            add,
            delete,
            change,
            change_emphasis,
        }
    }

    /// The style of the lines of a hunk of `kind` on `side`.
    pub fn lines(&self, kind: HunkKind, side: Side) -> Style {
        match (kind, side) {
            (HunkKind::Changed, _) => self.change,
            (HunkKind::Added, _) => self.add,
            (HunkKind::Removed, _) => self.delete,
        }
    }
}

/// `color` mixed into `background` by `amount` (0 to 1). A color that is not RGB is used as
/// it is; no background counts as black.
pub fn blend(color: Color, background: Option<Color>, amount: f32) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let (br, bg, bb) = match background {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        _ => (0, 0, 0),
    };
    let mix = |c: u8, base: u8| -> u8 {
        (base as f32 + (c as f32 - base as f32) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
}

/// Whether `color` (RGB) is a grey: its channels about the same. Other colors are not.
pub fn is_grey(color: Color) -> bool {
    let Color::Rgb(r, g, b) = color else {
        return false;
    };
    r.max(g).max(b) - r.min(g).min(b) < GREY_SPREAD
}

/// `color` mixed into `background` by `amount`, or by more if that is not `contrast` away from
/// the background (in the channel that differs most): as much more as it takes, up to `color`
/// itself.
pub fn blend_visibly(color: Color, background: Option<Color>, amount: f32, contrast: u8) -> Color {
    let (Color::Rgb(r, g, b), base) = (color, background) else {
        return color;
    };
    let (br, bg, bb) = match base {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        _ => (0, 0, 0),
    };
    let spread = [r.abs_diff(br), g.abs_diff(bg), b.abs_diff(bb)]
        .into_iter()
        .max()
        .unwrap_or(0);
    let amount = if spread == 0 {
        amount
    } else {
        amount.max(contrast as f32 / spread as f32).min(1.0)
    };
    blend(color, background, amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_blend_into_the_background() {
        let mixed = blend(Color::Rgb(200, 100, 0), Some(Color::Rgb(0, 0, 0)), 0.5);
        assert_eq!(mixed, Color::Rgb(100, 50, 0));
        let mixed = blend(
            Color::Rgb(255, 255, 255),
            Some(Color::Rgb(55, 55, 55)),
            0.25,
        );
        assert_eq!(mixed, Color::Rgb(105, 105, 105));
        assert_eq!(blend(Color::Red, None, 0.5), Color::Red);
    }

    fn theme(toml: &str) -> Theme {
        toml::from_str(toml).expect("a theme")
    }

    fn background(style: Style) -> (u8, u8, u8) {
        match style.bg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected an RGB background, got {other:?}"),
        }
    }

    #[test]
    fn greys_are_told_apart_from_colors() {
        let grey: bool = is_grey(Color::Rgb(0xaa, 0xab, 0xa6));
        assert!(grey);
        let purplish_grey: bool = is_grey(Color::Rgb(0x63, 0x53, 0x61));
        assert!(purplish_grey);
        let green: bool = is_grey(Color::Rgb(0xa6, 0xe2, 0x2e));
        assert!(!green);
        let named: bool = is_grey(Color::Gray);
        assert!(!named);
    }

    #[test]
    fn faint_colors_get_enough_contrast() {
        // 20% of a dark green on black is barely there: mixed in until 28 away
        let mixed: Color = blend_visibly(
            Color::Rgb(0, 0x40, 0),
            Some(Color::Rgb(0, 0, 0)),
            LINE_BLEND,
            LINE_CONTRAST,
        );
        assert_eq!(mixed, Color::Rgb(0, 28, 0));
        // a bright color is far enough at 20%
        let mixed: Color = blend_visibly(
            Color::Rgb(200, 100, 0),
            Some(Color::Rgb(0, 0, 0)),
            LINE_BLEND,
            LINE_CONTRAST,
        );
        assert_eq!(mixed, Color::Rgb(40, 20, 0));
        // never past the color itself
        let mixed: Color = blend_visibly(
            Color::Rgb(10, 10, 10),
            Some(Color::Rgb(0, 0, 0)),
            LINE_BLEND,
            LINE_CONTRAST,
        );
        assert_eq!(mixed, Color::Rgb(10, 10, 10));
    }

    #[test]
    fn a_monochrome_theme_gets_tinted_backgrounds() {
        // simpledark's diff colors: greys
        let mono = theme(
            r##""ui.background" = { bg = "#121212" }
"diff.plus" = "#aaaba6"
"diff.minus" = "#635361"
"diff.delta" = "#939490""##,
        );
        let styles: DiffStyles = DiffStyles::new(&mono);
        let (add, delete, change) = (
            background(styles.add),
            background(styles.delete),
            background(styles.change),
        );
        assert_eq!(add, (0x1b, 0x33, 0x1e));
        assert_eq!(delete, (0x40, 0x1f, 0x1d));
        assert_eq!(change, (0x20, 0x30, 0x41));
        // each the color of its kind
        assert!(add.1 > add.0 && add.1 > add.2);
        assert!(delete.0 > delete.1 && delete.0 > delete.2);
        assert!(change.2 > change.0 && change.2 > change.1);
        let emphasis = background(styles.change_emphasis);
        assert!(
            emphasis.2 > change.2,
            "{emphasis:?} stronger than {change:?}"
        );
    }

    #[test]
    fn a_colorful_theme_keeps_its_colors_and_ui_diff_wins() {
        let colorful = theme(
            r##""ui.background" = { bg = "#000000" }
"diff.plus" = "#00ff00"
"diff.minus" = "#ff0000"
"diff.delta" = "#0000ff"
"ui.diff.change" = { bg = "#123456" }"##,
        );
        let styles: DiffStyles = DiffStyles::new(&colorful);
        assert_eq!(styles.add.bg, Some(Color::Rgb(0, 51, 0)));
        assert_eq!(styles.delete.bg, Some(Color::Rgb(51, 0, 0)));
        assert_eq!(styles.change.bg, Some(Color::Rgb(0x12, 0x34, 0x56)));
    }

    #[test]
    fn keys_keep_their_defaults_unless_set() {
        let config: DiffConfig = toml::from_str("[keys]\nclose = \"Q\"\nsnap = \"\"").unwrap();
        assert_eq!(config.keys["close"], "Q");
        assert_eq!(config.keys["snap"], "");
        assert_eq!(config.keys["next-change"], "]c");
        assert_eq!(config.keys.len(), ACTIONS.len());
        let wrong = toml::from_str::<DiffConfig>("[keys]\njump = \"j\"");
        assert!(wrong
            .unwrap_err()
            .to_string()
            .contains("unknown action \"jump\""));
        let wrong = toml::from_str::<DiffConfig>("[keys]\nclose = \"<nope>\"");
        assert!(wrong.unwrap_err().to_string().contains("invalid keys"));
        // the stage marks: those set replaced, the others kept
        let marks: DiffConfig = toml::from_str("[stage-markers]\nstaged = \"S\"").expect("loads");
        let expected = StageMarkers {
            staged: "S".to_string(),
            ..StageMarkers::default()
        };
        assert_eq!(marks.stage_markers, expected);
        assert_eq!(marks.stage_markers.unstaged, "□");
        // the merge keys: their own table, with their own actions
        let merge: DiffConfig = toml::from_str("[merge-keys]\ntake-both = \"B\"").expect("loads");
        assert_eq!(merge.merge_keys["take-both"], "B");
        assert_eq!(merge.merge_keys["take-local"], "<gt><gt>");
        assert_eq!(merge.merge_keys.len(), MERGE_ACTIONS.len());
        let wrong = toml::from_str::<DiffConfig>("[merge-keys]\nstage = \"-\"");
        assert!(wrong
            .unwrap_err()
            .to_string()
            .contains("unknown action \"stage\" in [editor.diff.merge-keys]"));
        // an action that is no more still loads, and is ignored
        let old: DiffConfig =
            toml::from_str("[keys]\napply-right-to-left = \"<lt><lt>\"").expect("loads");
        assert!(!old.keys.contains_key("apply-right-to-left"));
        assert_eq!(old.keys.len(), ACTIONS.len());
    }

    #[test]
    fn the_connector_goes_first_once() {
        let gutters = GutterConfig::default();
        let with = with_connector(&with_connector(&gutters));
        assert_eq!(with.layout[0], GutterType::DiffConnector);
        assert_eq!(
            with.layout
                .iter()
                .filter(|gutter| **gutter == GutterType::DiffConnector)
                .count(),
            1
        );
        assert_eq!(with.layout[1..], gutters.layout[..]);
    }
}
