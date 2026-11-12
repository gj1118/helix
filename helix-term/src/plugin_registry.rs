use crate::compositor::Context;
use crate::job::{Job, Jobs};
use crate::ui::PromptEvent;
use helix_core::command_line::Args;
use helix_plugin::types::{EditorCommandRegistry, PluginPicker, PluginPickerItem, UiHandler};
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
    OpenPicker {
        picker: PluginPicker,
        plugin_name: String,
        callback_id: u64,
    },
    PickerAdd {
        picker_id: u64,
        items: Vec<PluginPickerItem>,
    },
    PickerClose {
        picker_id: u64,
    },
    Quickfix {
        open: bool,
    },
    /// A result for a plugin's pending callback (a program's output, a server's answer).
    Deliver {
        plugin_name: String,
        callback_id: u64,
        value: serde_json::Value,
    },
    /// A plugin's timer is due.
    Timer {
        plugin_name: String,
        timer_id: u64,
    },
    Popup(helix_plugin::types::PluginPopup),
    PopupClose {
        popup_id: u64,
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

    fn open_picker(
        &self,
        _editor: &mut Editor,
        picker: PluginPicker,
        plugin_name: String,
        callback_id: u64,
    ) {
        let _ = self.sender.send(UiRequest::OpenPicker {
            picker,
            plugin_name,
            callback_id,
        });
    }

    fn picker_add(&self, picker_id: u64, items: Vec<PluginPickerItem>) {
        let _ = self.sender.send(UiRequest::PickerAdd { picker_id, items });
    }

    fn picker_close(&self, picker_id: u64) {
        let _ = self.sender.send(UiRequest::PickerClose { picker_id });
    }

    fn quickfix(&self, open: bool) {
        let _ = self.sender.send(UiRequest::Quickfix { open });
    }

    fn deliver(&self, plugin_name: String, callback_id: u64, value: serde_json::Value) {
        let _ = self.sender.send(UiRequest::Deliver {
            plugin_name,
            callback_id,
            value,
        });
    }

    fn timer(&self, plugin_name: String, timer_id: u64) {
        let _ = self.sender.send(UiRequest::Timer {
            plugin_name,
            timer_id,
        });
    }

    fn popup(&self, popup: helix_plugin::types::PluginPopup) {
        let _ = self.sender.send(UiRequest::Popup(popup));
    }

    fn popup_close(&self, popup_id: u64) {
        let _ = self.sender.send(UiRequest::PopupClose { popup_id });
    }
}

/// The layer id of plugins' popups (one at a time).
pub const PLUGIN_POPUP_ID: &str = "plugin-popup";

/// The popup of a plugin, laid out in the editor area `area`.
pub fn plugin_popup(
    popup: helix_plugin::types::PluginPopup,
    editor: &Editor,
    area: helix_view::graphics::Rect,
) -> Box<dyn crate::compositor::Component> {
    use helix_plugin::types::{PopupContent, PopupPosition};
    use helix_view::graphics::{Modifier, Style};
    use tui::text::{Span, Spans, Text};
    let theme = &editor.theme;
    let title_style = theme
        .try_get("ui.popup.title")
        .unwrap_or_else(|| Style::default().add_modifier(Modifier::BOLD));
    // The size of the content, to center it.
    let (width, height) = match &popup.content {
        PopupContent::Lines(lines) => (
            lines
                .iter()
                .map(|line| {
                    line.iter()
                        .map(|(text, _, _)| {
                            helix_core::unicode::width::UnicodeWidthStr::width(text.as_str())
                        })
                        .sum::<usize>()
                })
                .chain(popup.title.iter().map(|t| t.chars().count()))
                .max()
                .unwrap_or(0),
            lines.len() + popup.title.as_ref().map_or(0, |_| 2),
        ),
        PopupContent::Markdown(text) => (
            text.lines().map(|l| l.chars().count()).max().unwrap_or(0),
            text.lines().count() + popup.title.as_ref().map_or(0, |_| 2),
        ),
    };
    let width = popup.max_width.map_or(width, |max| width.min(max as usize)) as u16 + 2;
    let height = popup
        .max_height
        .map_or(height, |max| height.min(max as usize)) as u16
        + 2;
    let anchor = match popup.position {
        PopupPosition::Cursor => None,
        PopupPosition::Center => Some(helix_core::Position::new(
            (area.height.saturating_sub(height) / 2).saturating_sub(1) as usize,
            area.width.saturating_sub(width) as usize / 2,
        )),
        // (the popup is drawn below its anchor)
        PopupPosition::At { row, col } => Some(helix_core::Position::new(
            row.saturating_sub(1) as usize,
            col as usize,
        )),
    };
    fn finish<T: crate::compositor::Component>(
        popup: crate::ui::Popup<T>,
        anchor: Option<helix_core::Position>,
        spec: &helix_plugin::types::PluginPopup,
    ) -> Box<dyn crate::compositor::Component> {
        let popup = popup
            .auto_close(!spec.persistent)
            .max_size(spec.max_width, spec.max_height);
        Box::new(match anchor {
            Some(anchor) => popup.fixed_position(anchor),
            None => popup,
        })
    }
    match &popup.content {
        PopupContent::Lines(lines) => {
            let mut text: Vec<Spans<'static>> = Vec::new();
            if let Some(title) = &popup.title {
                text.push(Spans::from(Span::styled(title.clone(), title_style)));
                text.push(Spans::default());
            }
            for line in lines {
                text.push(Spans::from(
                    line.iter()
                        .map(|(segment, scope, style)| {
                            let base = scope
                                .as_deref()
                                .map(|scope| theme.get(scope))
                                .unwrap_or_default();
                            Span::styled(segment.clone(), base.patch(*style))
                        })
                        .collect::<Vec<_>>(),
                ));
            }
            let contents = crate::ui::Text::from(Text::from(text));
            finish(
                crate::ui::Popup::new(PLUGIN_POPUP_ID, contents),
                anchor,
                &popup,
            )
        }
        PopupContent::Markdown(markdown) => {
            let markdown = match &popup.title {
                Some(title) => format!("**{title}**\n\n{markdown}"),
                None => markdown.clone(),
            };
            let contents = crate::ui::Markdown::new(markdown, editor.syn_loader.clone());
            finish(
                crate::ui::Popup::new(PLUGIN_POPUP_ID, contents),
                anchor,
                &popup,
            )
        }
    }
}

/// The text of a column of a plugin picker item (a column format is a plain function).
fn column_cell(item: &PluginPickerItem, column: usize) -> tui::widgets::Cell<'_> {
    item.columns
        .get(column)
        .map(String::as_str)
        .unwrap_or("")
        .into()
}

type ColumnCell = for<'a> fn(&'a PluginPickerItem, &'a u64) -> tui::widgets::Cell<'a>;

/// The columns a plugin picker may have (`helix_plugin::MAX_PICKER_COLUMNS`).
const COLUMN_CELLS: [ColumnCell; helix_plugin::MAX_PICKER_COLUMNS] = [
    |item, _| column_cell(item, 0),
    |item, _| column_cell(item, 1),
    |item, _| column_cell(item, 2),
    |item, _| column_cell(item, 3),
    |item, _| column_cell(item, 4),
    |item, _| column_cell(item, 5),
    |item, _| column_cell(item, 6),
    |item, _| column_cell(item, 7),
];

/// The picker of a plugin: its items' columns, a preview of their files or buffers, and the
/// plugin's callback with `{index, action}` of the item chosen. Its data is its id.
pub fn plugin_picker(
    picker: PluginPicker,
    pending: PendingUiCallback,
) -> crate::ui::Picker<PluginPickerItem, u64> {
    let names: Vec<String> = if picker.columns.is_empty() {
        vec!["item".to_string()]
    } else {
        picker.columns.clone()
    };
    let columns: Vec<_> = names
        .into_iter()
        .zip(COLUMN_CELLS)
        .map(|(name, cell)| crate::ui::PickerColumn::new(name, cell))
        .collect();
    let built = crate::ui::Picker::new(
        columns,
        0,
        picker.items,
        picker.id,
        move |cx, item: &PluginPickerItem, action| {
            let action = match action {
                helix_view::editor::Action::HorizontalSplit => "hsplit",
                helix_view::editor::Action::VerticalSplit => "vsplit",
                helix_view::editor::Action::Load => "load",
                helix_view::editor::Action::Replace => "replace",
            };
            pending.fire(
                cx.editor,
                serde_json::json!({ "index": item.index, "action": action }),
            );
        },
    )
    .with_title(picker.title)
    .with_prompt(picker.prompt);
    if picker.preview {
        built.with_preview(|_editor, item: &PluginPickerItem| {
            if let Some(path) = item.path.as_deref() {
                Some((path.into(), item.lines))
            } else {
                item.document.map(|doc| (doc.into(), item.lines))
            }
        })
    } else {
        built
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

    fn run_static(
        &self,
        editor: &mut Editor,
        name: &str,
        count: usize,
    ) -> std::result::Result<(), anyhow::Error> {
        let command = crate::commands::MappableCommand::STATIC_COMMAND_LIST
            .iter()
            .find(|command| command.name() == name)
            .ok_or_else(|| anyhow::anyhow!("No command '{name}'"))?;
        let mut jobs = Jobs::new();
        let mut cx = crate::commands::Context {
            register: None,
            count: std::num::NonZeroUsize::new(count),
            editor,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: &mut jobs,
            plugin_manager: None,
        };
        command.execute(&mut cx);
        // What the command asks of the compositor (a picker, a prompt) runs after the
        // plugin code; a command that waits for the next key (`f`, `r`...) cannot be run.
        let waits_for_key = cx.on_next_key_callback.is_some();
        for callback in std::mem::take(&mut cx.callback) {
            crate::compositor::defer(callback);
        }
        for future in std::mem::take(&mut jobs.wait_futures) {
            jobs.add(Job {
                future,
                wait: false,
            });
        }
        if waits_for_key {
            anyhow::bail!("'{name}' waits for a key: bind it to a key instead");
        }
        Ok(())
    }

    fn static_permissions(&self, name: &str) -> Option<Vec<Permission>> {
        crate::commands::MappableCommand::STATIC_COMMAND_LIST
            .iter()
            .any(|command| command.name() == name)
            .then(|| static_permissions(name).to_vec())
    }

    fn binding_permissions(&self, command: &str) -> std::result::Result<Vec<Permission>, String> {
        if let Some(line) = command.strip_prefix(':') {
            let mut words = line.split_whitespace();
            let name = words.next().unwrap_or("");
            let args: Vec<String> = words.map(String::from).collect();
            return self
                .permissions(name, &args)
                .ok_or_else(|| format!("No command ':{name}'"));
        }
        if let Some(keys) = command.strip_prefix('@') {
            // A macro replays keys, which can do anything.
            helix_view::input::parse_macro(keys)
                .map_err(|e| format!("Invalid macro '{command}': {e}"))?;
            return Ok(Permission::ALL.to_vec());
        }
        self.static_permissions(command)
            .ok_or_else(|| format!("No command '{command}'"))
    }
}

/// What running a command of key bindings needs. Most change the buffer or the selections
/// (`edit`): the others are listed. A test fails when commands are added or removed, until
/// this is checked again.
fn static_permissions(name: &str) -> &'static [Permission] {
    use Permission::*;
    match name {
        "no_op" => &[],
        // running programs or debuggees; replaying a macro (any keys)
        "shell_pipe"
        | "shell_pipe_to"
        | "shell_insert_output"
        | "shell_append_output"
        | "shell_keep_pipe"
        | "replay_macro"
        | "code_lens" => &[Shell],
        name if name.starts_with("dap_") => &[Shell],
        // edits other files, may create, rename or delete them
        "rename_symbol" => &[Edit, Files],
        // windows, and other buffers or files in them
        "hsplit"
        | "hsplit_new"
        | "vsplit"
        | "vsplit_new"
        | "wclose"
        | "wonly"
        | "jump_view_right"
        | "jump_view_left"
        | "jump_view_up"
        | "jump_view_down"
        | "swap_view_right"
        | "swap_view_left"
        | "swap_view_up"
        | "swap_view_down"
        | "transpose_view"
        | "rotate_view"
        | "rotate_view_reverse"
        | "toggle_focus_window"
        | "grow_buffer_width"
        | "shrink_buffer_width"
        | "grow_buffer_height"
        | "shrink_buffer_height"
        | "goto_file"
        | "goto_file_hsplit"
        | "goto_file_vsplit"
        | "goto_last_accessed_file"
        | "goto_last_modified_file"
        | "goto_next_buffer"
        | "goto_previous_buffer"
        | "goto_definition"
        | "goto_declaration"
        | "goto_type_definition"
        | "goto_implementation"
        | "goto_reference"
        | "jump_forward"
        | "jump_backward"
        | "quicklist_next"
        | "quicklist_prev"
        | "quicklist_next_in_file"
        | "quicklist_prev_in_file"
        | "quickfix"
        | "quickfix_focus"
        | "suspend" => &[Windows],
        // pickers, prompts and popups: what happens next is chosen by the user
        "command_mode"
        | "command_palette"
        | "file_picker"
        | "recent_picker"
        | "file_picker_in_current_buffer_directory"
        | "file_picker_in_current_directory"
        | "file_explorer"
        | "file_explorer_in_current_buffer_directory"
        | "file_explorer_in_current_directory"
        | "code_action"
        | "code_action_picker"
        | "code_lens_picker"
        | "buffer_picker"
        | "jumplist_picker"
        | "symbol_picker"
        | "syntax_symbol_picker"
        | "lsp_or_syntax_symbol_picker"
        | "changed_file_picker"
        | "git_stash_picker"
        | "git_file_history"
        | "git_history"
        | "workspace_symbol_picker"
        | "syntax_workspace_symbol_picker"
        | "lsp_or_syntax_workspace_symbol_picker"
        | "diagnostics_picker"
        | "workspace_diagnostics_picker"
        | "todos_picker"
        | "workspace_todos_picker"
        | "last_picker"
        | "global_search"
        | "local_search_grep"
        | "local_search_fuzzy"
        | "signature_help"
        | "hover"
        | "goto_hover"
        | "snippet_picker"
        | "quicklist_picker"
        | "blame_line"
        | "blame_details"
        | "completion" => &[Ui],
        "explorer" => &[Explorer],
        name if name.starts_with("explorer_") => &[Explorer],
        // moving in a diff (applying a change from a side to the other edits)
        "diff_next_file" | "diff_previous_file" => &[Files, Windows],
        "diff_stage" => &[Files],
        "diff_commit" => &[Files, Windows],
        "diff_next_change"
        | "diff_previous_change"
        | "diff_snap"
        | "diff_align_top"
        | "diff_align_bottom"
        | "diff_close" => &[Windows],
        _ => &[Edit],
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
        | "quickfix"
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
        // files (or git) read, and windows opened
        "diff" | "diff-buffers" | "diff-git" | "diff-review" | "diff-commit" => &[Files, Windows],
        "diff-close" => &[Windows],
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
        "explorer" | "explorer-focus" | "explorer-float" | "explorer-refresh"
        | "explorer-reveal" | "explorer-width" => &[Explorer],
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
    use helix_plugin::Permission;

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

    /// The classification of the commands of key bindings is checked again when commands
    /// come or go: update the count once `static_permissions` lists what they need.
    #[test]
    fn static_commands_are_classified() {
        let count = crate::commands::MappableCommand::STATIC_COMMAND_LIST.len();
        assert_eq!(
            count, 401,
            "commands were added or removed: check `static_permissions` for them"
        );
        use helix_plugin::types::EditorCommandRegistry;
        use helix_plugin::Permission::*;
        let registry = super::TermCommandRegistry {};
        let needs = |name: &str| registry.static_permissions(name);
        assert_eq!(needs("move_line_down"), Some(vec![Edit]));
        assert_eq!(needs("shell_pipe"), Some(vec![Shell]));
        assert_eq!(needs("dap_launch"), Some(vec![Shell]));
        assert_eq!(needs("vsplit"), Some(vec![Windows]));
        assert_eq!(needs("file_picker"), Some(vec![Ui]));
        assert_eq!(needs("explorer"), Some(vec![Explorer]));
        assert_eq!(needs("explorer_float"), Some(vec![Explorer]));
        assert_eq!(needs("rename_symbol"), Some(vec![Edit, Files]));
        assert_eq!(needs("no_such_command"), None);
    }

    #[test]
    fn key_bindings_need_what_their_command_needs() {
        use helix_plugin::types::EditorCommandRegistry;
        use helix_plugin::Permission::*;
        let registry = super::TermCommandRegistry {};
        let needs = |command: &str| registry.binding_permissions(command);
        assert_eq!(needs("move_line_down"), Ok(vec![Edit]));
        assert_eq!(needs(":write"), Ok(vec![Files]));
        assert_eq!(needs(":sh rm -rf x"), Ok(vec![Shell]));
        assert_eq!(needs(":set shell [\"x\"]"), Ok(vec![Config, Shell]));
        assert_eq!(needs("@ihi<esc>"), Ok(Permission::ALL.to_vec()));
        assert_eq!(needs("no_op"), Ok(vec![]));
        let unknown: Result<Vec<Permission>, String> = needs("nope");
        assert_eq!(unknown, Err("No command 'nope'".to_string()));
        let unknown_typable: Result<Vec<Permission>, String> = needs(":nope");
        assert_eq!(unknown_typable, Err("No command ':nope'".to_string()));
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
