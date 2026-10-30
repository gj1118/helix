use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{
    component_keys::{Component, ComponentKeys},
    document::Mode,
    icons::{Icons, ICONS},
    theme,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use std::sync::Arc;
use toml::de::Error as TomlError;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub editor: helix_view::editor::Config,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<KeysRaw>,
    pub editor: Option<toml::Value>,
    pub icons: Option<toml::Value>,
    /// User-defined command-mode commands.
    pub commands: Option<toml::Value>,
}

/// The `[keys]` section: a key trie per mode (`normal`, `select`, `insert`) and the action
/// tables of the UI components (`picker`, `menu`, `popup`, `prompt`), see
/// [`helix_view::component_keys`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeysRaw {
    pub modes: HashMap<Mode, KeyTrie>,
    pub components: HashMap<Component, HashMap<String, String>>,
}

impl<'de> Deserialize<'de> for KeysRaw {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = KeysRaw;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(
                    f,
                    "a table with the keymaps of the modes and of the picker, menu, popup and prompt"
                )
            }

            fn visit_map<M>(self, mut map: M) -> Result<KeysRaw, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut keys = KeysRaw::default();
                while let Some(name) = map.next_key::<String>()? {
                    if let Some(component) = Component::from_name(&name) {
                        keys.components.insert(component, map.next_value()?);
                    } else {
                        let mode: Mode = name.parse().map_err(|_| {
                            serde::de::Error::custom(format!(
                                "unknown key map `{name}`: expected `normal`, `select`, `insert`, `picker`, `menu`, `popup` or `prompt`"
                            ))
                        })?;
                        keys.modes.insert(mode, map.next_value()?);
                    }
                }
                Ok(keys)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

/// Merges the keys of a config file into the keys so far: the mode keymaps are merged
/// recursively, a component key replaces the binding of that key.
fn merge_raw_keys(
    keys: &mut HashMap<Mode, KeyTrie>,
    components: &mut ComponentKeys,
    raw: Option<KeysRaw>,
) -> Result<(), ConfigLoadError> {
    let Some(raw) = raw else {
        return Ok(());
    };
    merge_keys(keys, raw.modes);
    for (component, table) in &raw.components {
        components.extend(*component, table).map_err(|message| {
            ConfigLoadError::BadConfig(<TomlError as serde::de::Error>::custom(message))
        })?;
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            editor: helix_view::editor::Config::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

/// Parses and checks the `[commands]` tables. A workspace command replaces a
/// global one of the same name.
fn load_commands(
    global: Option<toml::Value>,
    local: Option<toml::Value>,
) -> Result<
    std::collections::BTreeMap<String, helix_view::custom_commands::CustomCommand>,
    ConfigLoadError,
> {
    let bad = |message: String| {
        ConfigLoadError::BadConfig(<TomlError as serde::de::Error>::custom(message))
    };
    let mut commands = std::collections::BTreeMap::new();
    for table in [global, local].into_iter().flatten() {
        commands.extend(helix_view::custom_commands::parse(table).map_err(bad)?);
    }
    for (name, command) in &commands {
        crate::commands::validate_custom_command(command)
            .map_err(|err| bad(format!("custom command `{name}`: {err}")))?;
    }
    Ok(commands)
}

impl Config {
    pub fn load(
        global: Result<&String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        let global_config: Result<ConfigRaw, ConfigLoadError> =
            global.and_then(|file| toml::from_str(file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));

        let res = match (global_config, local_config) {
            (Ok(global), Ok(local)) => {
                let mut keys = keymap::default();
                let mut component_keys = ComponentKeys::default();
                merge_raw_keys(&mut keys, &mut component_keys, global.keys)?;
                merge_raw_keys(&mut keys, &mut component_keys, local.keys)?;

                let editor = match (global.editor, local.editor) {
                    (None, None) => helix_view::editor::Config::default(),
                    (None, Some(val)) | (Some(val), None) => {
                        val.try_into().map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => merge_toml_values(global, local, 3)
                        .try_into()
                        .map_err(ConfigLoadError::BadConfig)?,
                };

                let icons: Icons = match (global.icons, local.icons) {
                    (None, None) => Icons::default(),
                    (None, Some(val)) | (Some(val), None) => {
                        val.try_into().map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => merge_toml_values(global, local, 3)
                        .try_into()
                        .map_err(ConfigLoadError::BadConfig)?,
                };

                ICONS.store(Arc::new(icons));

                let mut editor = editor;
                editor.commands = load_commands(global.commands, local.commands)?;
                editor.component_keys = component_keys;

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    editor,
                }
            }
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                let mut keys = keymap::default();
                let mut component_keys = ComponentKeys::default();
                merge_raw_keys(&mut keys, &mut component_keys, config.keys)?;

                let icons = config.icons.map_or_else(
                    || Ok(Icons::default()),
                    |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                )?;

                ICONS.store(Arc::new(icons));

                let mut editor: helix_view::editor::Config = config.editor.map_or_else(
                    || Ok(helix_view::editor::Config::default()),
                    |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                )?;
                editor.commands = load_commands(config.commands, None)?;
                editor.component_keys = component_keys;

                Config {
                    theme: config.theme,
                    keys,
                    editor,
                }
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        Ok(res)
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let global_config =
            fs::read_to_string(helix_loader::config_file()).map_err(ConfigLoadError::Error)?;
        let local_config = fs::read_to_string(helix_loader::workspace_config_file())
            .map_err(ConfigLoadError::Error);

        let phony_config = ConfigLoadError::Error(IOError::other("hacky placeholder"));
        let global_parsed = Config::load(Ok(&global_config), Err(phony_config))?;

        // We need to build a transient `WorkspaceTrust` just to ask whether the workspace is
        // trusted enough to load its `.helix/config.toml`. The persisted-trust file on disk is the
        // source of truth either way; this transient instance has an empty cache and is dropped
        // after the check.
        let trust = helix_loader::workspace_trust::WorkspaceTrust::new(
            (&global_parsed.editor.workspace_trust).into(),
        );
        if trust
            .query_current(helix_loader::workspace_trust::TrustQuery::LocalConfig)
            .is_trusted()
        {
            let mut merged = Config::load(Ok(&global_config), local_config)?;
            // editor.workspace-trust is global/user-scope only. Without this override, a
            // workspace's `.helix/config.toml` could set `level = "insecure"`; once the user trusted
            // *that* workspace, refresh_config would re-load with the override merged in and from
            // then on every subsequent workspace in the session would be implicitly trusted. Pin
            // the gate's own configuration to the global file.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            Ok(merged)
        } else {
            Ok(global_parsed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Config {
        fn load_test(config: &str) -> Config {
            Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default())).unwrap()
        }
    }

    #[test]
    fn parsing_keymaps_config_file() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Insert => keymap!({ "Insert mode"
                    "y" => move_line_down,
                    "S-C-a" => delete_selection,
                }),
                Mode::Normal => keymap!({ "Normal mode"
                    "A-F12" => move_next_word_end,
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    fn load_err(global: &str, local: &str) -> String {
        match Config::load(Ok(&global.to_owned()), Ok(local.to_owned())) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("the config should be rejected"),
        }
    }

    #[test]
    fn component_keys_are_read_from_the_keys_section() {
        use helix_view::component_keys::Translated;
        use helix_view::input::KeyEvent;

        let key = |s: &str| s.parse::<KeyEvent>().unwrap();
        let config = Config::load_test(
            r#"
            [keys.normal]
            y = "move_line_down"

            [keys.picker]
            C-j = "move_down"
            C-k = "move_up"
            C-o = "no_op"

            [keys.prompt]
            C-o = "delete_word_back"
            "#,
        );
        let keys = &config.editor.component_keys;
        assert_eq!(
            keys.translate(Component::Picker, key("C-j")),
            Translated::Key(key("down"))
        );
        assert_eq!(
            keys.translate(Component::Picker, key("C-o")),
            Translated::Disabled
        );
        assert_eq!(
            keys.translate(Component::Prompt, key("C-o")),
            Translated::Key(key("C-w"))
        );
        assert_eq!(
            keys.translate(Component::Menu, key("C-j")),
            Translated::Unchanged
        );
        // the mode keymaps beside them are still read
        assert_ne!(config.keys, keymap::default());
    }

    #[test]
    fn workspace_component_keys_override_the_global_ones() {
        use helix_view::component_keys::Translated;
        use helix_view::input::KeyEvent;

        let key = |s: &str| s.parse::<KeyEvent>().unwrap();
        let config = Config::load(
            Ok(
                &"[keys.popup]\nC-j = \"scroll_half_page_down\"\nC-n = \"scroll_half_page_down\""
                    .to_owned(),
            ),
            Ok("[keys.popup]\nC-j = \"close\"".to_owned()),
        )
        .unwrap();
        let keys = &config.editor.component_keys;
        assert_eq!(
            keys.translate(Component::Popup, key("C-j")),
            Translated::Key(key("esc"))
        );
        assert_eq!(
            keys.translate(Component::Popup, key("C-n")),
            Translated::Key(key("pagedown"))
        );
    }

    #[test]
    fn bad_component_keys_are_rejected_with_a_message_that_helps() {
        let err = load_err("[keys.picker]\nC-j = \"move_sideways\"", "");
        assert!(err.contains("unknown action `move_sideways`"), "{err}");
        assert!(err.contains("move_down"), "{err}");
        let err = load_err("[keys.menu]\n\"C-wat-\" = \"move_down\"", "");
        assert!(err.contains("invalid key"), "{err}");
        let err = load_err("[keys.dropdown]\nC-j = \"move_down\"", "");
        assert!(err.contains("unknown key map `dropdown`"), "{err}");
    }

    #[test]
    fn keys_resolve_to_correct_defaults() {
        // From serde default
        let default_keys = Config::load_test("").keys;
        assert_eq!(default_keys, keymap::default());

        // From the Default trait
        let default_keys = Config::default().keys;
        assert_eq!(default_keys, keymap::default());
    }

    #[test]
    fn custom_commands_are_loaded_and_validated() {
        let config = Config::load_test(
            r#"
            [commands]
            ":wq" = [":write", ":quit"]
            ":nextline" = "move_line_down"
            ":hints" = ":toggle lsp.display-inlay-hints"
            "0" = ":goto 1"
            [commands.":save-as"]
            commands = [':write "%arg{0}"']
            completer = ":write"
            "#,
        );
        let commands = &config.editor.commands;
        assert_eq!(commands.len(), 5);
        assert_eq!(commands["wq"].commands, [":write", ":quit"]);
        assert!(commands["0"].hidden && !commands["nextline"].hidden);
    }

    fn load_error(config: &str) -> String {
        match Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default())) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected an error for {config}"),
        }
    }

    #[test]
    fn custom_commands_must_name_real_commands() {
        let err = load_error("[commands]\n\":x\" = \":nonsense\"");
        assert!(
            err.contains("`:nonsense` is not a built-in typable command"),
            "{err}"
        );
        let err = load_error("[commands]\n\":x\" = \"not_a_command\"");
        assert!(err.contains("not_a_command"), "{err}");
        let err = load_error("[commands]\n\":x\" = [\"@ihi\", \":write\"]");
        assert!(err.contains("macro must be the only"), "{err}");
        // custom commands cannot call each other
        let err = load_error("[commands]\n\":a\" = \":write\"\n\":b\" = \":a\"");
        assert!(err.contains("`:a` is not a built-in"), "{err}");
        let err = load_error("[commands.\":x\"]\ncommands = [\":write\"]\ncompleter = \":nope\"");
        assert!(err.contains("completer"), "{err}");
    }

    #[test]
    fn workspace_commands_override_global_ones_by_name() {
        let global = "[commands]\n\":a\" = \":write\"\n\":b\" = \":quit\"".to_owned();
        let local = "[commands]\n\":b\" = \":goto 1\"".to_owned();
        let config = Config::load(Ok(&global), Ok(local)).unwrap();
        assert_eq!(config.editor.commands["a"].commands, [":write"]);
        assert_eq!(config.editor.commands["b"].commands, [":goto 1"]);
    }
}
