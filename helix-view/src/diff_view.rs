//! Side-by-side diffs: two views next to each other, the old version on the left (a read-only
//! snapshot) and the document on the right (editable as usual), the changed lines and words
//! highlighted on both sides and a connector between them that links each change on the left
//! to the same change on the right. Each side keeps its own lines: nothing is inserted to line
//! them up; the connector bends instead.

use std::collections::HashMap;

use helix_core::diff_hunks::{diff_hunks, DiffHunk, DiffOptions, HunkKind, Side};
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
    /// The keys of the diff views, by action (see [`ACTIONS`]); the actions not set keep
    /// their default keys, and an empty string turns an action's keys off.
    #[serde(deserialize_with = "deserialize_keys")]
    pub keys: HashMap<String, String>,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            connector_width: 9,
            align_on_jump: true,
            keys: default_keys(),
        }
    }
}

/// The actions of the diff views, with their default keys.
pub const ACTIONS: [(&str, &str); 8] = [
    ("next-change", "]c"),
    ("previous-change", "[c"),
    ("snap", "]s"),
    ("align-top", "[d"),
    ("align-bottom", "]d"),
    ("apply-left-to-right", "<gt><gt>"),
    ("apply-right-to-left", "<lt><lt>"),
    ("close", "q"),
];

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
    use serde::de::Error;
    let set = HashMap::<String, String>::deserialize(deserializer)?;
    let mut keys = default_keys();
    for (action, sequence) in set {
        if !ACTIONS.iter().any(|(name, _)| *name == action) {
            let names: Vec<&str> = ACTIONS.iter().map(|(name, _)| *name).collect();
            return Err(D::Error::custom(format!(
                "unknown action {action:?} in [editor.diff.keys] (expected one of {})",
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

/// A diff shown side by side.
#[derive(Debug, Clone)]
pub struct DiffSession {
    pub left_view: ViewId,
    pub right_view: ViewId,
    /// The old version: a scratch buffer of this session.
    pub left_doc: DocumentId,
    pub right_doc: DocumentId,
    pub options: DiffOptions,
    /// The changes between the two documents (as they are now: see `refresh`).
    pub hunks: Vec<DiffHunk>,
    /// The versions of the documents the hunks are of.
    versions: Option<(i32, i32)>,
    /// The gutters of the right view before the connector was added to them.
    right_gutters: GutterConfig,
}

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
            hunks: Vec::new(),
            versions: None,
            right_gutters,
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

    /// The gutters the right view had before the session.
    pub fn right_gutters(&self) -> &GutterConfig {
        &self.right_gutters
    }
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

    /// Brings the sessions up to date: the hunks of the documents as they are now, and the
    /// end of a session whose view or document is gone (the other view keeps its document;
    /// the right one gets its gutters back; the left snapshot is closed).
    pub fn refresh_diff_sessions(&mut self) {
        let mut ended = Vec::new();
        for (index, session) in self.diff_sessions.iter_mut().enumerate() {
            let views_alive =
                self.tree.contains(session.left_view) && self.tree.contains(session.right_view);
            let docs = (
                self.documents.get(&session.left_doc),
                self.documents.get(&session.right_doc),
            );
            let (Some(left), Some(right)) = docs else {
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
            if session.versions != Some(versions) {
                session.hunks = diff_hunks(
                    left.text().slice(..),
                    right.text().slice(..),
                    session.options,
                );
                session.versions = Some(versions);
            }
        }
        for index in ended.into_iter().rev() {
            let session = self.diff_sessions.remove(index);
            self.end_diff_session(session);
        }
    }

    /// Ends `session` (already out of the list): the right view gets its gutters back, the left
    /// view and its snapshot go.
    pub fn end_diff_session(&mut self, session: DiffSession) {
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
    pub text: String,
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
            helix_core::Rope::from(old.text),
            None,
            std::sync::Arc::clone(&self.config),
            std::sync::Arc::clone(&self.syn_loader),
        );
        doc.title = Some(old.title);
        doc.readonly = true;
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

impl DiffStyles {
    pub fn new(theme: &Theme) -> Self {
        let background = theme.get("ui.background").bg;
        let derived = |scope: &str, amount: f32| -> Style {
            let color = theme.get(scope).fg;
            match color {
                Some(color) => Style::default().bg(blend(color, background, amount)),
                None => Style::default().add_modifier(Modifier::REVERSED),
            }
        };
        let add = theme
            .try_get_exact("ui.diff.add")
            .unwrap_or_else(|| derived("diff.plus", LINE_BLEND));
        let delete = theme
            .try_get_exact("ui.diff.delete")
            .unwrap_or_else(|| derived("diff.minus", LINE_BLEND));
        let change = theme
            .try_get_exact("ui.diff.change")
            .unwrap_or_else(|| derived("diff.delta", LINE_BLEND));
        let change_emphasis = theme
            .try_get_exact("ui.diff.change.emphasis")
            .unwrap_or_else(|| {
                let style = derived("diff.delta", EMPHASIS_BLEND);
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
