use crate::compositor::Context;
use crate::job::{Job, Jobs};
use crate::ui::PromptEvent;
use helix_core::command_line::Args;
use helix_plugin::types::{EditorCommandRegistry, UiHandler};
use helix_plugin::{Permission, PluginManager};
use helix_view::Editor;
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub enum UiRequest {
    Prompt {
        message: String,
        default: Option<String>,
        plugin_name: String,
        callback_id: u64,
    },
    Confirm {
        message: String,
        plugin_name: String,
        callback_id: u64,
    },
    Picker {
        items: Vec<String>,
        prompt: String,
        plugin_name: String,
        callback_id: u64,
    },
}

pub struct TermUiHandler {
    sender: UnboundedSender<UiRequest>,
}

impl UiHandler for TermUiHandler {
    fn prompt(
        &self,
        _editor: &mut Editor,
        message: String,
        default: Option<String>,
        plugin_name: String,
        callback_id: u64,
    ) {
        let _ = self.sender.send(UiRequest::Prompt {
            message,
            default,
            plugin_name,
            callback_id,
        });
    }

    fn confirm(
        &self,
        _editor: &mut Editor,
        message: String,
        plugin_name: String,
        callback_id: u64,
    ) {
        let _ = self.sender.send(UiRequest::Confirm {
            message,
            plugin_name,
            callback_id,
        });
    }

    fn picker(
        &self,
        _editor: &mut Editor,
        items: Vec<String>,
        prompt: String,
        plugin_name: String,
        callback_id: u64,
    ) {
        let _ = self.sender.send(UiRequest::Picker {
            items,
            prompt,
            plugin_name,
            callback_id,
        });
    }
}

pub struct TermCommandRegistry {}

impl EditorCommandRegistry for TermCommandRegistry {
    fn execute(
        &self,
        editor: &mut Editor,
        name: &str,
        args: &[String],
    ) -> std::result::Result<(), anyhow::Error> {
        // Find the command in TYPABLE_COMMAND_LIST
        let cmd = crate::commands::typed::TYPABLE_COMMAND_LIST
            .iter()
            .find(|c| c.name == name || c.aliases.contains(&name))
            .ok_or_else(|| anyhow::anyhow!("Command not found: {}", name))?;

        // We need a Context. Let's try to create a minimal one.
        let mut jobs = Jobs::new();
        let mut cx = Context {
            editor,
            scroll: None,
            jobs: &mut jobs,
            plugin_manager: None,
        };

        let line = command_line(args, cmd.signature.raw_after);
        let args_struct = Args::parse(&line, cmd.signature, true, |token| Ok(token.content))
            .map_err(|e| anyhow::anyhow!("Failed to parse arguments: {}", e))?;

        let result = (cmd.fun)(&mut cx, args_struct, PromptEvent::Validate);

        // Plain jobs were already spawned by `Jobs::add` and report back through the
        // global job queue. Jobs that the command wanted awaited (e.g. the
        // format-then-save callbacks of `:write`) would be dropped together with this
        // temporary `Jobs`, so spawn them as well. Their callbacks reach the
        // application via the global job queue; they are just not awaited on exit.
        for future in std::mem::take(&mut jobs.wait_futures) {
            jobs.add(Job {
                future,
                wait: false,
            });
        }

        result?;
        Ok(())
    }

    fn permissions(&self, name: &str, args: &[String]) -> Option<Vec<Permission>> {
        let cmd = crate::commands::typed::TYPABLE_COMMAND_LIST
            .iter()
            .find(|c| c.name == name || c.aliases.contains(&name))?;
        // A command nobody classified (a new one) is treated like the most powerful kind.
        let mut needed = typable_permissions(cmd.name)
            .map(<[Permission]>::to_vec)
            .unwrap_or_else(|| vec![Permission::Shell]);
        // Settings that run programs.
        if matches!(cmd.name, "set-option" | "toggle-option")
            && args.first().is_some_and(|key| {
                let key = key.trim_start_matches("editor.");
                key == "shell" || key.starts_with("clipboard-provider")
            })
        {
            needed.push(Permission::Shell);
        }
        Some(needed)
    }
}

/// What running a typable command needs (see `EditorCommandRegistry::permissions`). Every
/// command is listed: a test fails for one that is not.
fn typable_permissions(name: &str) -> Option<&'static [Permission]> {
    use Permission::*;
    Some(match name {
        // look and show
        "show-clipboard-provider"
        | "show-directory"
        | "character-info"
        | "blame"
        | "tree-sitter-scopes"
        | "tree-sitter-highlight-name"
        | "tree-sitter-layers"
        | "tree-sitter-subtree"
        | "get-option"
        | "theme"
        | "echo"
        | "redraw"
        | "noop"
        | "file-history"
        | "git-history"
        | "index"
        | "reload-history"
        | "notifications-history"
        | "notifications-clear"
        | "notifications-dismiss"
        | "notifications-test" => &[Ui],
        // change buffers
        "format"
        | "indent-style"
        | "earlier"
        | "later"
        | "yank-join"
        | "clipboard-yank"
        | "primary-clipboard-yank"
        | "clipboard-paste-after"
        | "clipboard-paste-before"
        | "clipboard-paste-replace"
        | "primary-clipboard-paste-after"
        | "primary-clipboard-paste-before"
        | "primary-clipboard-paste-replace"
        | "encoding"
        | "restore-change"
        | "reload"
        | "reload-all"
        | "reset-diff-change"
        | "clear-register"
        | "set-register"
        | "sort"
        | "reflow"
        | "snippet-insert"
        | "read"
        | "fold"
        | "unfold"
        | "yank-diagnostic"
        | "goto"
        | "line-ending"
        | "clipboard-yank-join"
        | "primary-clipboard-yank-join" => &[Edit],
        // windows and buffers
        "quit"
        | "quit!"
        | "open"
        | "buffer-close"
        | "buffer-close!"
        | "buffer-close-others"
        | "buffer-close-others!"
        | "buffer-close-all"
        | "buffer-close-all!"
        | "buffer-next"
        | "buffer-previous"
        | "new"
        | "vsplit"
        | "vsplit-new"
        | "hsplit"
        | "hsplit-new"
        | "tutor"
        | "config-open"
        | "config-open-workspace"
        | "snippets-open"
        | "snippets-open-workspace"
        | "log-open"
        | "copen"
        | "cclose"
        | "quit-all"
        | "quit-all!"
        | "cquit"
        | "cquit!" => &[Windows],
        // writing files
        "write" | "write!" | "write-all" | "write-all!" | "update" | "move" | "move!" => &[Files],
        "exit"
        | "exit!"
        | "write-buffer-close"
        | "write-buffer-close!"
        | "write-quit"
        | "write-quit!"
        | "write-quit-all"
        | "write-quit-all!" => &[Files, Windows],
        // settings
        "change-current-directory"
        | "set-language"
        | "set-option"
        | "toggle-option"
        | "config-reload"
        | "reload-all-plugins"
        | "snippets-reload"
        | "lsp-restart"
        | "lsp-stop"
        | "workspace-untrust"
        | "workspace-exclude" => &[Config],
        "explorer-toggle"
        | "explorer-focus"
        | "explorer-left"
        | "explorer-right"
        | "explorer-float"
        | "explorer-float-toggle"
        | "explorer-float-reset"
        | "explorer-refresh"
        | "explorer-reveal"
        | "explorer-width" => &[Explorer],
        // running programs (trusting a workspace lets its settings run them)
        "insert-output"
        | "append-output"
        | "pipe"
        | "pipe-to"
        | "run-shell-command"
        | "debug-start"
        | "debug-remote"
        | "debug-eval"
        | "lsp-workspace-command"
        | "workspace-trust" => &[Shell],
        _ => return None,
    })
}

/// The command line for `args` given one argument per element by a plugin: each is quoted
/// so that spaces or quotes inside it do not split it (`{"my file.txt"}` is one path).
/// Arguments past the command's `raw_after` positionals are read raw by the parser, so
/// they are passed as they are.
fn command_line(args: &[String], raw_after: Option<u8>) -> String {
    let mut positionals = 0;
    let mut line = String::new();
    for arg in args {
        if !line.is_empty() {
            line.push(' ');
        }
        if raw_after.is_some_and(|max| positionals >= max as usize) {
            line.push_str(arg);
            continue;
        }
        if !arg.starts_with('-') {
            positionals += 1;
        }
        // In single quotes everything is literal; a quote is escaped by doubling it.
        line.push('\'');
        line.push_str(&arg.replace('\'', "''"));
        line.push('\'');
    }
    line
}

/// A UI callback a plugin is waiting on. Owned by the prompt/picker that will
/// answer it; when that component is dropped (aborted, closed with Esc, ...)
/// the pending Lua callback is released instead of leaking.
pub struct PendingUiCallback {
    plugin_manager: Arc<PluginManager>,
    plugin_name: String,
    callback_id: u64,
}

impl PendingUiCallback {
    pub fn new(plugin_manager: Arc<PluginManager>, plugin_name: String, callback_id: u64) -> Self {
        Self {
            plugin_manager,
            plugin_name,
            callback_id,
        }
    }

    /// Call the plugin's callback with `value` (at most once; later calls are no-ops).
    pub fn fire(&self, editor: &mut Editor, value: serde_json::Value) {
        if let Err(e) = self.plugin_manager.handle_ui_callback(
            editor,
            self.plugin_name.clone(),
            self.callback_id,
            value,
        ) {
            log::error!("Plugin UI callback failed: {}", e);
            editor.set_error(format!("Plugin error: {}", e));
        }
    }
}

impl Drop for PendingUiCallback {
    fn drop(&mut self) {
        self.plugin_manager
            .release_ui_callback(std::mem::take(&mut self.plugin_name), self.callback_id);
    }
}

pub fn get_registry() -> Arc<dyn EditorCommandRegistry> {
    Arc::new(TermCommandRegistry {})
}

pub fn get_ui_handler() -> (Arc<dyn UiHandler>, UnboundedReceiver<UiRequest>) {
    let (tx, rx) = unbounded_channel();
    (Arc::new(TermUiHandler { sender: tx }), rx)
}

#[cfg(test)]
mod tests {
    use super::command_line;
    use helix_core::command_line::{Args, Signature};

    fn parse(args: &[&str], raw_after: Option<u8>) -> Vec<String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let line = command_line(&args, raw_after);
        let signature = Signature {
            positionals: (0, None),
            raw_after,
            ..Signature::DEFAULT
        };
        Args::parse(&line, signature, true, |token| Ok(token.content))
            .unwrap()
            .iter()
            .map(|arg| arg.to_string())
            .collect()
    }

    /// A plugin may only run a command with the permissions it needs: every command must be
    /// classified (a new one fails here until it is added to `typable_permissions`).
    #[test]
    fn every_typable_command_needs_known_permissions() {
        let unclassified: Vec<&str> = crate::commands::typed::TYPABLE_COMMAND_LIST
            .iter()
            .map(|cmd| cmd.name)
            .filter(|name| super::typable_permissions(name).is_none())
            .collect();
        assert_eq!(unclassified, Vec::<&str>::new());
    }

    #[test]
    fn commands_and_their_aliases_need_their_permissions() {
        use helix_plugin::types::EditorCommandRegistry;
        use helix_plugin::Permission::*;
        let registry = super::TermCommandRegistry {};
        let needs = |name: &str, args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            registry.permissions(name, &args)
        };
        assert_eq!(needs("sh", &["ls"]), Some(vec![Shell]));
        assert_eq!(needs("!", &[]), Some(vec![Shell]));
        assert_eq!(needs("w", &[]), Some(vec![Files]));
        assert_eq!(needs("wq", &[]), Some(vec![Files, Windows]));
        assert_eq!(needs("vs", &[]), Some(vec![Windows]));
        assert_eq!(needs("set", &["scrolloff", "3"]), Some(vec![Config]));
        // the settings that run programs need `shell` too
        assert_eq!(
            needs("set", &["shell", "[\"sh\", \"-c\"]"]),
            Some(vec![Config, Shell])
        );
        assert_eq!(
            needs("toggle", &["clipboard-provider"]),
            Some(vec![Config, Shell])
        );
        assert_eq!(needs("workspace-trust", &[]), Some(vec![Shell]));
        assert_eq!(needs("no-such-command", &[]), None);
    }

    #[test]
    fn plugin_arguments_are_not_split_or_unquoted() {
        let args = ["my file.txt", "it's", "\"q\"", "", "%{x}", "a\\'b", "`c`"];
        assert_eq!(parse(&args, None), args);
    }

    #[test]
    fn arguments_after_raw_after_are_passed_raw() {
        // e.g. `:set-option key value`: the value is read raw
        assert_eq!(
            parse(&["key", "[1, 2]"], Some(1)),
            vec!["key".to_string(), "[1, 2]".to_string()]
        );
    }
}
