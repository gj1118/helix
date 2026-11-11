# Plugins

This fork can run plugins written in Lua 5.4. A plugin reacts to editor events, registers
`:` commands, reads and edits buffers, draws annotations, and opens prompts, menus and
pickers.

- [Where plugins live](#where-plugins-live)
- [Configuration](#configuration)
- [Permissions](#permissions)
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
permissions = ["read", "ui", "commands"]   # what it needs, see Permissions
```

A minimal `init.lua` (registering a command needs the `commands` permission, granted in
`config.toml`; see [Permissions](#permissions)):

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

The repository has three examples in `contrib/plugins/`: an `auto-save` skeleton,
`inline-diagnostic`, which shows the diagnostics of the cursor line in a panel (`set_panel`),
and `hlslens`, a port of [nvim-hlslens](https://github.com/kevinhwang91/nvim-hlslens) (see
[Search lenses](#search-lenses-hlslens)). Copy a directory to `~/.config/helix/plugins/` to
use it.

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
permissions = ["commands"]              # beyond "read" and "ui", see Permissions

[[plugins.plugins]]
name = "auto-save"
enabled = false                         # skip this one
```

| Key | Description | Default |
| --- | --- | --- |
| `enabled` | Load plugins at all | `true` |
| `plugin_dirs` | Directories to search for plugins. When set, the default directories are not searched. A leading `~` is expanded to the home directory, and a relative path is relative to the Helix config directory (`plugins` is `~/.config/helix/plugins` on Linux) | the default directories |
| `plugins` | One `[[plugins.plugins]]` entry per plugin to configure: `name`, `enabled` (default `true`), `config`, a table the plugin reads with `helix.get_config()`, and `permissions`, what it may do beyond the defaults (see [Permissions](#permissions)) | `[]` |

Plugins without an entry are loaded. The keys are written with underscores (`plugin_dirs`),
unlike the `[editor]` keys.

- The section is read from the global config file only. A workspace's `.helix/config.toml`
  cannot enable plugins or point Helix at other plugin directories: a `[plugins]` section
  there is ignored, with a warning in the log.
- An invalid `[plugins]` section (`enabled = 3`, say) is a config error, like any other.
- Plugins are loaded once, at startup. Changes to `[plugins]` take effect after a restart:
  neither `:config-reload` nor `:reload-all-plugins` reads the section again.

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
| `shell` | Running programs: `helix.process`, `:sh`, `:pipe`, `:insert-output`, debug adapters, `:lsp-workspace-command`, `:workspace-trust`, the settings that run programs (`:set shell`, `clipboard-provider`), and the language server requests that are not only reads (`workspace/executeCommand`, a server's own methods, notifications) | |

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
keys. Keep handlers short, especially for frequent events (`key_press`, `selection_change`,
`buffer_changed`). What takes time runs in the background with a callback: timers
(`helix.defer`, `helix.every`), programs (`helix.process`) and language server requests
(`helix.lsp.request`); the callbacks run on the editor's thread, with the time limit, when
the result is in.

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
- Each plugin has a Lua state of its own and may only do what it is granted (see
  [Permissions](#permissions)). Still, `os.getenv` and `os.time`/`clock`/`date` are
  available, and a plugin granted `shell` can run any program: grant it only to plugins you
  trust.

## Events

`helix.on(event, function(event) ... end)` registers a handler; an unknown event name is an
error. The `event` table has a `type` field (the event name) and, depending on the event:

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
| `idle` | | after [`idle-timeout`](./editor.md#editor-section) without input, like Vim's `CursorHold`; runs right away |
| `config_change` | | when the settings change (`:set`, `:config-reload`); once per frame |
| `theme_change` | `theme`, `old_theme` | when the theme changes |
| `cwd_change` | `path`, `old_path` | when the working directory changes (`:cd`) |
| `lsp_initialized` | `language_server_id`, `name` | when a language server has finished initializing |
| `lsp_attach` | `document_id`, `language_server_id`, `name` | when a language server is attached to a buffer, once per server and buffer |
| `lsp_detach` | `document_id`, `language_server_id`, `name` | when a language server stops serving a buffer that stays open |
| `lsp_exit` | `language_server_id`, `name` | when a language server exits |
| `lsp_diagnostic` | `document_id`, `path` | when a buffer's diagnostics change |
| `explorer_select` | `path` | when the cursor of the file explorer moves to another path |
| `file_create` | `path`, `is_dir` | when the file explorer created a file or folder |
| `file_rename` | `path`, `old_path` | when the file explorer renamed or moved (cut and paste) a file or folder |
| `file_copy` | `path`, `source` | when the file explorer copied a file or folder |
| `file_delete` | `path`, `is_dir` | when the file explorer deleted a file or folder |

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

## API reference

Positions: buffer **lines are 1-based** in `get_line`, `insert` and `delete`; char indices,
rows and columns elsewhere are **0-based**. Columns past the end of a line are clamped to the
end of that line (before its line ending), lines past the end to the last line.

Functions that need the editor raise an error when called outside of plugin code run by the
editor; the `helix.ui` messages, `helix.editor.mode`/`*_mode` and the `helix.editor`
functions that run commands then do nothing instead.

Each function needs a [permission](#permissions): the `helix.buffer` and `helix.window`
getters, `helix.lsp`, `helix.on` and the `helix.editor` getters need `read`; `helix.ui`,
`set_status`, `set_annotations` and `set_panel` need `ui`; changes of buffers (`insert`,
`delete`, selections, cursor, modes, registers, undo/redo) need `edit`; `open`, `close`,
`quit` and `window:focus()` need `windows`; `save` and `save_all` need `files`;
`register_command` needs `commands`, and `execute_command` needs `commands` and what the
command needs. `helix.log`, `helix.get_config()`, `helix.permissions()` and
`helix.has_permission()` need none.

### Commands and configuration

- `helix.register_command{ name = ..., doc = ..., args = ..., handler = function(args) end }`
  registers a typable command. `doc` (or `description`) and `args` are shown in the command
  palette; `handler` gets the arguments as a table of strings. Built-in commands of the same
  name take precedence.
- `helix.get_config()`: the `config` table of the calling plugin's `[[plugins.plugins]]`
  entry (nested tables included), or `nil` when it has none.
- `helix.version`: the Helix version string.
- `helix.permissions()`: the permissions of the calling plugin, `{ "read", "ui", ... }`;
  `helix.has_permission(name)`: whether it has that one (an unknown name is an error).
- A command name belongs to the plugin that registered it first: another plugin cannot
  register it again.

### `helix.buffer`

- `get_current()`: the buffer of the focused window.
- `get_by_id(id)`: the buffer with that id, or `nil`.
- `list()`: all open buffers.
- `create(text, language)`: a new scratch buffer holding `text` (both optional), loaded but
  not shown: show it with `window:set_buffer` or `helix.window.split` (`edit`).
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
- `insert(line, col, text)`, `delete(start_line, start_col, end_line, end_col)`,
  `replace(start_line, start_col, end_line, end_col, text)`, `set_text(text)`: change any
  open buffer, shown or not (a reversed range is accepted; a change of a buffer that is not
  focused is an undo step of its own) (`edit`).
- `get_lines(first, last)`: the lines `first` to `last` (1-based, inclusive; `last`
  defaults to the last line, not counting the empty one after a final line ending), without
  line endings; `get_range(start_line, start_col, end_line, end_col)`: the text between.
- `set_signs(list)`: gutter signs of the plugin for this buffer, `{ { line, text, ... } }`
  (`line` 1-based, `text` one cell wide, and a [style](#styles)); drawn in the diagnostics
  column (the `diagnostics` gutter) on lines without a diagnostic. Replaces the plugin's
  signs of the buffer; `set_signs({})` clears them (`ui`).
- `set_highlights(list)`: ranges the plugin highlights with a theme key: `{ { start_line,
  start_col, end_line, end_col, style } }` (positions like `delete`; without columns, whole
  lines) or `{ { from, to, style } }` (char indices); `style` is a theme key (`"warning"`,
  `"diff.plus"`, or one set with `helix.ui.set_style`). Replaces the plugin's highlights of
  the buffer; `set_highlights({})` clears them (`ui`).
- Signs, highlights, annotations and panels move with the text as it is edited: an anchor
  stays with the text after it, a highlight keeps what remains of its text (text typed at
  its edges is not in it; it goes when all of it is deleted), a sign stays on its line.
- `get_info()`: `{ indent_style = "tabs"|"spaces", indent_width, tab_width, line_ending =
  "lf"|"crlf", encoding, readonly, text_width }`.
- `save(force)`: writes the buffer (`files`); `close(force)`: closes it (`force`: even with
  unsaved changes) (`windows`); `set_language(name)`: like `:lang` (`"text"` for none)
  (`config`).
- `set_annotations(list)` (focused buffer only): replaces the calling plugin's annotations
  drawn in the focused window for this buffer; the other plugins' stay. Each annotation has
  `char_idx` (anchor; it picks the line), `text`, and optionally `fg`/`bg` (`"#rrggbb"`),
  `style` (a theme key), `offset` (columns, 0-65535), `is_line` (a virtual line below the
  anchor's line instead of text after the end of the line), `virt_line_idx` (virtual lines
  with the same index share a row) and `dropped_text` (shown instead of `text` when inline
  annotations do not fit and move to a line below). A field of the wrong type is an error.
  Annotations move with the text when it is edited. `set_annotations({})` clears them.
- `clear_annotations()`: removes the calling plugin's annotations of this buffer in every
  window (any buffer, focused or not) (`ui`).
- `find_all(pattern, options)`: the matches of the regex `pattern` in the buffer, like
  Helix's search finds them (multi-line; smart case per `search.smart-case`, unless
  `options.case_insensitive` is `true` or `false`): `{ { from = ..., to = ... }, ... }`
  (char indices, `to` exclusive), at most `options.max` (default 10 000). An invalid pattern
  is an error.

  ```lua
  local buffer = helix.buffer.get_current()
  buffer:set_text("one two\nTwo\n") -- needs `edit`
  local matches = buffer:find_all("two")
  assert(type(matches) == "table" and #matches == 2)
  assert(math.type(matches[1].from) == "integer" and matches[1].from == 4)
  assert(matches[1].to == 7 and matches[2].from == 8)
  local exact = buffer:find_all("Two") -- an uppercase letter: the case matters
  assert(#exact == 1 and exact[1].from == 8)
  ```
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
    `bg`), `left_cap`/`right_cap` (drawn in `bg` around each row, e.g. powerline caps),
    `min_width` (`30`) and `show_in_insert` (`false`: hidden in insert mode).
  - The anchor moves with the text when it is edited.

### `helix.editor`

- `mode()`: `"normal"`, `"insert"` or `"select"`; `normal_mode()`, `insert_mode()`,
  `select_mode()` switch modes.
- `get_cursor()`: `{row, col}` of the primary cursor (0-based); `set_cursor(row, col)`.
- `move_cursor(direction, count)`: `"left"`, `"right"`, `"up"` or `"down"`, like `h`/`l`/`k`/`j`
  (extends the selections in select mode).
- `get_selections()`: the `{anchor, head}` char indices of the focused window's
  selections, and the index of the primary one (1-based); `set_selections(list, primary)`
  (clamped to the text; `primary` defaults to 1); `select_all()`.
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
- `last_search()`: the pattern of the last search (`/`, `n`, `*`, ...), or `nil`.
- `set_status(message, level)`: like `helix.ui.notify`.
- `run(name, count)`: runs a command of key bindings (`"move_line_down"`,
  `"goto_definition"`, `"file_picker"`; see the command palette), `count` times, like
  typing the count first. It needs `commands` and what the command needs: most need `edit`;
  `shell_*`, `dap_*`, `replay_macro` and `code_lens` need `shell`; splits, window moves and
  the commands that open other files need `windows`; pickers and popups need `ui`; the
  `explorer_*` ones `explorer`. Commands that wait for a key (`f`, `r`, ...) cannot be run.
- `get_option(key)`: a setting of `[editor]` as a Lua value (`"text-width"`,
  `"soft-wrap.enable"`, `"lsp"`), `nil` for none (`read`); `set_option(key, value)`: like
  `:set key value`, `value` a string as typed or a Lua value (`config`; the settings that
  run programs need `shell`).
- `get_cwd()` (`read`), `set_cwd(path)`: the working directory, like `:cd` (`config`).

### `helix.ui`

- `notify(message, level)`: a message; `level` is `"info"` (default), `"warning"` or
  `"error"`. `info(message)`, `warn(message)`, `error(message)`, `set_status(message)`.
- `prompt(message, default, function(input) end)`: asks for a line of text; the callback is
  not called when the prompt is cancelled.
- `confirm(message, function(confirmed) end)`: a yes/no question (`false` when cancelled).
- `picker{ items, on_select, title, prompt, columns, preview, dock }`: a picker over `items`
  and returns its handle. An item is a string, or a table with `text` (or `columns`, the text
  of each column), `path` or `buffer` to preview, `line`/`end_line` (1-based) to show in the
  preview, and anything else the plugin wants back (`data`, say). `on_select(item, action)`
  (required) gets the item chosen, the very value from `items`, and how: `"replace"`
  (`Enter`), `"hsplit"` (`C-s`), `"vsplit"` (`C-v`). `title` defaults to `Plugin`,
  `prompt` to `"Select:"`; `columns` names up to 8 columns (one without a header when
  omitted); `preview` defaults to whether an item has a `path` or `buffer`; `dock = true`
  shows it in the quickfix panel at the bottom instead of over the editor.
  - `handle:add(items)` adds items to the open picker (and to `items`, so they come back to
    `on_select` too); `handle:close()` closes it without a choice.
- `menu(items, function(item, action) end)`: the same picker with the default title and
  label.
- `set_style(scope, style)`: sets the style of a theme key (`"ui.statusline"`, `"keyword"`,
  `"diagnostic.error"`, or a new one like `"plugin.mark"` for `set_highlights`), written like
  in a theme ([styles](#styles)); it stays when the theme changes. `reset_style(scope)` brings
  the theme's own back.
- `get_theme()`, `set_theme(name)`, `get_terminal_size()` (`{width, height}` of the editor
  area), `redraw()`.
- `popup{ text | lines | markdown, title, position, max_width, max_height, persistent }`: a
  popup with plain `text`, styled `lines` (each line a string, or a list of strings and
  `{ text, ... }` segments with a [style](#styles)), or `markdown` (drawn like the hover
  documentation). `position` is `"cursor"` (default; next to it, like hover), `"center"`, or
  `{ row, col }` (its top left corner on the screen, 0-based). `max_width` and `max_height`
  limit its size (columns, rows of content). It closes with `Esc` and, unless `persistent`,
  with the next key; a persistent popup lets the keys through to the editor. Returns a
  handle: `handle:update(options)` shows other content (the same options),
  `handle:close()` closes it. One plugin popup is shown at a time: a new one replaces it
  (`ui`).
- `show_help()` is not implemented (raises an error).

Callbacks run later, after the current plugin code has returned. A prompt or picker that is
closed without a choice releases its callback without calling it (except `confirm`, which
gets `false`).

### `helix.window`

- `get_current()`, `list()`, `get_by_id(id)` (or `nil`): windows (`read`).
- `split(direction, buffer)`: splits the focused window (`"vertical"` or `"horizontal"`),
  showing `buffer` (default: the same one); returns the new window, which has the focus
  (`windows`).

Window methods (they raise an error once the window is closed):

- `id()` (the `window_id` of events), `get_buffer()`, `is_focused()`, `is_valid()`,
  `get_area()` (`{ x, y, width, height }` on the screen), `get_cursor()` (`{ row, col }`,
  0-based), `get_selections()` (and the primary index), `get_top_line()` (the first line
  shown, 0-based) (`read`).
- `set_cursor(row, col)`, `set_selections(list, primary)` (`edit`).
- `focus()`, `scroll_to(line)` (the first line shown), `set_buffer(buffer)`, `close()` (not
  the last window: use `helix.editor.quit`) (`windows`).

### `helix.keymap`

- `set(modes, keys, action, opts)`: binds `keys` in `modes` (`"normal"`, `"insert"`,
  `"select"` or a list of them) to `action`, a command as in `[keys]` of `config.toml`
  (`"move_line_down"`, `":write"`, `"@ihello<esc>"`, `"no_op"` to unbind) or a Lua function.
  `keys` is written like a macro: `"<space>z"`, `"gx"`, `"<C-s>"` (one named key may leave
  out the brackets: `"C-s"`). `opts.desc` is what the key help shows. It needs `keymaps`,
  and for a command what the command needs when the key is pressed (`":sh ..."` needs
  `shell`, a macro every permission): a plugin cannot make a key do what it may not do
  itself. A function runs as the plugin's code, with its permissions. A plugin may bind its
  own `:` commands, not another plugin's.
- `del(modes, keys)`: removes a binding the plugin set; the configured one is back. Returns
  whether there was one (`keymaps`).
- `list()`: the bindings of all plugins: `{ { plugin, mode, keys, command, desc }, ... }`
  (`read`).

The bindings go over the keys of `config.toml` (also after `:config-reload`); a later
binding of the same keys replaces an earlier one, whichever plugin set it.
`:reload-all-plugins` removes them all before loading the plugins again.

### `helix.statusline`

Texts in the status line, drawn by Helix every frame (no plugin code runs while drawing).

- `set(id, { text, position, buffer, priority, ... })`: shows the plugin's segment `id`, or
  changes it (set it again with another `text` to update it). `position` is `"left"` (after
  the configured left elements), `"center"` or `"right"` (default; before the configured right
  elements). With `buffer`, it is shown only in the status lines of windows showing that
  buffer. Segments of a position are in `priority` order (default 0, lower first). A
  [style](#styles) goes over the status line's (`ui.statusline`) (`ui`).
- `remove(id)`: removes it, returns whether there was one; `clear()`: removes all of the
  plugin's (`ui`).
- `list()`: the segments of all plugins: `{ { plugin, id, text, position, priority } }`
  (`read`).

```lua
-- the time of the last save, and a counter of the changes since
local changes = 0
helix.on("buffer_changed", function()
    changes = changes + 1
    assert(math.type(changes) == "integer" and changes >= 1)
    helix.statusline.set("changes", { text = "Δ" .. changes, style = "warning" })
end)
helix.on("buffer_post_save", function()
    changes = 0
    local time = os.date("%H:%M")
    assert(type(time) == "string" and time:match("^%d%d:%d%d$"))
    helix.statusline.set("changes", { text = "saved " .. time, fg = "green" })
end)
```

### Styles

A style is written like in a theme: `fg` and `bg` (a color name such as `"red"` or
`"light-blue"`, or `"#rrggbb"`), `modifiers` (`{ "bold", "italic", "underlined", ... }`),
`underline` (`{ style = "curl", color = "red" }`), and `style`, a theme key the others go over:
`{ style = "comment", modifiers = { "bold" } }`.

### `helix.quickfix`

The quicklist, shown in the quickfix panel at the bottom (`:copen`, see
[Quicklist](./quicklist.md)).

- `set(items, open)`: replaces the quicklist with `items`, each `{ path = ... }` or
  `{ buffer = ... }` with `line`, `end_line` (1-based; `line` defaults to 1) and `text`;
  shows it in the panel when `open` is true (`edit`, and `ui` to open).
- `add(item)`, `clear()` (`edit`); `get()`: the entries, as given to `set` (`read`).
- `open()`: shows the quicklist in the panel; `close()`: closes the panel (`ui`);
  `is_open()` (`read`).

A plugin's own list can also go in the panel: `helix.ui.picker{ ..., dock = true }`.

### `helix.explorer`

- `is_open()`, `is_focused()`, `get_root()`, `get_selected()` (the path under the cursor;
  `nil` until the explorer was shown) (`read`).
- `open()`, `close()`, `focus()`, `refresh()` (reads the folders again), `reveal(path)` (shows
  the explorer with the cursor on `path`, the folders above it expanded) (`explorer`).
- `decorate(path, { text, fg, style })`: a text at the right of the row of `path`, left of the
  git marker, in `fg` (`"#rrggbb"`) or the theme key `style`; `decorate(path, nil)` removes it;
  `clear_decorations()` removes the calling plugin's (`ui`).

What the explorer does to files is heard of with the `file_create`, `file_rename`,
`file_copy` and `file_delete` events, and its cursor with `explorer_select`.

### Timers

- `helix.defer(ms, fn)`: runs `fn` once, `ms` milliseconds from now (`0`: as soon as the
  running plugin code has returned); returns the timer's id.
- `helix.every(ms, fn)`: runs `fn` every `ms` milliseconds (10 at least) until cancelled;
  returns its id. While the editor is busy, a firing that is due waits, and the ones after it
  are not queued up behind it.
- `helix.cancel(id)`: stops a timer of the plugin; returns whether it was running.

The functions run on the editor's thread like event handlers (with the editor and the time
limit). `:reload-all-plugins` stops every timer.

### `helix.process`

Programs run in the background; their result comes to a callback (`shell`).

- `run({ program, arg, ... }, options, function(result) end)`: runs `program` with the
  arguments (no shell in between). `options` (may be left out) has `cwd`, `stdin` (a string
  written to the program's input), `env` (`{ NAME = "value" }`, added to Helix's environment)
  and `timeout` (milliseconds, default 60000; the program is killed after it).
- `shell(command_line, options, function(result) end)`: the same with the configured shell
  (`shell` of `[editor]`, `sh -c` by default).

`result` is `{ success, code, stdout, stderr, timed_out, truncated, error }`: `code` is the
exit code (`nil` when the program was killed by a signal), `stdout` and `stderr` are its
output (each cut at 16 MiB, then `truncated` is true), `error` says why the program could not
run (`success` is false then).

```lua
helix.register_command{ name = "git-branch", handler = function()
    helix.process.run({ "git", "branch", "--show-current" }, function(result)
        assert(type(result.success) == "boolean" and type(result.stdout) == "string")
        if result.success then
            local branch = result.stdout:gsub("%s+$", "")
            assert(type(branch) == "string")
            -- (empty on a detached HEAD)
            if branch ~= "" then
                helix.statusline.set("branch", { text = branch, position = "left" })
            end
        end
    end)
end }
```

### `helix.lsp`

- `get_clients()`: `{ {name = ..., id = ...}, ... }` for all running language servers.
- `request(method, params, options, function(result, error) end)`: sends a request to a
  language server of `options.buffer` (default: the focused buffer), the one named
  `options.server` or its first (`options` may be left out). The answer comes to the callback:
  `result` (`nil` for none) or `error` (a string). Requests that only read (`textDocument/*`,
  `workspace/symbol`) need `read`; the others need `shell`.
- `notify(method, params, options)`: sends a notification (`shell`).
- `position_params(options)`: `{ textDocument = { uri }, position = { line, character } }` of
  the focused window's cursor, in the position encoding of the server (`options.server`, or
  the first): the params of most `textDocument/*` requests.

```lua
helix.register_command{ name = "hover-status", handler = function()
    helix.lsp.request("textDocument/hover", helix.lsp.position_params(), function(result, err)
        assert(err == nil or type(err) == "string")
        if result then
            local contents = result.contents
            local text = type(contents) == "table" and (contents.value or contents[1]) or contents
            assert(type(text) == "string")
            helix.ui.popup{ markdown = text }
        end
    end)
end }
```

### `helix.log`

- `info(msg)`, `warn(msg)`, `error(msg)`, `debug(msg)`, `trace(msg)`: write to the Helix log
  (`:log-open`; start Helix with `hx -v` for debug messages).

## Search lenses (`hlslens`)

`contrib/plugins/hlslens` shows, after a search (`/`, `?`, `n`, `N`, `*`, ...), a lens at the
end of the line of each match in view: `[3/12]` for the nearest match (its index and the
number of matches) and `[5]` for the others, with the matches highlighted. The lenses follow
the cursor, scrolling and edits until `:hlslens-stop` (like Vim's `:nohlsearch`) or the next
search. Its commands need the `commands` permission:

```toml
[[plugins.plugins]]
name = "hlslens"
permissions = ["commands"]   # :hlslens-stop, -start, -toggle, -enable, -disable
config = { calm_down = true, near_lens = "[{index}/{total}]", lens = "[{arrow}{index}]" }
```

| Setting | Description | Default |
| --- | --- | --- |
| `auto_enable` | Lenses after every search; `false`: only after `:hlslens-enable` | `true` |
| `calm_down` | Remove them once the cursor leaves the matches, or on an edit | `false` |
| `nearest_only` | A lens on the nearest match only | `false` |
| `near_lens`, `lens` | The text of the nearest match's lens and of the others': `{index}`, `{total}`, and `{arrow}` (`▲` above the nearest match, `▼` below) | `"[{index}/{total}]"`, `"[{index}]"` |
| `near_lens_style`, `lens_style` | Their theme keys | `"ui.cursor.match"`, `"ui.virtual"` |
| `near_match_style`, `match_style` | The theme keys of the nearest match and the others (`""`: not highlighted) | `"ui.cursor.match"`, `"ui.highlight"` |
| `padding` | Columns between the end of the line and the lens, beyond the one Helix leaves | `0` |
| `max_matches` | Matches counted at most (the total then reads `10000+`) | `10000` |

A line with several matches has one lens: the nearest match's, or its first match's. Helix
does not tell plugins what is typed in the search prompt, so the lenses show once the search
is done (not while typing it).
