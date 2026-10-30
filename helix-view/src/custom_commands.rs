//! User-defined command-mode commands (the `[commands]` table).
//!
//! This only parses and stores them. Checking that the commands they run
//! exist, and running them, happens in `helix-term`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// What a custom command does, with its optional help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    /// Typable commands (`:write`), static commands (`move_char_right`) or a
    /// single key macro (`@miw`), run in order.
    pub commands: Vec<String>,
    /// Markdown shown in the command prompt.
    pub desc: Option<String>,
    /// Placeholder for the arguments, shown in the command prompt.
    pub accepts: Option<String>,
    /// A built-in typable command whose argument completion is reused.
    pub completer: Option<String>,
    /// Hidden commands are usable but missing from completion.
    pub hidden: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Raw {
    One(String),
    Many(Vec<String>),
    Detailed(Detailed),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Detailed {
    commands: Vec<String>,
    desc: Option<String>,
    accepts: Option<String>,
    completer: Option<String>,
}

/// Parses a `[commands]` table. Keys starting with `:` are listed in
/// completion; other keys are hidden. The `:` is not part of the stored name.
pub fn parse(table: toml::Value) -> Result<BTreeMap<String, CustomCommand>, String> {
    let toml::Value::Table(table) = table else {
        return Err("`commands` must be a table".into());
    };
    let mut commands = BTreeMap::new();
    for (key, value) in table {
        let (name, hidden) = match key.strip_prefix(':') {
            Some(name) => (name, false),
            None => (key.as_str(), true),
        };
        if name.is_empty() || name.chars().any(char::is_whitespace) || name.starts_with('^') {
            return Err(format!(
                "invalid custom command name `{key}`: it must be a non-empty word and cannot start with `^`"
            ));
        }
        let raw: Raw = value
            .try_into()
            .map_err(|err: toml::de::Error| format!("custom command `{key}`: {err}"))?;
        let (commands_list, desc, accepts, completer) = match raw {
            Raw::One(command) => (vec![command], None, None, None),
            Raw::Many(commands) => (commands, None, None, None),
            Raw::Detailed(d) => (d.commands, d.desc, d.accepts, d.completer),
        };
        if commands_list.is_empty() {
            return Err(format!("custom command `{key}` has no commands"));
        }
        commands.insert(
            name.to_owned(),
            CustomCommand {
                commands: commands_list,
                desc,
                accepts,
                completer,
                hidden,
            },
        );
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(toml: &str) -> Result<BTreeMap<String, CustomCommand>, String> {
        parse(toml::from_str(toml).unwrap())
    }

    #[test]
    fn accepts_a_string_an_array_and_a_table() {
        let commands = parse_str(
            r#"
            ":f" = ":format"
            ":wq" = [":write", ":quit"]
            "0" = ":goto 1"
            [":save-as"]
            commands = [':write "%arg{0}"']
            desc = "Save to a path"
            accepts = "<path>"
            completer = ":write"
            "#,
        )
        .unwrap();
        assert_eq!(commands["f"].commands, [":format"]);
        assert!(!commands["f"].hidden);
        assert_eq!(commands["wq"].commands, [":write", ":quit"]);
        // no `:` prefix: usable but hidden
        assert!(commands["0"].hidden);
        let save_as = &commands["save-as"];
        assert_eq!(save_as.desc.as_deref(), Some("Save to a path"));
        assert_eq!(save_as.accepts.as_deref(), Some("<path>"));
        assert_eq!(save_as.completer.as_deref(), Some(":write"));
    }

    #[test]
    fn rejects_bad_definitions() {
        assert!(parse_str("\":\" = \":write\"").is_err());
        assert!(parse_str("\":a b\" = \":write\"").is_err());
        assert!(parse_str("\":^w\" = \":write\"").is_err());
        assert!(parse_str("\":x\" = []").is_err());
        assert!(parse_str("\":x\" = 5").is_err());
        // unknown fields in the detailed form are mistakes, not silently ignored
        assert!(parse_str("[\":x\"]\ncommands = [\":write\"]\ndescription = \"typo\"").is_err());
        assert!(parse(toml::Value::Integer(1)).is_err());
    }
}
