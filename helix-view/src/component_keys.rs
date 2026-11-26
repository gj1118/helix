//! Key bindings of the editor's UI components (pickers, menus, popups, prompts, the explorer,
//! the quickfix panel, ...).
//!
//! The keys of the editor itself are a [`KeyTrie`](../../helix_term/keymap/enum.KeyTrie.html)
//! per mode. The components handle their keys themselves, so their bindings are not commands but
//! *actions*: a table in `[keys.picker]` (and `menu`, `popup`, `prompt`, `explorer`, ... see
//! [`Component::ALL`]) maps a key to the name of an action of that component, for example
//! `"C-j" = "move_down"`. Every key a component handles is the default key of an action (or
//! does what one does, like `Tab` beside `Down`), so that all of them can be bound elsewhere.
//!
//! An action stands for the key the component has always handled it with. A pressed key that has
//! an entry in the user's table is replaced by that key before the component looks at it, so a
//! component only has to call [`ComponentKeys::translate`] and everything it knew keeps working.
//! The keys of the default bindings stay bound unless the user binds them to another action or to
//! `no_op`.

use crate::input::KeyEvent;
use std::collections::HashMap;
use std::str::FromStr;

/// The components with configurable keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    Picker,
    Menu,
    Popup,
    Prompt,
    /// The explorer's tree (its own keys; moving up and down goes through the normal keymap).
    Explorer,
    /// The explorer's filter box, while it has the keyboard.
    ExplorerFilter,
    /// The list of the quickfix panel (while its query is typed, the keys are the picker's
    /// and the prompt's).
    Quickfix,
    /// The key after the quickfix panel's `window` key.
    QuickfixWindow,
    /// A confirmation box (yes / no).
    Confirm,
    /// The blame details popup.
    Blame,
    /// A floating window of a plugin.
    PluginWindow,
    /// The file explorer picker's own actions.
    FileExplorer,
    /// The changed files picker's own actions.
    ChangedFiles,
    /// The stash picker's own actions.
    Stashes,
    /// The own actions of the git history pickers (the commits, and a commit's files).
    GitHistory,
    /// The quicklist and location list pickers' own actions.
    Quicklist,
}

/// The name of the action that does nothing, to take a key away from a component.
pub const NO_OP: &str = "no_op";

impl Component {
    pub const ALL: [Component; 16] = [
        Component::Picker,
        Component::Menu,
        Component::Popup,
        Component::Prompt,
        Component::Explorer,
        Component::ExplorerFilter,
        Component::Quickfix,
        Component::QuickfixWindow,
        Component::Confirm,
        Component::Blame,
        Component::PluginWindow,
        Component::FileExplorer,
        Component::ChangedFiles,
        Component::Stashes,
        Component::GitHistory,
        Component::Quicklist,
    ];

    /// The name of the table in the `[keys]` section.
    pub fn name(self) -> &'static str {
        match self {
            Component::Picker => "picker",
            Component::Menu => "menu",
            Component::Popup => "popup",
            Component::Prompt => "prompt",
            Component::Explorer => "explorer",
            Component::ExplorerFilter => "explorer-filter",
            Component::Quickfix => "quickfix",
            Component::QuickfixWindow => "quickfix-window",
            Component::Confirm => "confirm",
            Component::Blame => "blame",
            Component::PluginWindow => "plugin-window",
            Component::FileExplorer => "file-explorer",
            Component::ChangedFiles => "changed-files",
            Component::Stashes => "stashes",
            Component::GitHistory => "git-history",
            Component::Quicklist => "quicklist",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The name of an action as the component has it: the explorer's actions were named
    /// without their `explorer_` (`create`, `reveal_buffer`), and those names still work.
    pub fn action_name(self, name: &str) -> Option<&'static str> {
        let actions = self.actions();
        let find = |name: &str| actions.iter().find(|(n, _)| *n == name).map(|(n, _)| *n);
        find(name).or_else(|| match self {
            Component::Explorer if name == "reveal_buffer" => find("explorer_reveal"),
            Component::Explorer if !name.starts_with("explorer_") => {
                find(&format!("explorer_{name}"))
            }
            _ => None,
        })
    }

    /// The key `action` is handled with by default.
    pub fn default_key(self, action: &str) -> Option<KeyEvent> {
        let name = self.action_name(action)?;
        let (_, key) = self.actions().iter().find(|(n, _)| *n == name)?;
        KeyEvent::from_str(key).ok()
    }

    /// The actions of the component, each with the key it is handled with by default.
    pub fn actions(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Component::Picker => &[
                ("move_up", "up"),
                ("move_down", "down"),
                ("half_page_up", "C-u"),
                ("half_page_down", "C-d"),
                ("page_up", "pageup"),
                ("page_down", "pagedown"),
                ("to_start", "home"),
                ("to_end", "end"),
                ("select", "ret"),
                ("select_in_background", "A-ret"),
                ("select_horizontal", "C-s"),
                ("select_vertical", "C-v"),
                ("toggle_preview", "C-t"),
                ("copy_to_quicklist", "C-q"),
                ("copy_to_location_list", "A-q"),
                ("scroll_preview_up", "A-k"),
                ("scroll_preview_down", "A-j"),
                ("toggle_replace", "A-r"),
                ("replace_all", "A-a"),
                ("close", "esc"),
            ],
            Component::Menu => &[
                ("move_up", "up"),
                ("move_down", "down"),
                ("half_page_up", "C-u"),
                ("half_page_down", "C-d"),
                ("select", "ret"),
                ("abort", "esc"),
            ],
            Component::Popup => &[
                ("scroll_half_page_up", "pageup"),
                ("scroll_half_page_down", "pagedown"),
                ("previous", "A-p"),
                ("next", "A-n"),
                ("close", "esc"),
            ],
            Component::Prompt => &[
                ("abort", "esc"),
                ("submit", "ret"),
                ("move_char_back", "left"),
                ("move_char_forward", "right"),
                ("move_word_back", "A-b"),
                ("move_word_forward", "A-f"),
                ("move_start", "home"),
                ("move_end", "end"),
                ("delete_char_back", "backspace"),
                ("delete_char_forward", "del"),
                ("delete_word_back", "C-w"),
                ("delete_word_forward", "A-d"),
                ("kill_to_start", "C-u"),
                ("kill_to_end", "C-k"),
                ("history_prev", "up"),
                ("history_next", "down"),
                ("complete_next", "tab"),
                ("complete_prev", "S-tab"),
                ("insert_word_under_cursor", "C-s"),
                ("insert_register", "C-r"),
                ("exit_selection", "C-q"),
            ],
            // Named as the commands that run them from anywhere (`explorer_create`...).
            Component::Explorer => &[
                ("explorer_open", "l"),
                ("explorer_open_vsplit", "v"),
                ("explorer_open_hsplit", "s"),
                ("explorer_collapse", "h"),
                ("explorer_parent_folder", "-"),
                ("explorer_first_row", "home"),
                ("explorer_last_row", "end"),
                ("explorer_reveal", "."),
                ("explorer_filter", "/"),
                ("explorer_refresh", "R"),
                ("explorer_toggle_hidden", "H"),
                ("explorer_create", "a"),
                ("explorer_create_folder", "A"),
                ("explorer_rename", "r"),
                ("explorer_delete", "d"),
                ("explorer_cut", "x"),
                ("explorer_copy", "c"),
                ("explorer_paste", "p"),
                ("explorer_toggle_mark", "m"),
                ("explorer_mark_range", "M"),
                ("explorer_unmark_all", "u"),
                ("explorer_close", "q"),
                ("explorer_leave", "esc"),
            ],
            Component::ExplorerFilter => &[
                ("accept", "ret"),
                ("next_result", "down"),
                ("prev_result", "up"),
                ("browse_results", "tab"),
                ("clear", "esc"),
                ("move_char_back", "left"),
                ("move_char_forward", "right"),
                ("move_start", "home"),
                ("move_end", "end"),
                ("delete_char_back", "backspace"),
                ("delete_char_forward", "del"),
                ("delete_word_back", "C-w"),
                ("kill_to_start", "C-u"),
            ],
            Component::Quickfix => &[
                ("move_down", "j"),
                ("move_up", "k"),
                ("to_start", "g"),
                ("to_end", "G"),
                ("open_and_stay", "o"),
                ("filter", "/"),
                ("window", "C-w"),
                ("editor_key", ":"),
                ("leave", "esc"),
                ("close", "q"),
            ],
            Component::QuickfixWindow => &[("up", "k"), ("left", "h"), ("right", "l")],
            Component::Confirm => &[
                ("confirm", "y"),
                ("cancel", "n"),
                ("accept", "ret"),
                ("switch", "tab"),
            ],
            Component::Blame => &[("copy_commit", "y"), ("copy_message", "Y")],
            Component::PluginWindow => &[
                ("move_down", "j"),
                ("move_up", "k"),
                ("half_page_down", "C-d"),
                ("half_page_up", "C-u"),
                ("page_down", "pagedown"),
                ("page_up", "pageup"),
                ("to_start", "g"),
                ("to_end", "G"),
                ("close", "q"),
            ],
            Component::FileExplorer => &[
                ("create", "A-n"),
                ("move", "A-m"),
                ("delete", "A-d"),
                ("copy", "A-c"),
                ("yank_path", "A-y"),
            ],
            Component::ChangedFiles => &[
                ("stage", "A-s"),
                ("unstage", "A-u"),
                ("stash", "A-z"),
                ("stashes", "A-l"),
                ("commit", "A-c"),
            ],
            Component::Stashes => &[("apply", "A-s"), ("apply_with_index", "A-i")],
            Component::GitHistory => &[
                ("yank_commit", "A-y"),
                ("open_version", "A-o"),
                ("back_to_commits", "A-h"),
            ],
            Component::Quicklist => &[("remove", "C-x")],
        }
    }
}

/// What a component does with a key it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Translated {
    /// No binding: handle the key as it is.
    Unchanged,
    /// Handle this key instead.
    Key(KeyEvent),
    /// The key is bound to [`NO_OP`].
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    Key(KeyEvent),
    Disabled,
}

/// The user's key bindings of the components.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentKeys {
    tables: HashMap<Component, HashMap<KeyEvent, Binding>>,
}

impl ComponentKeys {
    pub fn is_empty(&self) -> bool {
        self.tables.values().all(HashMap::is_empty)
    }

    /// Adds the bindings of `table` (key → action name) of a component. Bindings of a key that is
    /// already bound are replaced, this is how a workspace overrides the user config.
    pub fn extend(
        &mut self,
        component: Component,
        table: &HashMap<String, String>,
    ) -> Result<(), String> {
        let actions = component.actions();
        let mut parsed = Vec::with_capacity(table.len());
        for (key, action) in table {
            let key_event = KeyEvent::from_str(key)
                .map_err(|err| format!("keys.{}: invalid key `{key}`: {err}", component.name()))?;
            let binding = if action == NO_OP {
                Binding::Disabled
            } else {
                let default_key = component.default_key(action).ok_or_else(|| {
                    let names = actions
                        .iter()
                        .map(|(name, _)| *name)
                        .chain([NO_OP])
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "keys.{}: unknown action `{action}` for `{key}` (the actions are: {names})",
                        component.name()
                    )
                })?;
                Binding::Key(default_key)
            };
            parsed.push((key_event, binding));
        }
        self.tables.entry(component).or_default().extend(parsed);
        Ok(())
    }

    /// The key to show for `action` of `component`: the first (in name order) of the keys the
    /// user bound to it, else its default key, unless that key was given to something else.
    pub fn key_for(&self, component: Component, action: &str) -> Option<KeyEvent> {
        let default = component.default_key(action)?;
        let table = self.tables.get(&component);
        let bound = table.into_iter().flatten().filter_map(|(key, binding)| {
            (*binding == Binding::Key(default) && *key != default).then_some(*key)
        });
        if let Some(key) = bound.min_by_key(|key| key.to_string()) {
            return Some(key);
        }
        match table.and_then(|table| table.get(&default)) {
            None => Some(default),
            Some(Binding::Key(key)) if *key == default => Some(default),
            Some(_) => None,
        }
    }

    pub fn translate(&self, component: Component, key: KeyEvent) -> Translated {
        match self
            .tables
            .get(&component)
            .and_then(|table| table.get(&key))
        {
            None => Translated::Unchanged,
            Some(Binding::Key(key)) => Translated::Key(*key),
            Some(Binding::Disabled) => Translated::Disabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn key(s: &str) -> KeyEvent {
        s.parse().unwrap()
    }

    #[test]
    fn the_name_of_every_component_reads_back() {
        let mut names = std::collections::HashSet::new();
        for component in Component::ALL {
            assert_eq!(Component::from_name(component.name()), Some(component));
            assert!(names.insert(component.name()), "{}", component.name());
            assert!(!component.actions().is_empty(), "{}", component.name());
        }
        assert_eq!(Component::from_name("pickers"), None);
    }

    /// An action stands for its default key: two actions of a component with one key
    /// would be one action.
    #[test]
    fn no_two_actions_of_a_component_share_a_key_or_a_name() {
        for component in Component::ALL {
            let mut keys = std::collections::HashSet::new();
            let mut names = std::collections::HashSet::new();
            for (name, default) in component.actions() {
                assert!(keys.insert(key(default)), "{}: {default}", component.name());
                assert!(names.insert(name), "{}: {name}", component.name());
                assert_ne!(*name, NO_OP);
            }
        }
    }

    /// The keys the components handled with nothing to bind them by, once.
    #[test]
    fn the_keys_of_every_component_have_an_action() {
        let default = |component: Component, action: &str| {
            ComponentKeys::default()
                .key_for(component, action)
                .unwrap_or_else(|| panic!("{}: no action {action}", component.name()))
        };
        assert_eq!(default(Component::Picker, "toggle_replace"), key("A-r"));
        assert_eq!(default(Component::Picker, "replace_all"), key("A-a"));
        assert_eq!(default(Component::Popup, "previous"), key("A-p"));
        assert_eq!(default(Component::Popup, "next"), key("A-n"));
        assert_eq!(default(Component::Quickfix, "open_and_stay"), key("o"));
        assert_eq!(default(Component::Quickfix, "window"), key("C-w"));
        assert_eq!(default(Component::Quickfix, "editor_key"), key(":"));
        assert_eq!(default(Component::QuickfixWindow, "up"), key("k"));
        assert_eq!(default(Component::Confirm, "confirm"), key("y"));
        assert_eq!(default(Component::Blame, "copy_message"), key("Y"));
        assert_eq!(default(Component::PluginWindow, "close"), key("q"));
        assert_eq!(default(Component::FileExplorer, "delete"), key("A-d"));
        assert_eq!(default(Component::ChangedFiles, "stage"), key("A-s"));
        assert_eq!(default(Component::Stashes, "apply_with_index"), key("A-i"));
        assert_eq!(default(Component::GitHistory, "open_version"), key("A-o"));
        assert_eq!(default(Component::Quicklist, "remove"), key("C-x"));

        // the same key is another action in another table
        let mut keys = ComponentKeys::default();
        keys.extend(Component::ChangedFiles, &table(&[("C-a", "stage")]))
            .unwrap();
        keys.extend(Component::Stashes, &table(&[("C-a", "apply_with_index")]))
            .unwrap();
        assert_eq!(
            keys.translate(Component::ChangedFiles, key("C-a")),
            Translated::Key(key("A-s"))
        );
        assert_eq!(
            keys.translate(Component::Stashes, key("C-a")),
            Translated::Key(key("A-i"))
        );
        assert_eq!(
            keys.translate(Component::Picker, key("C-a")),
            Translated::Unchanged
        );
        assert_eq!(
            keys.key_for(Component::ChangedFiles, "stage"),
            Some(key("C-a"))
        );
    }

    #[test]
    fn every_default_key_of_every_action_parses() {
        for component in Component::ALL {
            for (name, default) in component.actions() {
                assert!(default.parse::<KeyEvent>().is_ok(), "{name}: {default}");
            }
        }
    }

    #[test]
    fn a_bound_key_becomes_the_key_of_its_action() {
        let mut keys = ComponentKeys::default();
        keys.extend(
            Component::Picker,
            &table(&[("C-j", "move_down"), ("C-o", "no_op")]),
        )
        .unwrap();
        assert_eq!(
            keys.translate(Component::Picker, key("C-j")),
            Translated::Key(key("down"))
        );
        assert_eq!(
            keys.translate(Component::Picker, key("C-o")),
            Translated::Disabled
        );
        // other keys and other components are not affected
        assert_eq!(
            keys.translate(Component::Picker, key("C-n")),
            Translated::Unchanged
        );
        assert_eq!(
            keys.translate(Component::Prompt, key("C-j")),
            Translated::Unchanged
        );
    }

    #[test]
    fn mistakes_are_reported_with_what_would_have_been_valid() {
        let mut keys = ComponentKeys::default();
        let err = keys
            .extend(Component::Menu, &table(&[("C-j", "jump_around")]))
            .unwrap_err();
        assert!(err.contains("unknown action `jump_around`"), "{err}");
        assert!(err.contains("move_down"), "{err}");
        let err = keys
            .extend(Component::Menu, &table(&[("C-nonsense-", "move_down")]))
            .unwrap_err();
        assert!(err.contains("invalid key"), "{err}");
        assert!(keys.is_empty());
    }

    #[test]
    fn the_explorer_tables_have_their_own_names_and_actions() {
        assert_eq!(Component::from_name("explorer"), Some(Component::Explorer));
        assert_eq!(
            Component::from_name("explorer-filter"),
            Some(Component::ExplorerFilter)
        );
        let mut keys = ComponentKeys::default();
        keys.extend(
            Component::Explorer,
            &table(&[
                ("space", "explorer_toggle_mark"),
                ("D", "explorer_delete"),
                ("m", "no_op"),
            ]),
        )
        .unwrap();
        keys.extend(Component::ExplorerFilter, &table(&[("C-j", "next_result")]))
            .unwrap();
        assert_eq!(
            keys.translate(Component::Explorer, key("space")),
            Translated::Key(key("m"))
        );
        assert_eq!(
            keys.translate(Component::Explorer, key("D")),
            Translated::Key(key("d"))
        );
        assert_eq!(
            keys.translate(Component::Explorer, key("m")),
            Translated::Disabled
        );
        assert_eq!(
            keys.translate(Component::ExplorerFilter, key("C-j")),
            Translated::Key(key("down"))
        );
        // the tree's table is not the filter box's
        assert_eq!(
            keys.translate(Component::ExplorerFilter, key("space")),
            Translated::Unchanged
        );
        // what to show for an action: the user's key, else the default one, unless taken away
        assert_eq!(
            keys.key_for(Component::Explorer, "explorer_delete"),
            Some(key("D"))
        );
        assert_eq!(
            keys.key_for(Component::Explorer, "explorer_rename"),
            Some(key("r"))
        );
        assert_eq!(
            keys.key_for(Component::Explorer, "explorer_toggle_mark"),
            Some(key("space"))
        );
        keys.extend(Component::Explorer, &table(&[("u", "no_op")]))
            .unwrap();
        assert_eq!(
            keys.key_for(Component::Explorer, "explorer_unmark_all"),
            None
        );
        assert_eq!(keys.key_for(Component::Explorer, "jump_around"), None);
        // an action of the filter box is not one of the tree
        let err = keys
            .extend(Component::Explorer, &table(&[("C-j", "next_result")]))
            .unwrap_err();
        assert!(err.contains("keys.explorer: unknown action"), "{err}");
        assert!(err.contains("explorer_toggle_mark"), "{err}");
    }

    /// The explorer's actions are named as their commands (`explorer_create`); the names they
    /// had (`create`, `reveal_buffer`) still work.
    #[test]
    fn the_explorer_actions_keep_their_old_names() {
        for (name, _) in Component::Explorer.actions() {
            assert!(name.starts_with("explorer_"), "{name}");
        }
        let explorer = Component::Explorer;
        assert_eq!(explorer.action_name("create"), Some("explorer_create"));
        assert_eq!(
            explorer.action_name("explorer_create"),
            Some("explorer_create")
        );
        assert_eq!(
            explorer.action_name("reveal_buffer"),
            Some("explorer_reveal")
        );
        assert_eq!(explorer.action_name("jump_around"), None);
        // only the explorer's: `create` is the file explorer picker's own name
        assert_eq!(Component::Picker.action_name("explorer_close"), None);
        assert_eq!(
            Component::FileExplorer.action_name("create"),
            Some("create")
        );
        assert_eq!(explorer.default_key("paste"), Some(key("p")));
        assert_eq!(explorer.default_key("explorer_last_row"), Some(key("end")));

        let mut keys = ComponentKeys::default();
        keys.extend(
            explorer,
            &table(&[("space", "toggle_mark"), ("C-r", "reveal_buffer")]),
        )
        .unwrap();
        assert_eq!(
            keys.translate(explorer, key("space")),
            Translated::Key(key("m"))
        );
        assert_eq!(
            keys.translate(explorer, key("C-r")),
            Translated::Key(key("."))
        );
        assert_eq!(
            keys.key_for(explorer, "explorer_toggle_mark"),
            Some(key("space"))
        );
        assert_eq!(keys.key_for(explorer, "toggle_mark"), Some(key("space")));
    }

    #[test]
    fn later_tables_override_earlier_ones() {
        let mut keys = ComponentKeys::default();
        keys.extend(
            Component::Popup,
            &table(&[("C-j", "scroll_half_page_down")]),
        )
        .unwrap();
        keys.extend(Component::Popup, &table(&[("C-j", "close")]))
            .unwrap();
        assert_eq!(
            keys.translate(Component::Popup, key("C-j")),
            Translated::Key(key("esc"))
        );
    }
}
