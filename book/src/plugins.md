# Plugins

This fork can run plugins written in Lua 5.4. A plugin reacts to editor events, registers
`:` commands, reads and edits buffers, draws annotations, and opens prompts, menus and
pickers.

- [Where plugins live](#where-plugins-live)
- [Configuration](#configuration)
- [Running plugin code](#running-plugin-code)
- [Sandbox](#sandbox)
- [Events](#events)
- [API reference](#api-reference)

## Where plugins live

Each plugin is a directory inside a plugin directory. By default these are searched:

- `plugins/` in the Helix config directory (`~/.config/helix/plugins/` on Linux and macOS,
  `%AppData%\helix\plugins\` on Windows)
- `/usr/share/helix/plugins/` and `/usr/local/share/helix/plugins/` (Linux)
- `/Library/Application Support/helix/plugins/` (macOS)

```
~/.config/helix/plugins/
└── my-plugin/
    ├── plugin.toml    # metadata (optional)
    ├── init.lua       # entry point (required, unless plugin.toml names another one)
    └── util.lua       # loaded with require("my-plugin.util")
```

`plugin.toml` is optional; without it the plugin is named after its directory. All its fields
but `name` and `version` are optional:

```toml
name = "my-plugin"
version = "0.1.0"
description = "My first Helix plugin"
author = "Your Name"
entry = "init.lua"
```

A minimal `init.lua`:

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

Run `:hello` (or bind it to a key like any typable command; it is also listed in the command
palette). `:reload-all-plugins` loads every plugin again without restarting Helix: handlers,
commands and pending callbacks of the old ones are dropped first.

The repository has two examples in `contrib/plugins/`: an `auto-save` skeleton and
`inline-diagnostic`, which shows the diagnostics of the cursor line in a panel (`set_panel`).

## Configuration

Plugins are set up in the `[plugins]` section of the **global** `config.toml`:

```toml
[plugins]
enabled = true                          # false: no plugin is loaded
plugin_dirs = ["~/src/helix-plugins"]   # replaces the default directories

[[plugins.plugins]]
name = "greeter"                        # the plugin's name (plugin.toml or directory)
enabled = true
config = { greeting = "hi", nested = { level = 2 } }

[[plugins.plugins]]
name = "auto-save"
enabled = false                         # skip this one
```

| Key | Description | Default |
| --- | --- | --- |
| `enabled` | Load plugins at all | `true` |
| `plugin_dirs` | Directories to search for plugins. When set, the default directories are not searched. A leading `~` is expanded to the home directory, and a relative path is relative to the Helix config directory (`plugins` is `~/.config/helix/plugins` on Linux) | the default directories |
| `plugins` | One `[[plugins.plugins]]` entry per plugin to configure: `name`, `enabled` (default `true`) and `config`, a table the plugin reads with `helix.get_config()` | `[]` |

Plugins without an entry are loaded. The keys are written with underscores (`plugin_dirs`),
unlike the `[editor]` keys.

- The section is read from the global config file only. A workspace's `.helix/config.toml`
  cannot enable plugins or point Helix at other plugin directories: a `[plugins]` section
  there is ignored, with a warning in the log.
- An invalid `[plugins]` section (`enabled = 3`, say) is a config error, like any other.
- Plugins are loaded once, at startup. Changes to `[plugins]` take effect after a restart:
  neither `:config-reload` nor `:reload-all-plugins` reads the section again.

## Running plugin code

Plugin code runs on the editor's thread: while it runs, the editor does not draw or read
keys. Keep handlers short, especially for frequent events (`key_press`, `selection_change`,
`buffer_changed`).

- **Time limit**: loading a plugin, and each call of an event handler, a command or a UI
  callback, may run for at most 2 seconds. Then it is aborted with the error
  `plugin code exceeded the execution time limit`. The error cannot be caught: `pcall`,
  `xpcall`, `coroutine.resume`, `coroutine.close` and `load` raise it again, and the string
  pattern functions (`find`, `match`, `gmatch`, `gsub`) check the limit too.
- **Memory limit**: the Lua state may use at most 512 MiB; beyond that, allocations fail
  with a `not enough memory` error.
- **Errors** of a handler are written to the log (`:log-open`) and do not stop the
  handlers of other plugins.
- **Output**: `print` and `warn` write to the Helix log, not to the terminal.

## Sandbox

- `io`, `os.execute`, `os.exit`, `os.remove`, `os.rename`, `os.tmpname`, `os.setlocale`,
  `loadfile`, `dofile` and the `debug` library are not available. `load` only accepts source
  text, not precompiled chunks.
- `require` loads Lua modules from the plugin directories only (`require("my-plugin.util")`
  loads `<plugin dir>/my-plugin/util.lua`), never from the working directory; C modules
  cannot be loaded.
- `setmetatable` refuses metatables with a `__gc` field: finalizers would run outside the
  time limit.
- Plugins are still trusted code: `os.getenv` and `os.time`/`clock`/`date` are available,
  and `helix.editor.execute_command` can run any typable command (`:write`, `:sh`, ...).
  Only install plugins you trust.

## Events

`helix.on(event, function(event) ... end)` registers a handler; an unknown event name is an
error. The `event` table has a `type` field (the event name) and, depending on the event:

| Event | Fields | Emitted |
| --- | --- | --- |
| `init` | | after all plugins are loaded (also after `:reload-all-plugins`) |
| `ready` | | once at startup, after `buffer_open` of the files given on the command line |
| `buffer_open` | `document_id`, `path` | when a file is opened |
| `buffer_changed` | `document_id`, `path` | when a buffer's text changes; at most once per buffer per frame, however many edits were made (previews, such as a completion being shown, do not count) |
| `buffer_pre_save` | `document_id`, `path` | before a buffer is written: edits the handler makes are part of the saved file |
| `buffer_post_save` | `document_id`, `path` | after a buffer was written |
| `buffer_close` | `document_id`, `path` | when a buffer is closed |
| `selection_change` | `document_id`, `path` | when the selection or cursor moves; at most once per buffer per frame |
| `mode_change` | `old_mode`, `new_mode` (`"normal"`, `"insert"`, `"select"`) | on mode switches |
| `key_press` | `key` (e.g. `"a"`, `"C-s"`, `"ret"`) | for every key |
| `lsp_initialized` | | when a language server has finished initializing |
| `lsp_attach` | `document_id`, `language_server_id` | when a language server is attached to a buffer, once per server and buffer |
| `lsp_diagnostic` | `document_id` | when a buffer's diagnostics change |
| `view_change` | | when the terminal is resized |

`path` is `nil` for a buffer without a file. `document_id` is the same string `buffer:id()`
returns; `helix.buffer.get_by_id(id)` turns it into a buffer.

Except for `ready`, `init`, `view_change` and `buffer_pre_save`, events are queued while the
editor works and handed to the handlers before the next frame is drawn. `buffer_pre_save`
runs right before the write; when the save comes from plugin code that is running (a
command calling `helix.editor.save()`), the handlers hear of it with the next frame instead,
and their edits are not part of that write.

## API reference

Positions: buffer **lines are 1-based** in `get_line`, `insert` and `delete`; char indices,
rows and columns elsewhere are **0-based**. Columns past the end of a line are clamped to the
end of that line (before its line ending), lines past the end to the last line.

Functions that need the editor raise an error when called outside of plugin code run by the
editor; the `helix.ui` messages, `helix.editor.mode`/`*_mode` and the `helix.editor`
functions that run commands then do nothing instead.

### Commands and configuration

- `helix.register_command{ name = ..., doc = ..., args = ..., handler = function(args) end }`
  registers a typable command. `doc` (or `description`) and `args` are shown in the command
  palette; `handler` gets the arguments as a table of strings. Built-in commands of the same
  name take precedence.
- `helix.get_config()`: the `config` table of the calling plugin's `[[plugins.plugins]]`
  entry (nested tables included), or `nil` when it has none.
- `helix.version`: the Helix version string.

### `helix.buffer`

- `get_current()`: the buffer of the focused window.
- `get_by_id(id)`: the buffer with that id, or `nil`.
- `list()`: all open buffers.
- `annotation{ ... }`: an annotation (see `set_annotations`); a plain table works too.

Buffer methods (they raise an error once the buffer is closed):

- `get_text()`, `len()` (in chars), `line_count()`, `get_line(n)` (with its line ending),
  `get_path()` (or `nil`), `id()`, `is_modified()`, `get_language()` (or `nil`).
- `char_to_line(char_idx)`, `line_to_char(line)`, `get_visual_column(char_idx)` (0-based;
  tabs expanded, wide characters counted twice).
- `get_cursor()`: char index of the primary cursor (focused buffer only).
- `get_selections()`: `{ {anchor = ..., head = ...}, ... }` (char indices; empty unless the
  buffer is focused).
- `get_diagnostics()`: diagnostics with `range.start`, `range["end"]`, `line` (0-based),
  `message` and `severity` (`"error"`, `"warning"`, `"info"`, `"hint"` or `nil`).
- `insert(line, col, text)`, `delete(start_line, start_col, end_line, end_col)` (focused
  buffer only; a reversed range is accepted).
- `set_annotations(list)` (focused buffer only): replaces the annotations drawn in the
  focused window for this buffer (they are shared by all plugins). Each annotation has
  `char_idx` (anchor; it picks the line), `text`, and optionally `fg`/`bg` (`"#rrggbb"`),
  `style` (a theme key), `offset` (columns, 0-65535), `is_line` (a virtual line below the
  anchor's line instead of text after the end of the line), `virt_line_idx` (virtual lines
  with the same index share a row) and `dropped_text` (shown instead of `text` when inline
  annotations do not fit and move to a line below). A field of the wrong type is an error.
  Annotations are not moved by edits: set them again when the text changes (on
  `buffer_changed`). `set_annotations({})` clears them.
- `set_panel(panel)` (focused buffer only): shows a panel in the focused window, after the
  end of a line, drawn over the text: unlike annotations it takes no rows, so the lines
  below never move and the view does not scroll for it. `set_panel(nil)` removes it; a
  window has one panel (shared by all plugins). Helix lays it out again on every frame, so
  it follows edits of the line, resizes and scrolling:
  - Beside the line, after an arrow, when at least `min_width` columns are left for the
    text; otherwise in a box under the line, from its first column. Long texts wrap; the
    rows after the first are drawn over the lines below, and the panel is cut off at the
    bottom of the window.
  - Fields: `char_idx` (anchor; it picks the line), `items` (a list of `{ text = ...,
    icon = ..., icon_fg = ... }`; only `text` is required), and optionally `footer` (a last
    line without icon), `fg`/`bg` (`"#rrggbb"`; default: the theme's `ui.popup`), `arrow`
    (`" ← "`), `under_arrow` (`"╰ "`, before the box under the line), `arrow_fg` (default:
    `bg`), `left_cap`/`right_cap` (drawn in `bg` at the ends of a one-row panel, e.g.
    powerline caps; a panel of several rows is one block, their columns in `bg`), `min_width`
    (`30`), `spacing` (`1`: blank rows between the items and before the footer, so that
    messages do not read as one) and `show_in_insert` (`false`: hidden in insert mode).
  - The anchor is not moved by edits: set the panel again when the text changes.

### `helix.editor`

- `mode()`: `"normal"`, `"insert"` or `"select"`; `normal_mode()`, `insert_mode()`,
  `select_mode()` switch modes.
- `get_cursor()`: `{row, col}` of the primary cursor (0-based); `set_cursor(row, col)`.
- `move_cursor(direction, count)`: `"left"`, `"right"`, `"up"` or `"down"`, like `h`/`l`/`k`/`j`
  (extends the selections in select mode).
- `get_selections()`, `set_selections(list)`: `{anchor, head}` char indices (clamped to the
  text); `select_all()`.
- `undo()`, `redo()`: return whether anything changed.
- `execute_command(name, args)`: runs a typable command (`"write"`, `"vsplit"`, ...) with
  `args`, a table with one string per argument.
- `save(force)`, `save_all(force)`, `quit(force)`: `:write`, `:write-all` and `:quit` (with
  `!` when `force` is true). `quit` closes the focused window and exits Helix when it was the
  last one, unless buffers have unsaved changes and `force` is not set.
- `open(path, action)`: opens a file and returns its buffer; `action` is `"replace"`,
  `"split"`, `"vsplit"`, or omitted to load it in the background.
- `close()`: closes the focused window (not the last one: use `quit`).
- `focus()`: the focused window.
- `get_config()`: some editor settings (`scrolloff`, `mouse`, `cursorline`, ...).
- `get_register(name)`, `set_register(name, values)`: register contents as a table of
  strings.
- `set_status(message, level)`: like `helix.ui.notify`.

### `helix.ui`

- `notify(message, level)`: a message; `level` is `"info"` (default), `"warning"` or
  `"error"`. `info(message)`, `warn(message)`, `error(message)`, `set_status(message)`.
- `prompt(message, default, function(input) end)`: asks for a line of text; the callback is
  not called when the prompt is cancelled.
- `confirm(message, function(confirmed) end)`: a yes/no question (`false` when cancelled).
- `picker{ items = {...}, prompt = "...", on_select = function(item) end }`: a picker titled
  `Plugin` over `items` (strings); `prompt` is the label of its input line (`"Select:"` when
  omitted). `on_select` is required and gets the chosen string.
- `menu(items, function(item) end)`: the same picker with the default label.
- `get_theme()`, `set_theme(name)`, `get_terminal_size()` (`{width, height}` of the editor
  area), `redraw()`.
- `show_help()` is not implemented (raises an error).

Callbacks run later, after the current plugin code has returned. A prompt or picker that is
closed without a choice releases its callback without calling it (except `confirm`, which
gets `false`).

### `helix.window`

- `get_current()`, `list()`: windows; methods `id()`, `get_buffer()`, `focus()`.

### `helix.lsp`

- `get_clients()`: `{ {name = ..., id = ...}, ... }` for all running language servers.

### `helix.log`

- `info(msg)`, `warn(msg)`, `error(msg)`, `debug(msg)`, `trace(msg)`: write to the Helix log
  (`:log-open`; start Helix with `hx -v` for debug messages).
