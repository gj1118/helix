# Helix Lua Plugin System

A Lua 5.4 plugin system for this Helix fork. Plugins react to editor events, register
commands, read and edit buffers, draw annotations, and open prompts and pickers.

## Quick start

Plugins are loaded at startup from these directories (each plugin is a subdirectory):

- `~/.config/helix/plugins/` (the Helix config directory)
- `/usr/share/helix/plugins/` and `/usr/local/share/helix/plugins/` (Linux)
- `/Library/Application Support/helix/plugins/` (macOS)

```
~/.config/helix/plugins/
└── my-plugin/
    ├── plugin.toml    # Metadata (optional)
    ├── init.lua       # Entry point (required, unless plugin.toml names another one)
    └── util.lua       # Loaded with require("my-plugin.util")
```

**plugin.toml** (all fields but `name` and `version` are optional; without the file the
plugin is named after its directory):

```toml
name = "my-plugin"
version = "0.1.0"
description = "My first Helix plugin"
author = "Your Name"
entry = "init.lua"
permissions = ["read", "ui", "commands"]   # what it needs, see Permissions
```

**init.lua** (registering a command needs the `commands` permission, granted in
`config.toml`):

```lua
helix.on("buffer_open", function(event)
    helix.log.info("opened " .. tostring(event.path))
end)

helix.register_command({
    name = "hello",
    doc = "Say hello",
    handler = function(args)
        helix.ui.notify("Hello " .. (args[1] or "world"))
    end,
})
```

Run `:hello` (or bind it to a key like any typable command). `:reload-all-plugins`
reloads every plugin without restarting.

## Configuration

The `[plugins]` section of the **global** `config.toml` (a workspace's `.helix/config.toml`
cannot set it: it is ignored there, with a warning in the log):

```toml
[plugins]
enabled = true                                 # false: no plugin is loaded
plugin_dirs = ["~/src/helix-plugins"]          # replaces the default directories; `~` is
                                               # expanded, relative paths are relative to
                                               # the Helix config directory

[[plugins.plugins]]
name = "greeter"
enabled = true                                 # false: skip this plugin
config = { greeting = "hi", nested = { level = 2 } }   # helix.get_config() in the plugin
permissions = ["commands"]                     # beyond "read" and "ui", see Permissions
```

Plugins without an entry are loaded. The section is read at startup only: changes need a
restart (`:reload-all-plugins` reloads the plugins' code, not this section).

The user documentation is the book's [Plugins](../book/src/plugins.md) chapter.

## Permissions

A plugin may look at the editor and show things; anything else is granted per plugin, in its
`[[plugins.plugins]]` entry of `config.toml`:

```toml
[[plugins.plugins]]
name = "formatter-on-save"
permissions = ["edit", "files"]
```

| Permission | Allows | Default |
| --- | --- | --- |
| `read` | Reading buffers, windows, selections, diagnostics, registers and settings; `helix.on` (hearing of events) | granted |
| `ui` | Messages, prompts, pickers, menus, annotations, panels, the theme | granted |
| `edit` | Changing buffers: text, selections, cursor, mode, registers, undo/redo | |
| `windows` | Opening files and windows, splitting, focusing, closing, quitting | |
| `files` | Writing buffers to disk (`helix.editor.save`, `:write`, `:move`...) | |
| `commands` | Registering `:` commands, and running Helix's own with `helix.editor.execute_command` | |
| `config` | Changing settings: `:set`, `:toggle`, `:config-reload`, `:lang`, `:cd`, `:lsp-restart`... | |
| `keymaps` | Changing key bindings | |
| `explorer` | Controlling the file explorer | |
| `shell` | Running programs: `:sh`, `:pipe`, `:insert-output`, debug adapters, `:lsp-workspace-command`, `:workspace-trust`, and the settings that run programs (`:set shell`, `clipboard-provider`) | |

`"all"` grants everything. An unknown name is a config error.

- **What a plugin asks for**: a plugin can list the permissions it needs in its `plugin.toml`
  (`permissions = ["read", "ui", "edit"]`). It then gets no more than that, whatever is
  granted; what it asks for and is not granted is reported in the status line (and the log)
  when it loads. Without the list it gets the defaults and its grants. A plugin may also
  leave out a default (`["read"]`: no messages).
- **Refused calls** raise an error that names the permission and how to grant it:
  `plugin 'x' may not use helix.editor.save: it needs the 'files' permission (grant with
  permissions = ["files"] ...)`. `helix.permissions()` lists what the calling plugin may do,
  `helix.has_permission("edit")` checks one.
- **Commands run by plugins**: `helix.editor.execute_command` needs `commands` and what the
  command itself needs: `:write` needs `files` too, `:vsplit` `windows`, `:sh` `shell`.
  `helix.editor.save()`/`save_all()` need only `files`, `quit()` only `windows`.
- **Isolation**: every plugin has a Lua state of its own. A plugin cannot change what another
  calls (its `helix` table, `string` functions, globals), act with another's permissions, nor
  take over another plugin's `:` command. Plugins that want to share code `require` the same
  module, each loading its own copy.

## Running plugin code

Plugin code runs on the editor's thread: while it runs, the editor does not draw or read
keys. Keep handlers short, especially for frequent events (`key_press`,
`selection_change`).

- **Time limit**: loading a plugin and each call of an event handler, command or UI
  callback may run for at most 2 seconds; then it is aborted with the error
  `plugin code exceeded the execution time limit`. `pcall`, `xpcall`, `coroutine.resume`,
  `coroutine.close` and `load` cannot catch that error, and the pattern functions of the
  string library check the limit too.
- **Memory limit**: the Lua state may use at most 512 MiB; beyond that allocations fail
  with a `not enough memory` error.
- **Errors** of a handler are logged (`:log-open`) and do not stop the handlers of other
  plugins.
- **Output**: `print` and `warn` write to the Helix log, not to the terminal.

## Sandbox

- `io`, `os.execute`, `os.exit`, `os.remove`, `os.rename`, `os.tmpname`, `os.setlocale`,
  `loadfile`, `dofile` and the `debug` library are not available. `load` only accepts
  source text (no precompiled chunks).
- `require` loads Lua modules from the plugin directories only (never from the working
  directory); C modules cannot be loaded.
- `setmetatable` refuses metatables with a `__gc` field: finalizers would run outside the
  time limit.
- Each plugin has a Lua state of its own and may only do what it is granted (see
  [Permissions](#permissions)). `os.getenv` and `os.time`/`clock`/`date` are available, and
  a plugin granted `shell` can run any program.

## API reference

Positions: buffer **lines are 1-based** in `get_line`, `insert` and `delete`; char indices,
rows and columns elsewhere are **0-based**. Columns past the end of a line are clamped to
the end of that line (before its line ending), lines past the end to the last line.

Functions that need the editor raise an error when called outside of plugin code run by
the editor; the `helix.ui` messages, `helix.editor.mode`/`*_mode` and the
`helix.editor` functions that run commands then do nothing instead.

### Events

`helix.on(event, function(event) ... end)` registers a handler. The `event` table has a
`type` field (the event name) and, depending on the event:

| Event | Fields | Emitted |
| --- | --- | --- |
| `init` | | after all plugins are loaded (also after `:reload-all-plugins`) |
| `ready` | | once at startup, after `buffer_open` of the files given on the command line |
| `exit` | | when Helix exits, before the last writes: buffers saved now are written |
| `buffer_open` | `document_id`, `path` | when a file is opened |
| `buffer_changed` | `document_id`, `path` | when a buffer's text changes; at most once per buffer per frame, however many edits were made (previews, such as a completion being shown, do not count) |
| `buffer_pre_save` | `document_id`, `path` | before a buffer is written: edits the handler makes are part of the saved file |
| `buffer_post_save` | `document_id`, `path` | after a buffer was written |
| `buffer_save_failed` | `document_id`, `path`, `error` | when writing a buffer failed |
| `buffer_close` | `document_id`, `path` | when a buffer is closed |
| `buffer_rename` | `document_id`, `path`, `old_path` | when a buffer's path changes (`:write <path>`, `:move`) |
| `buffer_focus_lost` | `document_id`, `path` | when the focus leaves a buffer (another window, another buffer in the window) |
| `language_change` | `document_id`, `language`, `old_language` | when a buffer's language changes (`:lang`, a rename); `nil` for none |
| `selection_change` | `document_id`, `path` | when the selection or cursor moves; at most once per buffer per frame |
| `mode_change` | `old_mode`, `new_mode` (`"normal"`, `"insert"`, `"select"`) | on mode switches |
| `key_press` | `key` (e.g. `"a"`, `"C-s"`, `"ret"`) | for every key |
| `insert_char` | `char`, `document_id` | for every character typed in insert mode |
| `command` | `name` (`"move_char_left"`, `"write"`, ...) | after each command run from a key, `:` or a macro |
| `scroll` | `window_id`, `document_id`, `top_char` | when a window scrolls (once per frame) |
| `window_open` | `window_id`, `document_id` | when a window opens (`:vsplit`, ...) |
| `window_close` | `window_id`, `document_id` | when a window closes |
| `window_focus` | `window_id`, `document_id`, `previous_window_id` | when another window gets the focus |
| `view_change` | | when the terminal is resized |
| `focus_gained`, `focus_lost` | | when the terminal gets or loses the focus (terminals with focus reporting) |
| `paste` | `text` | when text is pasted into the terminal (bracketed paste) |
| `mouse` | `kind` (`"down"`, `"up"`, `"drag"`, `"scroll_up"`, `"scroll_down"`, `"scroll_left"`, `"scroll_right"`), `button` (`"left"`, `"right"`, `"middle"` or `nil`), `row`, `col`, `modifiers` (`"C-S"`, `""`) | for clicks, drags and the wheel (not plain moves) |
| `idle` | | after [`idle-timeout`](../book/src/editor.md#editor-section) without input, like Vim's `CursorHold`; runs right away |
| `config_change` | | when the settings change (`:set`, `:config-reload`); once per frame |
| `theme_change` | `theme`, `old_theme` | when the theme changes |
| `cwd_change` | `path`, `old_path` | when the working directory changes (`:cd`) |
| `lsp_initialized` | `language_server_id`, `name` | when a language server has finished initializing |
| `lsp_attach` | `document_id`, `language_server_id`, `name` | when a language server is attached to a buffer, once per server and buffer |
| `lsp_detach` | `document_id`, `language_server_id`, `name` | when a language server stops serving a buffer that stays open |
| `lsp_exit` | `language_server_id`, `name` | when a language server exits |
| `lsp_diagnostic` | `document_id`, `path` | when a buffer's diagnostics change |

`path` is `nil` for a buffer without a file. `document_id` is the same string `buffer:id()`
returns (`helix.buffer.get_by_id(id)` turns it into a buffer), and `window_id` the one
`window:id()` returns. `language_server_id` is a number.

Helix does not queue an event no plugin has a handler for. Except for `ready`, `init`,
`exit`, `idle`, `view_change`, `buffer_save_failed`, `buffer_post_save` and
`buffer_pre_save`, events are queued while the editor works and handed to the handlers before
the next frame (a frame queues at most 10 000; a long macro does not turn into as many
handler calls). `buffer_pre_save` runs right before the write; when the save comes from
plugin code that is running (a command calling `helix.editor.save()`), the handlers hear of
it with the next frame instead, and their edits are not part of that write.

### Commands and configuration

- `helix.register_command{ name = ..., doc = ..., args = ..., handler = function(args) end }`
  registers a typable command. `doc` (or `description`) and `args` are shown in the
  command palette; `handler` gets the arguments as a table of strings. Built-in commands
  of the same name take precedence.
- `helix.get_config()`: the `config` table of the calling plugin's `[[plugins.plugins]]`
  entry (nested tables included), or `nil`.
- `helix.version`: the Helix version string.
- `helix.permissions()`, `helix.has_permission(name)`: what the calling plugin may do.

### `helix.buffer`

- `get_current()`: the buffer of the focused window.
- `get_by_id(id)`: the buffer with that id, or `nil`.
- `list()`: all open buffers.
- `annotation{ ... }`: an annotation (see `set_annotations`); a plain table works too.

Buffer methods (they raise an error once the buffer is closed):

- `get_text()`, `len()` (in chars), `line_count()`, `get_line(n)` (with its line ending),
  `get_path()` (or `nil`), `id()`, `is_modified()`, `get_language()` (or `nil`).
- `char_to_line(char_idx)`, `line_to_char(line)`, `get_visual_column(char_idx)`
  (0-based; tabs expanded, wide characters counted twice).
- `get_cursor()`: char index of the primary cursor (focused buffer only).
- `get_selections()`: `{ {anchor = ..., head = ...}, ... }` (char indices; empty unless
  the buffer is focused).
- `get_diagnostics()`: diagnostics with `range.start`, `range["end"]`, `line` (0-based),
  `message` and `severity` (`"error"`, `"warning"`, `"info"`, `"hint"` or `nil`).
- `insert(line, col, text)`, `delete(start_line, start_col, end_line, end_col)` (focused
  buffer only; a reversed range is accepted).
- `set_annotations(list)` (focused buffer only): replaces the annotations drawn in the
  focused window for this buffer (they are shared by all plugins). Each annotation has `char_idx` (anchor; it picks the line),
  `text`, and optionally `fg`/`bg` (`"#rrggbb"`), `style` (a theme key), `offset`
  (columns, 0-65535), `is_line` (a virtual line below the anchor's line instead of text
  after the end of the line), `virt_line_idx` (virtual lines with the same index share a
  row) and `dropped_text` (shown instead of `text` when inline annotations do not fit and
  move to a line below). A field of the wrong type is an error. Annotations are not moved
  by edits: set them again when the text changes. `set_annotations({})` clears them.
- `set_panel(panel)` (focused buffer only): a panel after the end of a line, drawn over the
  text (it takes no rows, so nothing moves or scrolls for it); `set_panel(nil)` removes it.
  Beside the line after an `arrow` when `min_width` (30) columns are left for the text,
  otherwise in a box under the line; texts wrap, and it is cut off at the bottom of the
  window. Fields: `char_idx`, `items` (`{ text, icon, icon_fg }`), and optionally `footer`,
  `fg`/`bg`, `arrow`, `under_arrow`, `arrow_fg`, `left_cap`/`right_cap`, `min_width` and
  `show_in_insert` (default `false`). Laid out again on every frame; the anchor is not
  moved by edits.

### `helix.editor`

- `mode()`: `"normal"`, `"insert"` or `"select"`; `normal_mode()`, `insert_mode()`,
  `select_mode()` switch modes.
- `get_cursor()`: `{row, col}` of the primary cursor (0-based); `set_cursor(row, col)`.
- `move_cursor(direction, count)`: `"left"`, `"right"`, `"up"` or `"down"`, like `h`/`l`/
  `k`/`j` (extends the selections in select mode).
- `get_selections()`, `set_selections(list)`: `{anchor, head}` char indices (clamped to the
  text); `select_all()`.
- `undo()`, `redo()`: return whether anything changed.
- `execute_command(name, args)`: runs a typable command (`"write"`, `"vsplit"`, ...) with
  `args`, a table with one string per argument.
- `save(force)`, `save_all(force)`, `quit(force)`: `:write`, `:write-all` and `:quit`
  (with `!` when `force` is true). `quit` closes the focused window and exits Helix when
  it was the last one, unless buffers have unsaved changes and `force` is not set.
- `open(path, action)`: opens a file and returns its buffer; `action` is `"replace"`,
  `"split"`, `"vsplit"`, or omitted to load it in the background.
- `close()`: closes the focused window (not the last one: use `quit`).
- `focus()`: the focused window.
- `get_config()`: some editor settings (`scrolloff`, `mouse`, `cursorline`, ...).
- `get_register(name)`, `set_register(name, values)`: register contents as a table of
  strings.
- `set_status(message, level)`: like `helix.ui.notify`.

### `helix.ui`

- `notify(message, level)`: status line message; `level` is `"info"` (default),
  `"warning"` or `"error"`. `info(message)`, `warn(message)`, `error(message)`,
  `set_status(message)`.
- `prompt(message, default, function(input) end)`: asks for a line of text; the callback
  is not called when the prompt is cancelled.
- `confirm(message, function(confirmed) end)`: a yes/no question (`false` when cancelled).
- `picker{ items = {...}, prompt = "...", on_select = function(item) end }`: a picker
  over `items`; `prompt` is the label of its input line (`"Select:"` by default).
  `menu(items, function(item) end)`: the same with the default label.
- `get_theme()`, `set_theme(name)`, `get_terminal_size()` (`{width, height}` of the
  editor area), `redraw()`.
- `show_help()` is not implemented (raises an error).

Callbacks run later, after the current plugin code has returned.

### `helix.window`

- `get_current()`, `list()`: windows; methods `id()`, `get_buffer()`, `focus()`.

### `helix.lsp`

- `get_clients()`: `{ {name = ..., id = ...}, ... }` for all running language servers.

### `helix.log`

- `info(msg)`, `warn(msg)`, `error(msg)`, `debug(msg)`, `trace(msg)`: write to the Helix
  log (`:log-open`; `hx -v` for debug messages).

## Example plugins

`contrib/plugins/` has an `auto-save` skeleton and `inline-diagnostic`, which shows the
diagnostics of the cursor line in a panel, like tiny-inline-diagnostic.nvim.

## Development

```bash
cargo test -p helix-plugin
RUST_LOG=helix_plugin=debug hx   # or hx -v
```
