//! What a plugin may do.
//!
//! Every plugin runs in a Lua state of its own, which carries its [`Permissions`]: an API
//! function checks the permissions of the state it was called from ([`require`]), so a
//! plugin can neither use another plugin's permissions nor change what another plugin's
//! code calls (there is no shared Lua table to tamper with).
//!
//! A plugin gets [`Permissions::DEFAULT`] (`read` and `ui`, which only look at the editor
//! and show things) and what its `[[plugins.plugins]]` entry in `config.toml` grants. A
//! plugin that lists `permissions` in its `plugin.toml` gets no more than it lists there:
//! the manifest says what a plugin needs, the user decides what it gets.

use mlua::prelude::*;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// One kind of access to the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Permission {
    /// Read the editor's state: buffers, windows, selections, diagnostics, registers,
    /// settings; and hear of the editor's events. Granted by default.
    Read,
    /// Show things: messages, prompts, pickers, annotations, panels, the theme. Granted by
    /// default.
    Ui,
    /// Change buffers: their text, selections and cursors, the mode, registers, undo/redo.
    Edit,
    /// Open, close, split and focus windows; open files into buffers; quit.
    Windows,
    /// Write buffers to disk, and other changes of files (rename, delete).
    Files,
    /// Register `:` commands, and run Helix's own `:` commands.
    Commands,
    /// Change the editor's settings (`:set`, `:config-reload`, the language of a buffer...).
    Config,
    /// Change key bindings.
    Keymaps,
    /// Control the file explorer.
    Explorer,
    /// Run programs: shell commands (`:sh`, `:pipe`...), debug adapters, and the settings
    /// that run programs (`shell`, `clipboard-provider`, workspace trust).
    Shell,
}

impl Permission {
    pub const ALL: [Permission; 10] = [
        Permission::Read,
        Permission::Ui,
        Permission::Edit,
        Permission::Windows,
        Permission::Files,
        Permission::Commands,
        Permission::Config,
        Permission::Keymaps,
        Permission::Explorer,
        Permission::Shell,
    ];

    /// The name used in `plugin.toml` and `config.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Ui => "ui",
            Permission::Edit => "edit",
            Permission::Windows => "windows",
            Permission::Files => "files",
            Permission::Commands => "commands",
            Permission::Config => "config",
            Permission::Keymaps => "keymaps",
            Permission::Explorer => "explorer",
            Permission::Shell => "shell",
        }
    }

    fn bit(self) -> u16 {
        1 << self as u8
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Permission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Permission::ALL
            .into_iter()
            .find(|permission| permission.as_str() == s)
            .ok_or_else(|| {
                let names: Vec<_> = Permission::ALL.iter().map(|p| p.as_str()).collect();
                format!(
                    "unknown plugin permission '{s}' (expected one of {}, or \"all\")",
                    names.join(", ")
                )
            })
    }
}

/// A set of [`Permission`]s.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions(u16);

impl Permissions {
    /// What every plugin may do unless its manifest leaves it out: look and show.
    pub const DEFAULT: Permissions =
        Permissions((1 << Permission::Read as u8) | (1 << Permission::Ui as u8));
    pub const NONE: Permissions = Permissions(0);

    pub fn all() -> Self {
        Permission::ALL.into_iter().collect()
    }

    pub fn contains(self, permission: Permission) -> bool {
        self.0 & permission.bit() != 0
    }

    pub fn insert(&mut self, permission: Permission) {
        self.0 |= permission.bit();
    }

    pub fn union(self, other: Permissions) -> Permissions {
        Permissions(self.0 | other.0)
    }

    pub fn intersection(self, other: Permissions) -> Permissions {
        Permissions(self.0 & other.0)
    }

    /// The permissions in `self` but not in `other`.
    pub fn difference(self, other: Permissions) -> Permissions {
        Permissions(self.0 & !other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Permission> {
        Permission::ALL
            .into_iter()
            .filter(move |permission| self.contains(*permission))
    }

    /// What a plugin gets: the defaults and the grants of `config.toml`, no more than its
    /// manifest requests when it lists any.
    pub fn effective(requested: Option<Permissions>, granted: Permissions) -> Permissions {
        let allowed = Permissions::DEFAULT.union(granted);
        match requested {
            Some(requested) => allowed.intersection(requested),
            None => allowed,
        }
    }
}

impl FromIterator<Permission> for Permissions {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        let mut permissions = Permissions::NONE;
        for permission in iter {
            permissions.insert(permission);
        }
        permissions
    }
}

impl fmt::Debug for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<_> = self.iter().map(Permission::as_str).collect();
        f.write_str(&names.join(", "))
    }
}

/// `permissions = ["edit", "files"]` (or `["all"]`) of a `[[plugins.plugins]]` entry: an
/// unknown name is a config error.
impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        let mut permissions = Permissions::NONE;
        for name in names {
            if name == "all" {
                permissions = permissions.union(Permissions::all());
            } else {
                permissions.insert(name.parse().map_err(de::Error::custom)?);
            }
        }
        Ok(permissions)
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Permission::as_str))
    }
}

/// The permissions a manifest (`plugin.toml`) lists. A name this Helix does not know (from
/// a newer one, say) is left out with a warning: it grants nothing here anyway.
pub fn parse_requested(plugin: &str, names: &[String]) -> Permissions {
    let mut permissions = Permissions::NONE;
    for name in names {
        if name == "all" {
            permissions = permissions.union(Permissions::all());
            continue;
        }
        match name.parse() {
            Ok(permission) => permissions.insert(permission),
            Err(err) => log::warn!("plugin '{plugin}': {err}"),
        }
    }
    permissions
}

/// The identity of the plugin a Lua state belongs to, kept in the state's app data.
#[derive(Debug, Clone)]
pub struct PluginIdentity {
    pub name: String,
    pub permissions: Permissions,
}

/// Fails unless the plugin whose Lua state calls has `permission`. `what` names the
/// function in the error (`"helix.editor.save"`).
pub fn require(lua: &Lua, permission: Permission, what: &str) -> LuaResult<()> {
    let identity = lua.app_data_ref::<PluginIdentity>();
    let Some(identity) = identity else {
        return Err(LuaError::RuntimeError(format!(
            "{what} is not available: the plugin's permissions are unknown"
        )));
    };
    if identity.permissions.contains(permission) {
        return Ok(());
    }
    Err(LuaError::RuntimeError(denied(
        &identity.name,
        &[permission],
        what,
    )))
}

/// Fails unless the calling plugin has every permission of `permissions`.
pub fn require_all(lua: &Lua, permissions: &[Permission], what: &str) -> LuaResult<()> {
    let identity = lua.app_data_ref::<PluginIdentity>();
    let Some(identity) = identity else {
        return Err(LuaError::RuntimeError(format!(
            "{what} is not available: the plugin's permissions are unknown"
        )));
    };
    let missing: Vec<Permission> = permissions
        .iter()
        .copied()
        .filter(|permission| !identity.permissions.contains(*permission))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(LuaError::RuntimeError(denied(
        &identity.name,
        &missing,
        what,
    )))
}

/// The error message of a refused call: what is missing and how to grant it.
fn denied(plugin: &str, missing: &[Permission], what: &str) -> String {
    let names: Vec<String> = missing.iter().map(|p| format!("\"{p}\"")).collect();
    format!(
        "plugin '{plugin}' may not use {what}: it needs the {} permission{} (grant with \
         permissions = [{}] in its [[plugins.plugins]] entry of config.toml, and list {} in \
         its plugin.toml if that has permissions)",
        missing
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(" and "),
        if missing.len() > 1 { "s" } else { "" },
        names.join(", "),
        if missing.len() > 1 { "them" } else { "it" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for permission in Permission::ALL {
            let parsed: Permission = permission.as_str().parse().unwrap();
            assert_eq!(parsed, permission);
        }
        let unknown: Result<Permission, String> = "network".parse();
        assert!(unknown
            .unwrap_err()
            .contains("unknown plugin permission 'network'"));
    }

    #[test]
    fn effective_permissions() {
        let granted: Permissions = [Permission::Edit, Permission::Shell].into_iter().collect();
        // no manifest list: defaults and grants
        let all = Permissions::effective(None, granted);
        assert_eq!(
            all.iter().collect::<Vec<_>>(),
            [
                Permission::Read,
                Permission::Ui,
                Permission::Edit,
                Permission::Shell
            ]
        );
        // a manifest narrows: shell is granted but not requested, files requested but not
        // granted
        let requested: Permissions = [Permission::Read, Permission::Edit, Permission::Files]
            .into_iter()
            .collect();
        let narrowed: Permissions = Permissions::effective(Some(requested), granted);
        assert_eq!(
            narrowed.iter().collect::<Vec<_>>(),
            [Permission::Read, Permission::Edit]
        );
        // nothing granted: the defaults
        let defaults: Permissions = Permissions::effective(None, Permissions::NONE);
        assert_eq!(defaults, Permissions::DEFAULT);
    }

    #[test]
    fn config_grants_parse_with_all_and_reject_unknown_names() {
        #[derive(Deserialize)]
        struct Entry {
            permissions: Permissions,
        }
        let entry: Entry = toml::from_str(r#"permissions = ["edit", "files"]"#).unwrap();
        assert_eq!(entry.permissions.to_string(), "edit, files");
        let entry: Entry = toml::from_str(r#"permissions = ["all"]"#).unwrap();
        assert_eq!(entry.permissions, Permissions::all());
        let error = toml::from_str::<Entry>(r#"permissions = ["edit", "network"]"#)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            error.contains("unknown plugin permission 'network'"),
            "{error}"
        );
    }

    #[test]
    fn manifest_names_unknown_here_are_left_out() {
        let names = ["edit".to_string(), "telepathy".to_string()];
        let requested: Permissions = parse_requested("p", &names);
        assert_eq!(requested.iter().collect::<Vec<_>>(), [Permission::Edit]);
    }

    #[test]
    fn calls_are_checked_against_the_state_s_plugin() {
        let lua = Lua::new();
        let unknown = require(&lua, Permission::Read, "helix.x");
        assert!(
            unknown.is_err(),
            "a state without an identity may do nothing"
        );
        lua.set_app_data(PluginIdentity {
            name: "p".into(),
            permissions: Permissions::DEFAULT,
        });
        let read: LuaResult<()> = require(&lua, Permission::Read, "helix.x");
        assert!(read.is_ok());
        let error = require(&lua, Permission::Files, "helix.editor.save")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("plugin 'p' may not use helix.editor.save")
                && error.contains("'files' permission")
                && error.contains(r#"permissions = ["files"]"#),
            "{error}"
        );
        let error = require_all(&lua, &[Permission::Commands, Permission::Shell], ":sh")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("'commands' and 'shell' permissions"),
            "{error}"
        );
    }
}
