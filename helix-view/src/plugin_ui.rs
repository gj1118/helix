//! What plugins add to the diff viewer, the explorer and the quickfix panel: keys of their own
//! there (actions), and marks on the entries and changes.

use std::path::PathBuf;

use crate::input::KeyEvent;
use crate::quicklist::Target;
use crate::ViewId;

/// Where the keys of a plugin action work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionPlace {
    /// The views of a side-by-side diff, in normal mode.
    Diff,
    /// The explorer panel, while it has the focus.
    Explorer,
    /// The quickfix panel, while it has the focus.
    Quickfix,
}

impl ActionPlace {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionPlace::Diff => "diff",
            ActionPlace::Explorer => "explorer",
            ActionPlace::Quickfix => "quickfix",
        }
    }
}

/// A key of a plugin in the diff viewer, the explorer or the quickfix panel.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginAction {
    /// The plugin that added it.
    pub plugin: String,
    pub place: ActionPlace,
    /// One key in the panels, a sequence (`<space>e`) in the diff views.
    pub keys: Vec<KeyEvent>,
    /// What it does, for the lists of keys.
    pub description: String,
    /// The plugin engine runs it by this id (see `PluginManager::run_action`).
    pub id: u64,
}

/// What `keys` are to the actions of a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMatch {
    /// The keys of the action of this id.
    Action(u64),
    /// The start of the keys of an action: wait for more.
    Prefix,
    /// No action's keys.
    None,
}

/// The actions of all plugins.
#[derive(Debug, Default, Clone)]
pub struct PluginActions {
    actions: Vec<PluginAction>,
}

impl PluginActions {
    /// Adds `action`, replacing the action with the same keys in the same place (whichever
    /// plugin added it). Returns the id of the action it replaced.
    pub fn add(&mut self, action: PluginAction) -> Option<u64> {
        let replaced = self
            .actions
            .iter()
            .position(|a| a.place == action.place && a.keys == action.keys)
            .map(|index| self.actions.remove(index).id);
        self.actions.push(action);
        replaced
    }

    /// Removes the action of `plugin` with `keys` in `place`; returns its id.
    pub fn remove(&mut self, plugin: &str, place: ActionPlace, keys: &[KeyEvent]) -> Option<u64> {
        let index = self
            .actions
            .iter()
            .position(|a| a.plugin == plugin && a.place == place && a.keys == keys)?;
        Some(self.actions.remove(index).id)
    }

    /// Removes every action (the plugins are reloaded).
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// The actions of `place`, in the order they were added.
    pub fn of(&self, place: ActionPlace) -> impl Iterator<Item = &PluginAction> {
        self.actions.iter().filter(move |a| a.place == place)
    }

    /// The action of `id`.
    pub fn get(&self, id: u64) -> Option<&PluginAction> {
        self.actions.iter().find(|a| a.id == id)
    }

    pub fn is_empty(&self, place: ActionPlace) -> bool {
        self.of(place).next().is_none()
    }

    /// What `keys` (typed so far) are to the actions of `place`.
    pub fn matching(&self, place: ActionPlace, keys: &[KeyEvent]) -> ActionMatch {
        if keys.is_empty() {
            return ActionMatch::None;
        }
        let mut prefix = false;
        for action in self.of(place) {
            if action.keys == keys {
                return ActionMatch::Action(action.id);
            }
            if action.keys.len() > keys.len() && action.keys.starts_with(keys) {
                prefix = true;
            }
        }
        if prefix {
            ActionMatch::Prefix
        } else {
            ActionMatch::None
        }
    }
}

/// What a plugin action runs on: handed to its function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionContext {
    /// The diff of `view` (one of its sides), and the change under its cursor.
    Diff { view: ViewId },
    /// The explorer's entry under the cursor.
    Explorer { path: PathBuf, is_dir: bool },
    /// The quickfix panel's entry under the cursor: where it points (lines 0-based,
    /// inclusive), and the text of its row.
    Quickfix {
        target: Target,
        start_line: usize,
        end_line: usize,
        text: String,
    },
}

/// A text a plugin shows on a row, with its color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub plugin: String,
    pub text: String,
    /// `"#rrggbb"`.
    pub fg: Option<String>,
    /// A theme key (`"diagnostic.error"`), when `fg` is not set.
    pub style: Option<String>,
}

/// The marks plugins show at the right of the quickfix panel's rows, by the location the
/// rows point at: a file and a line (0-based).
#[derive(Debug, Default, Clone)]
pub struct QuickfixMarks {
    marks: std::collections::HashMap<(PathBuf, usize), Vec<Mark>>,
}

impl QuickfixMarks {
    /// Sets the mark of `mark.plugin` on `line` of `path` (replacing its mark there).
    pub fn set(&mut self, path: PathBuf, line: usize, mark: Mark) {
        let marks = self.marks.entry((path, line)).or_default();
        marks.retain(|m| m.plugin != mark.plugin);
        marks.push(mark);
    }

    /// Removes the mark of `plugin` on `line` of `path`.
    pub fn remove(&mut self, plugin: &str, path: PathBuf, line: usize) {
        let key = (path, line);
        if let Some(marks) = self.marks.get_mut(&key) {
            marks.retain(|m| m.plugin != plugin);
            if marks.is_empty() {
                self.marks.remove(&key);
            }
        }
    }

    /// Removes the marks of `plugin`.
    pub fn clear_plugin(&mut self, plugin: &str) {
        self.marks.retain(|_, marks| {
            marks.retain(|m| m.plugin != plugin);
            !marks.is_empty()
        });
    }

    pub fn clear(&mut self) {
        self.marks.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The marks of the rows of `line` (0-based) of `path`.
    pub fn get(&self, path: &std::path::Path, line: usize) -> &[Mark] {
        self.marks
            .get(&(path.to_path_buf(), line))
            .map_or(&[], Vec::as_slice)
    }
}

impl Mark {
    /// Its style in `theme` (see [`color_style`]).
    pub fn style(&self, theme: &crate::Theme) -> crate::graphics::Style {
        color_style(theme, self.fg.as_deref(), self.style.as_deref())
    }
}

/// The style of a plugin's text: its `"#rrggbb"` (or `"#rgb"`) color, or its theme key, without the key's
/// background (the row's own stays: a selection, the line's).
pub fn color_style(
    theme: &crate::Theme,
    fg: Option<&str>,
    key: Option<&str>,
) -> crate::graphics::Style {
    use crate::graphics::Style;
    match (
        fg.and_then(|fg| crate::graphics::Color::from_hex(fg).ok()),
        key,
    ) {
        (Some(color), _) => Style::default().fg(color),
        (None, Some(key)) => Style {
            bg: None,
            ..theme.get(key)
        },
        (None, None) => Style::default(),
    }
}

/// A mark a plugin shows on a change of a diff, at the left edge of the connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffMark {
    /// The index of the change in the diff's changes.
    pub change: usize,
    pub mark: Mark,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_macro;

    fn action(plugin: &str, place: ActionPlace, keys: &str, id: u64) -> PluginAction {
        PluginAction {
            plugin: plugin.to_string(),
            place,
            keys: parse_macro(keys).unwrap(),
            description: String::new(),
            id,
        }
    }

    #[test]
    fn actions_match_by_place_and_keys() {
        let mut actions = PluginActions::default();
        let replaced: Option<u64> = actions.add(action("a", ActionPlace::Diff, "<space>e", 1));
        assert_eq!(replaced, None);
        actions.add(action("a", ActionPlace::Explorer, "y", 2));

        let space: Vec<KeyEvent> = parse_macro("<space>").unwrap();
        assert_eq!(
            actions.matching(ActionPlace::Diff, &space),
            ActionMatch::Prefix
        );
        let full: Vec<KeyEvent> = parse_macro("<space>e").unwrap();
        assert_eq!(
            actions.matching(ActionPlace::Diff, &full),
            ActionMatch::Action(1)
        );
        // the keys of another place
        let y: Vec<KeyEvent> = parse_macro("y").unwrap();
        assert_eq!(actions.matching(ActionPlace::Diff, &y), ActionMatch::None);
        assert_eq!(
            actions.matching(ActionPlace::Explorer, &y),
            ActionMatch::Action(2)
        );
        assert_eq!(
            actions.matching(ActionPlace::Explorer, &[]),
            ActionMatch::None
        );

        // the same keys in the same place: replaced, whoever added them
        let replaced: Option<u64> = actions.add(action("b", ActionPlace::Explorer, "y", 3));
        assert_eq!(replaced, Some(2));
        assert_eq!(
            actions.matching(ActionPlace::Explorer, &y),
            ActionMatch::Action(3)
        );
        let count: usize = actions.of(ActionPlace::Explorer).count();
        assert_eq!(count, 1);

        // only the plugin's own
        let removed: Option<u64> = actions.remove("a", ActionPlace::Explorer, &y);
        assert_eq!(removed, None);
        let removed: Option<u64> = actions.remove("b", ActionPlace::Explorer, &y);
        assert_eq!(removed, Some(3));
        let empty: bool = actions.is_empty(ActionPlace::Explorer);
        assert!(empty);
    }

    #[test]
    fn quickfix_marks_are_kept_by_plugin() {
        let mark = |plugin: &str, text: &str| Mark {
            plugin: plugin.to_string(),
            text: text.to_string(),
            fg: None,
            style: None,
        };
        let mut marks = QuickfixMarks::default();
        let path = PathBuf::from("/a.rs");
        marks.set(path.clone(), 3, mark("a", "1"));
        marks.set(path.clone(), 3, mark("b", "2"));
        marks.set(path.clone(), 3, mark("a", "3"));
        let texts: Vec<&str> = marks
            .get(&path, 3)
            .iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(texts, vec!["2", "3"]);
        let other: usize = marks.get(&path, 4).len();
        assert_eq!(other, 0);

        marks.remove("b", path.clone(), 3);
        let texts: Vec<&str> = marks
            .get(&path, 3)
            .iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(texts, vec!["3"]);
        marks.clear_plugin("a");
        let empty: bool = marks.is_empty();
        assert!(empty);
    }
}

/// A completion source of a plugin (`helix.completion.register_source`): its function stays
/// in the plugin's Lua state, by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCompletionSource {
    pub plugin: String,
    pub id: u64,
    pub name: String,
    /// The languages it completes in (language ids); every one when empty.
    pub languages: Vec<String>,
    /// Characters that ask it for completions right away when typed (`.`, `:`).
    pub triggers: Vec<String>,
}

impl PluginCompletionSource {
    /// Whether it is asked in a buffer of `language`, typed `before` the cursor (the text of
    /// the line up to it), asked by hand (`manual`) or not.
    pub fn wanted(&self, language: Option<&str>, before: &str, prefix: &str, manual: bool) -> bool {
        let language_fits = self.languages.is_empty()
            || language.is_some_and(|language| self.languages.iter().any(|l| l == language));
        language_fits
            && (manual
                || !prefix.is_empty()
                || self
                    .triggers
                    .iter()
                    .any(|trigger| before.ends_with(trigger.as_str())))
    }
}

/// What a completion source is asked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCompletionRequest {
    pub document: crate::DocumentId,
    /// The primary cursor: its line (0-based) and column (chars, 0-based).
    pub line: usize,
    pub col: usize,
    /// The word typed before the cursor (what an item replaces).
    pub prefix: String,
    /// The line up to the cursor.
    pub before: String,
    /// Asked by hand (`C-x`), not by typing.
    pub manual: bool,
}

/// An item a completion source gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCompletionItem {
    pub label: String,
    /// What is inserted in place of the prefix (`label` when not set).
    pub insert: Option<String>,
    /// Shown at the right of the menu (the source's name when not set).
    pub kind: Option<String>,
    /// Markdown shown beside the menu.
    pub documentation: Option<String>,
}

/// Asks the plugins' completion sources (set by the application when plugins are loaded):
/// the items of each source asked, with the source's name.
pub type PluginCompletionHook = Box<
    dyn FnMut(
        &mut crate::Editor,
        &PluginCompletionRequest,
    ) -> Vec<(String, Vec<PluginCompletionItem>)>,
>;

#[cfg(test)]
mod completion_source_tests {
    use super::*;

    #[test]
    fn a_source_is_asked_for_its_languages_words_and_triggers() {
        let source = PluginCompletionSource {
            plugin: "p".into(),
            id: 1,
            name: "emoji".into(),
            languages: vec!["markdown".into()],
            triggers: vec![":".into()],
        };
        assert!(source.wanted(Some("markdown"), "hello wor", "wor", false));
        assert!(
            source.wanted(Some("markdown"), "smile :", "", false),
            "a trigger"
        );
        assert!(
            !source.wanted(Some("markdown"), "hello ", "", false),
            "nothing typed"
        );
        assert!(
            source.wanted(Some("markdown"), "hello ", "", true),
            "asked by hand"
        );
        assert!(
            !source.wanted(Some("rust"), "wor", "wor", false),
            "another language"
        );
        assert!(!source.wanted(None, "wor", "wor", true));
        let everywhere = PluginCompletionSource {
            languages: Vec::new(),
            ..source
        };
        let wanted: bool = everywhere.wanted(None, "wor", "wor", false);
        assert!(wanted);
    }
}
