//! Key bindings of the editor's UI components (pickers, menus, popups and prompts).
//!
//! The keys of the editor itself are a [`KeyTrie`](../../helix_term/keymap/enum.KeyTrie.html)
//! per mode. The components handle their keys themselves, so their bindings are not commands but
//! *actions*: a table in `[keys.picker]` (and `menu`, `popup`, `prompt`) maps a key to the name
//! of an action of that component, for example `"C-j" = "move_down"`.
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
}

/// The name of the action that does nothing, to take a key away from a component.
pub const NO_OP: &str = "no_op";

impl Component {
    pub const ALL: [Component; 4] = [
        Component::Picker,
        Component::Menu,
        Component::Popup,
        Component::Prompt,
    ];

    /// The name of the table in the `[keys]` section.
    pub fn name(self) -> &'static str {
        match self {
            Component::Picker => "picker",
            Component::Menu => "menu",
            Component::Popup => "popup",
            Component::Prompt => "prompt",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
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
                ("scroll_preview_up", "A-k"),
                ("scroll_preview_down", "A-j"),
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
                let (_, default_key) = actions
                    .iter()
                    .find(|(name, _)| name == action)
                    .ok_or_else(|| {
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
                Binding::Key(
                    KeyEvent::from_str(default_key).expect("default keys of actions are valid"),
                )
            };
            parsed.push((key_event, binding));
        }
        self.tables.entry(component).or_default().extend(parsed);
        Ok(())
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
