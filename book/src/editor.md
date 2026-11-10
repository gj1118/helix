## Editor

- [`[editor]` Section](#editor-section)
- [`[editor.bufferline]` Section](#editorbufferline-section)
- [`[editor.inline-completion]` Section](#editorinline-completion-section)
- [`[editor.breadcrumb]` Section](#editorbreadcrumb-section)
- [`[editor.clipboard-provider]` Section](#editorclipboard-provider-section)
- [`[editor.statusline]` Section](#editorstatusline-section)
- [`[editor.lsp]` Section](#editorlsp-section)
- [`[editor.cursor-shape]` Section](#editorcursor-shape-section)
- [`[editor.file-picker]` Section](#editorfile-picker-section)
- [`[editor.recent-picker]` Section](#editorrecent-picker-section)
- [`[editor.file-explorer]` Section](#editorfile-explorer-section)
- [`[editor.explorer]` Section](#editorexplorer-section)
- [`[editor.cmdline]` Section](#editorcmdline-section)
- [`[editor.quickfix]` Section](#editorquickfix-section)
- [`[editor.notifications]` Section](#editornotifications-section)
- [`[editor.inline-blame]` Section](#editorinline-blame-section)
- [`[editor.gradient-borders]` Section](#editorgradient-borders-section)
- [`[editor.buffer-picker]` Section](#editorbuffer-picker-section)
- [`[editor.auto-pairs]` Section](#editorauto-pairs-section)
- [`[editor.auto-reload]` Section](#editorauto-reload-section)
- [`[editor.auto-save]` Section](#editorauto-save-section)
- [`[editor.search]` Section](#editorsearch-section)
- [`[editor.whitespace]` Section](#editorwhitespace-section)
- [`[editor.indent-guides]` Section](#editorindent-guides-section)
- [`[editor.gutters]` Section](#editorgutters-section)
  - [`[editor.gutters.line-numbers]` Section](#editorguttersline-numbers-section)
  - [`[editor.gutters.diagnostics]` Section](#editorguttersdiagnostics-section)
  - [`[editor.gutters.diff]` Section](#editorguttersdiff-section)
  - [`[editor.gutters.spacer]` Section](#editorguttersspacer-section)
  - [`[editor.gutters.code-action-hint]` Section](#editorgutterscode-action-hint-section)
- [`[editor.debugger]` Section](#editordebugger-section)
- [`[editor.image-preview]` Section](#editorimage-preview-section)
- [`[editor.soft-wrap]` Section](#editorsoft-wrap-section)
- [`[editor.smart-tab]` Section](#editorsmart-tab-section)
- [`[editor.inline-diagnostics]` Section](#editorinline-diagnostics-section)
- [`[editor.word-completion]` Section](#editorword-completion-section)
- [`[editor.completion-highlight]` Section](#editorcompletion-highlight-section)
- [`[editor.scrolloff]` Section](#editorscrolloff-section)
- [`[editor.workspace-trust]` Section](#editorworkspace-trust-section)
- [`[editor.persistence]` Section](#editorpersistence-section)
- [Window resizing and focus mode](#window-resizing-and-focus-mode)
- [Welcome screen](#welcome-screen)
- [Window title](#window-title)
- [Ruler character](#ruler-character)

### `[editor]` Section

| Key | Description | Default |
|--|--|---------|
| `mouse` | Enable mouse mode | `true` |
| `mouse-yank-register` | Which register to use for mouse yanks. | `*` |
| `middle-click-paste` | Middle click paste support | `true` |
| `default-yank-register` | Default register used for yank/paste | `'"'` |
| `scroll-lines` | Number of lines to scroll per scroll wheel step | `3` |
| `shell` | Shell to use when running external commands | Unix: `["sh", "-c"]`<br/>Windows: `["cmd", "/C"]` |
| `line-number` | Line number display: `absolute` simply shows each line's number, while `relative` shows the distance from the current line. When unfocused or in insert mode, `relative` will still show absolute line numbers | `"absolute"` |
| `cursorline` | Highlight all lines with a cursor | `false` |
| `cursorcolumn` | Highlight all columns with a cursor | `false` |
| `continue-comments` | if helix should automatically add a line comment token if you create a new line inside a comment. | `true` |
| `gutters` | Gutters to display: Available are `diagnostics` and `diff` and `line-numbers` and `spacer` and `code-action-hint`, note that `diagnostics` also includes other features like breakpoints, 1-width padding will be inserted if gutters is non-empty | `["diagnostics", "spacer", "line-numbers", "spacer", "diff"]` |
| `auto-completion` | Enable automatic pop up of auto-completion | `true` |
| `path-completion` | Enable filepath completion. Show files and directories if an existing path at the cursor was recognized, either absolute or relative to the current opened document or current working directory (if the buffer is not yet saved). Defaults to true. | `true` |
| `auto-format` | Enable automatic formatting on save[^3] | `true` |
| `idle-timeout` | Time in milliseconds since last keypress before idle timers trigger. | `250` |
| `completion-timeout` | Time in milliseconds after typing a word character before completions are shown, set to 5 for instant.  | `250` |
| `preview-completion-insert` | Whether to apply completion item instantly when selected | `true` |
| `completion-trigger-len` | The min-length of word under cursor to trigger autocompletion | `2` |
| `snippet-completion` | Offer [snippets](./snippets.md) in the completion menu when their prefix is typed | `true` |
| `completion-replace` | Whether to make completions always replace the entire word and not just the part before the cursor | `false` |
| `auto-info` | Whether to display info boxes | `true` |
| `true-color` | Whether to override automatic detection of terminal truecolor support in the event of a false negative | `false` |
| `undercurl` | Whether to override automatic detection of terminal undercurl support in the event of a false negative | `false` |
| `rulers` | List of column positions at which to display the rulers. Can be overridden by language specific `rulers` in `languages.toml` file | `[]` |
| `bufferline` | A line at the top of the editor showing the open buffers. A table, see [`[editor.bufferline]`](#editorbufferline-section), or just the render mode as a string (`"always"`, `"never"`, `"multiple"`) | `"never"` |
| `color-modes` | Whether to color the mode indicator with different colors depending on the mode itself | `false` |
| `text-width` | Maximum line length. Used for the `:reflow` command and soft-wrapping if `soft-wrap.wrap-at-text-width` is set | `80` |
| `fold-textobjects` | Text objects folded automatically when a document is opened, such as `["comment"]`. See [Folding](./textobjects.md#folding-text-objects) | `[]` |
| `workspace-lsp-roots` | Directories relative to the workspace root that are treated as LSP roots. Should only be set in `.helix/config.toml` | `[]` |
| `default-line-ending` | The line ending to use for new documents. Can be `native`, `lf`, `crlf`, `ff`, `cr` or `nel`. `native` uses the platform's native line ending (`crlf` on Windows, otherwise `lf`). | `"native"` |
| `insert-final-newline` | Whether to automatically insert a trailing line-ending on write if missing | `true` |
| `atomic-save` | Whether to use atomic operations to write documents to disk. This prevents data loss if the editor is interrupted while writing the file, but may confuse some file watching/hot reloading programs. | `true` |
| `trim-final-newlines` | Whether to automatically remove line-endings after the final one on write | `false` |
| `trim-trailing-whitespace` | Whether to automatically remove whitespace preceding line endings on write | `false` |
| `popup-border` | Draw border around `popup`, `menu`, `all`, or `none`. Hover, signature help, the completion menu (and its documentation) and the blame popup always have a border | `"none"` |
| `rounded-corners` | Draw borders with rounded corners. Superseded by `border-style`: when that is set, this is ignored (`rounded-corners = true` is the same as `border-style = "rounded"`) | `false` |
| `border-style` | Glyphs of panel, popup, menu and notification borders and of the separators inside panels: `"none"`, `"plain"`, `"rounded"`, `"double"`, `"thick"`, `"quadrant-inside"` or `"quadrant-outside"`. `"none"` draws nothing but keeps the space of the border, and separators stay plain lines. `"plain"` draws thin lines along the outer edge of the border cells (`▏ ▔ ▕ ▁` with the square corners `🭽 🭾 🭼 🭿`), so the popup's background reaches right up to the line, with no gap inside it and nothing beyond it; the corners need a font or terminal that draws Unicode's "Symbols for Legacy Computing". The other styles run through the middle of their cells, which then keep the background behind the popup. Takes priority over `rounded-corners` (`rounded-corners = true` is the same as `border-style = "rounded"`). Gradient borders follow `"none"` and `"rounded"` only; their line weight comes from `gradient-borders.thickness`. Without gradient borders, all of these borders have the same color, see `ui.popup.border` in [themes](./themes.md). | `"plain"` |
| `indent-heuristic` | How the indentation for a newly inserted line is computed: `simple` just copies the indentation level from the previous line, `tree-sitter` computes the indentation based on the syntax tree and `hybrid` combines both approaches. If the chosen heuristic is not available, a different one will be used as a fallback (the fallback order being `hybrid` -> `tree-sitter` -> `simple`). | `"hybrid"` |
| `jump-label-alphabet` | The characters that are used to generate two character jump labels. Characters at the start of the alphabet are used first. | `"abcdefghijklmnopqrstuvwxyz"` |
| `end-of-line-diagnostics` | Minimum severity of diagnostics to render at the end of the line. Set to `disable` to disable entirely. Refer to the setting about `inline-diagnostics` for more details | `"hint"` |
| `clipboard-provider` | Which API to use for clipboard interaction. One of `pasteboard` (MacOS), `wayland`, `x-clip`, `x-sel`, `win32-yank`, `termux`, `tmux`, `windows`, `termcode`, `none`, or a custom command set. | Platform and environment specific. |
| `editor-config` | Whether to read settings from [EditorConfig](https://editorconfig.org) files | `true` |
| `rainbow-brackets` | Whether to render rainbow colors for matching brackets. Requires tree-sitter `rainbows.scm` queries for the language. | `false` |
| `kitty-keyboard-protocol` | Whether to enable Kitty Keyboard Protocol. Can be `enabled`, `disabled` or `auto` | `"auto"` |
| `terminal` | Where a debuggee that asks for a terminal is started: `launcher` or `command`/`args`, see [The terminal of the debuggee](./debugging.md#the-terminal-of-the-debuggee) | detected |
| `picker-symbol` | Drawn before the selected row of pickers and of the command line popup's completions | `">"` |
| `fullscreen-overlay` | Pickers (and the command palette) take the whole screen on terminals narrower than 200 columns, instead of 90% of it. The file picker with [`file-picker.hide-preview`](#editorfile-picker-section) keeps its own size | `false` |
| `welcome-screen` | Show the [welcome screen](#welcome-screen) when Helix starts without a file | `true` |
| `set-title` | Set the window title of the terminal, see [Window title](#window-title) | `false` |
| `title-format` | The window title when `set-title` is on; supports [expansions](./command-line.md#expansions) | `"hx %{buffer_name}:%{cursor_line}:%{cursor_column}"` |
| `ruler-char` | Character used to draw the rulers, see [Ruler character](#ruler-character) | `""` |
| `max-panel-width`, `max-panel-height`, `max-panel-width-percent`, `max-panel-height-percent` | Limits of window resizing, see [Window resizing](#window-resizing-and-focus-mode) | `50`, `50`, `0.8`, `0.8` |

[^3]: In most cases, you also need to enable the `auto-format` setting under `languages.toml`. You can find the reasoning [here](https://github.com/helix-editor/helix/discussions/9043#discussioncomment-7811497).

### `[editor.bufferline]` Section

The buffer line is a line at the top of the editor with a label per open buffer. It scrolls
to keep the current buffer in view, and buffers with the same file name are told apart by
their path.

| Key | Description | Default |
|--|--|---------|
| `render-mode` | When to show it: `"always"`, `"never"` or `"multiple"` (only when more than one buffer is open) | `"never"` |
| `separator` | Text drawn between two labels | `"│"` |

```toml
[editor.bufferline]
render-mode = "multiple"
separator = "│"
```

The upstream string form is accepted too: `bufferline = "multiple"` (or `"always"`,
`"never"`) under `[editor]` sets `render-mode` and keeps the other keys at their defaults.

### `[editor.inline-completion]` Section

Language servers that support `textDocument/inlineCompletion` (LSP 3.18) can show suggestions as faint text after
the cursor. The suggestion is only inserted when accepted, and disappears when you edit, move the cursor, leave insert
mode or open the completion menu. Add the `inline-completion` feature to a language server's `only-features` /
`except-features` to route it (see [languages](./languages.md#configuring-language-servers-for-a-language)).
Snippet suggestions and multiple cursors are not supported.

|      Setting      |                                      Description                                      | Default |
|-------------------|---------------------------------------------------------------------------------------|---------|
| `auto-trigger`    | Ask for suggestions while typing. When off, only `inline_completion_trigger` asks      | `true`  |
| `trigger-delay`   | Milliseconds after typing before a suggestion is requested                            | `150`   |

There are no default keys. Add these to `config.toml`:

```toml
[keys.insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_prev"
```

The preview uses the `ui.virtual.inline-completion` theme scope, falling back to `ui.virtual.inlay-hint`.

### `[editor.breadcrumb]` Section

Allows configuration of the breadcrumb navigation bar.

|       Setting        |                                   Description                                    | Default |
|----------------------|----------------------------------------------------------------------------------|---------|
| `enable`             | Whether to enable the breadcrumb navigation bar                                  | `false` |
| `path`               | Selecting how much path information is shown                                     | `"full"`|
| `max-depth`          | Maximum number of symbol levels shown; `0` means unlimited                       | `8`     |
| `max-name-length`    | Maximum length of a single symbol name, truncated in the middle; `0` unlimited   | `32`    |

```toml
[editor.breadcrumb]
enable = true
# full: helix-term > src > commands > typed.rs > TypeableCommand > name
# file: typed.rs > TypeableCommand > name
# none: TypeableCommand > name
path = "full|file|none"
max-depth = 8
max-name-length = 32
```

The bar is displayed at the top of the view.

The symbol trail comes from the first language server that supports document
symbols. When no such language server is available (or it only returns a flat
symbol list), the trail is derived from tree-sitter tag queries (`tags.scm`)
instead. When more symbols are nested than `max-depth`, the outermost symbols
are elided with a leading `…`.

If icon support is enabled via `[icons.kind]`, an icon is shown before each
symbol name.

### `[editor.clipboard-provider]` Section

Helix can be configured either to use a builtin clipboard configuration or to use
a provided command.

For instance, setting it to use OSC 52 termcodes, the configuration would be:
```toml
[editor]
clipboard-provider = "termcode"
```

Alternatively, Helix can be configured to use arbitrary commands for clipboard integration:

```toml
[editor.clipboard-provider.custom]
yank = { command = "cat",  args = ["test.txt"] }
paste = { command = "tee",  args = ["test.txt"] }
primary-yank = { command = "cat",  args = ["test-primary.txt"] } # optional
primary-paste = { command = "tee",  args = ["test-primary.txt"] } # optional
```

For custom commands the contents of the yank/paste is communicated over stdin/stdout.

### `[editor.statusline]` Section

Allows configuring the statusline at the bottom of the editor.

The configuration distinguishes between three areas of the status line:

`[ ... ... LEFT ... ... | ... ... ... CENTER ... ... ... | ... ... RIGHT ... ... ]`

Statusline elements can be defined as follows:

```toml
[editor.statusline]
left = ["mode", "spinner"]
center = ["file-name"]
right = ["diagnostics", "selections", "position", "file-encoding", "file-line-ending", "file-type"]
separator = "│"
mode.normal = "NORMAL"
mode.insert = "INSERT"
mode.select = "SELECT"
diagnostics = ["warning", "error"]
workspace-diagnostics = ["warning", "error"]
```
The `[editor.statusline]` key takes the following sub-keys:

| Key           | Description | Default |
| ---           | ---         | ---     |
| `left`        | A list of elements aligned to the left of the statusline | `["mode", "spinner", "file-name", "read-only-indicator", "file-modification-indicator"]` |
| `center`      | A list of elements aligned to the middle of the statusline | `[]` |
| `right`       | A list of elements aligned to the right of the statusline | `["debug-session", "diagnostics", "selections", "register", "position", "file-encoding"]` |
| `separator`   | The character used to separate elements in the statusline | `"│"` |
| `mode.normal` | The text shown in the `mode` element for normal mode | `"NOR"` |
| `mode.insert` | The text shown in the `mode` element for insert mode | `"INS"` |
| `mode.select` | The text shown in the `mode` element for select mode | `"SEL"` |
| `diagnostics` | A list of severities which are displayed for the current buffer | `["warning", "error"]` |
| `workspace-diagnostics` | A list of severities which are displayed for the workspace | `["warning", "error"]` |

The following statusline elements can be configured:

| Key    | Description |
| ------ | ----------- |
| `mode` | The current editor mode (`mode.normal`/`mode.insert`/`mode.select`) |
| `spinner` | A progress spinner indicating LSP activity |
| `file-name` | The path/name of the opened file |
| `file-absolute-path` | The absolute path/name of the opened file |
| `file-base-name` | The basename of the opened file |
| `current-working-directory` | The current working directory  |
| `file-modification-indicator` | The indicator to show whether the file is modified (a `[+]` appears when there are unsaved changes) |
| `file-encoding` | The encoding of the opened file if it differs from UTF-8 |
| `file-line-ending` | The file line endings (CRLF or LF) |
| `file-indent-style` | The file indentation style |
| `read-only-indicator` | An indicator that shows `[readonly]` when a file cannot be written |
| `total-line-numbers` | The total line numbers of the opened file |
| `file-type` | The type of the opened file |
| `diagnostics` | The number of warnings and/or errors |
| `workspace-diagnostics` | The number of warnings and/or errors on workspace |
| `selections` | The primary selection index out of the number of active selections |
| `primary-selection-length` | The number of characters currently in primary selection |
| `position` | The cursor position |
| `position-percentage` | The cursor position as a percentage of the total number of lines |
| `separator` | The string defined in `editor.statusline.separator` (defaults to `"│"`) |
| `spacer` | Inserts a space between elements (multiple/contiguous spacers may be specified) |
| `version-control` | The current branch name or detached commit hash of the opened workspace |
| `register` | The current selected register |
| `code-action-hint` | Indicator for when code actions are available |
| `function-name` | The name of the function, method or closure the cursor is in, found with tree-sitter (with the `function` icon of `[icons.kind]` when enabled, see [Icons](./configuration.md#icons)) |
| `debug-session` | The selected [debug session](./debugging.md#several-sessions): its name, whether it is stopped, and the number of sessions |

### `[editor.lsp]` Section

| Key                   | Description                                                 | Default |
| ---                   | -----------                                                 | ------- |
| `enable`              | Enables LSP integration. Setting to false will completely disable language servers regardless of language settings.| `true` |
| `display-messages`    | Display LSP `window/showMessage` messages below statusline[^1] | `true` |
| `display-progress-messages` | Display LSP progress messages below statusline[^1]    | `false` |
| `auto-signature-help` | Enable automatic popup of signature help (parameter hints)  | `true`  |
| `signature-help-position` | Place the signature help popup `"above"` or `"below"` the cursor | `"above"` |
| `auto-document-highlight` | Automatically highlight symbol references at the cursor | `false` |
| `display-inlay-hints` | Display inlay hints[^2]                                     | `false` |
| `inlay-hints-length-limit` | Maximum displayed length (non-zero number) of inlay hints | Unset by default  |
| `display-color-swatches` | Show color swatches next to colors | `true` |
| `color-swatches-string` | Symbol drawn for each color swatch. Any string, e.g. `"●"`, `"◆"`, `"⬢"` | `"■"` |
| `display-code-lenses` | Show [code lenses](./lsp.md#code-lenses) (run, debug, test, reference counts, ...) from the language server at the end of the lines they belong to. When off, no code lens request is sent. | `false` |
| `display-signature-help-docs` | Display docs under signature help popup             | `true`  |
| `snippets`      | Enables snippet completions. Requires a server restart (`:lsp-restart`) to take effect after `:config-reload`/`:set`. | `true`  |
| `goto-reference-include-declaration` | Include declaration in the goto references popup. | `true`  |
| `workspace-diagnostics` | Show diagnostics for the whole workspace in the workspace diagnostics picker, asking language servers for diagnostics of files that are not open. | `true` |

[^1]: By default, a progress spinner is shown in the statusline beside the file path.

[^2]: You may also have to activate them in the language server config for them to appear, not just in Helix. Inlay hints in Helix are still being improved on and may be a little bit laggy/janky under some circumstances. Please report any bugs you see so we can fix them!

### `[editor.cursor-shape]` Section

Defines the shape of cursor in each mode.
Valid values for these options are `block`, `bar`, `underline`, or `hidden`.

> 💡 Due to limitations of the terminal environment, only the primary cursor can
> change shape.

| Key      | Description                                | Default   |
| ---      | -----------                                | -------   |
| `normal` | Cursor shape in [normal mode][normal mode] | `"block"` |
| `insert` | Cursor shape in [insert mode][insert mode] | `"block"` |
| `select` | Cursor shape in [select mode][select mode] | `"block"` |

[normal mode]: ./keymap.md#normal-mode
[insert mode]: ./keymap.md#insert-mode
[select mode]: ./keymap.md#select--extend-mode

### `[editor.file-picker]` Section

Set options for file picker and global search. Ignoring a file means it is
not visible in the Helix file picker and global search.

All git related options are only enabled in a git repository.

| Key | Description | Default |
|--|--|---------|
|`hidden` | Enables ignoring hidden files | `true`
|`follow-symlinks` | Follow symlinks instead of ignoring them | `true`
|`deduplicate-links` | Ignore symlinks that point at files already shown in the picker | `true`
|`parents` | Enables reading ignore files from parent directories | `true`
|`ignore` | Enables reading `.ignore` files | `true`
|`git-ignore` | Enables reading `.gitignore` files | `true`
|`git-global` | Enables reading global `.gitignore`, whose path is specified in git's config: `core.excludesfile` option | `true`
|`git-exclude` | Enables reading `.git/info/exclude` files | `true`
|`max-depth` | Set with an integer value for maximum depth to recurse | Unset by default
|`hide-preview` | Hide the preview panel in the file picker, to save space or improve performance | `false` |

Ignore files can be placed locally as `.ignore` or put in your home directory as `~/.ignore`. They support the usual ignore and negative ignore (unignore) rules used in `.gitignore` files.

Additionally, you can use Helix-specific ignore files by creating a local `.helix/ignore` file in the current workspace or a global `ignore` file located in your Helix config directory:
- Linux and Mac: `~/.config/helix/ignore`
- Windows: `%AppData%\helix\ignore`

Example:

```ini
# unignore in file picker and global search
!.github/
!.gitignore
!.gitattributes
```

### `[editor.recent-picker]` Section

The recent file picker (`Space O`) lists the files you opened or saved most recently, in this
editor or another one running, most recent first. They are kept in a small file of the cache
directory, written in the background (opening and saving never wait for it), merged with what
the other editors wrote, and replaced atomically.

| Key | Description | Default |
|--|--|---------|
| `enable` | Note the files opened and saved (off, the picker shows what was noted before) | `true` |
| `max-show-entries` | The most files the picker shows | `50` |
| `max-save-entries` | The most files kept | `100` |
| `recents-path` | The file they are kept in | `recents.txt` in the cache directory (`~/.cache/helix` on Linux) |

```toml
[editor.recent-picker]
max-show-entries = 30
```

### `[editor.file-explorer]` Section

In addition to the options for the file picker and global search, a similar set of options is presented to configure the file explorer separately. However, unlike the file picker, the defaults are set to avoid ignoring most files.

Note that the ignore files consulted by the file explorer when `ignore` is set to true are the same ones used by the file picker, including the aforementioned Helix-specific ignore files.


| Key | Description | Default |
|--|--|---------|
|`hidden` | Enables ignoring hidden files | `false`
|`follow-symlinks` | Follow symlinks instead of ignoring them | `false`
|`parents` | Enables reading ignore files from parent directories | `false`
|`ignore` | Enables reading `.ignore` files | `false`
|`git-ignore` | Enables reading `.gitignore` files | `false`
|`git-global` | Enables reading global `.gitignore`, whose path is specified in git's config: `core.excludesfile` option | `false`
|`git-exclude` | Enables reading `.git/info/exclude` files | `false`
|`flatten-dirs` | Enables flattening single child directories | `true`

### `[editor.explorer]` Section

Configuration for the explorer file tree panel (toggled with `<space>x` or
`:explorer-toggle`). The panel is a real tree sidebar: pinned to one edge of
the terminal it resizes the editor views around itself, or it can float over
the editor. Opening the panel does not scan the whole workspace — directory
children are read from disk on first expansion only.

| Key | Description | Default |
|--|--|---------|
|`side` | Where the panel lives: `left`, `right`, or `floating` | `left` |
|`width` | Width of the pinned panel in columns | `30` |
|`floating-width` | Width of the floating window in columns. Unset: as wide as the file picker without its preview (75% of the screen), so it follows the screen size | unset |
|`floating-height` | Height of the floating window in lines. Unset: as high as the file picker (90% of the screen) | unset |
|`floating-x`, `floating-y` | No longer used: the floating window is always centered. Still accepted, so older configs keep loading | unset |
|`icons` | Show per-file-type icons (honors the global `[icons.mime]` settings) | `true` |
|`auto-reveal` | Select the current buffer's file when the panel opens | `false` |
|`show-hidden` | Show dotfiles (hidden files) | `false` |
|`directories-first` | Sort directories before files in directory listings | `true` |
|`git-status` | Mark changed files, and the folders that contain them, with their git status (see [Git status](#git-status-in-the-tree)) | `true` |
|`indent-guides` | Draw the lines (`├─ └─ │`) that show the nesting of folders. Off: the tree is indented by two columns per level, without lines | `true` |

Example:

```toml
[editor.explorer]
side = "left"
width = 35
```

The side and width can also be switched at runtime with
`:explorer-left`, `:explorer-right`, `:explorer-float`, and
`:explorer-width <columns>`.

`<space>x` is a two-state toggle: it opens and focuses the panel when it is
hidden, and hides it when it is open, whether the tree or the editor has the keyboard. To move
focus to an open panel without hiding it, use `C-h`, `<space>wx` (the Window submode) or
`:explorer-focus`.
Opening a file with `l`/Enter lands focus on that buffer directly; `Esc`
hands focus back to the editor while keeping the tree open, and `q` hides
the panel entirely. When the panel floats, anything that hands focus back
into an opened buffer (`l`/Enter, a split with `v`/`s`) — or plain `Esc`
— also closes the float, since it would otherwise cover the editor.

The tree uses the global [`[icons]`](./configuration.md#icons) configuration, shared with
the buffer picker and the bufferline; icons are shown when `[icons.mime] enabled = true`. The folder glyph is the `directory` override in `[icons.mime]`,
and file icons are keyed by exact file name (`README.md`) or file
extension (`rs`) — not by language name:

```toml
[icons.mime]
enabled = true
directory = "📁"
directory-open = "📂"
directory-empty = "🗀"  # folder with nothing in it (default: outline folder glyph)
rs = "🦀"
```

Inside the panel (while it is focused):

Moving up and down uses your normal-mode keymap: the movement commands its keys resolve to
(`move_visual_line_down`, `goto_file_start`, `goto_last_line`, `half_page_down`,
`goto_window_top`, ..., with any count) move the tree's cursor instead of the buffer's. So
remapping them in `[keys.normal]` changes them in the tree too. The keys below that are the
tree's own (open, rename, delete, ...) take precedence over the keymap.

| Key | Description |
|--|--|
|`j`/`k`, up/down, with a count (`5j`) | Move within the tree |
|`gg`/`ge` | Jump to the top/bottom of the tree (`10gg`: the tenth row) |
|`C-d`/`C-u`, `C-f`/`C-b`, PageDown/PageUp | Move by half a page / a page |
|`gt`/`gc`/`gb` | Jump to the top/middle/bottom of the visible rows |
|`G`, `5G` | Jump to the last row; with a count, to that row (`5G`: the fifth row) |
|Home/End | Jump to the first/last row |
|`h`, left | Collapse the directory under the cursor, or ascend to its parent |
|`-` | Move the cursor to the parent directory row |
|`l`/Enter, right | Expand the directory under the cursor, or open the file |
|`v` | Open the file under the cursor in a vertical split |
|`s` | Open the file under the cursor in a horizontal split |
|`.` | Reveal and select the current buffer's file |
|`R` | Reload expanded directories from disk |
|`H` | Toggle showing hidden files |
|`l`/Enter on `...` | Go up: the parent directory becomes the explorer's root (the `...` row is the first row, hidden while filtering) |
|`/` | Focus the filter box at the top of the panel (see below) |
|`a` | Create a file (or a directory when the name ends in `/`) in the selected directory, or beside the selected file; at the root when nothing is selected |
|`A` | Create a directory in the same place |
|`r` | Rename the entry under the cursor (the popup is titled `Rename file <name>` / `Rename folder <name>` and prefilled). Open buffers of the file follow it. Changing only the case of the name (`readme.md` to `README.md`) also works on case-insensitive file systems (macOS, Windows) |
|`d` | Delete the entry under the cursor; a confirmation box opens first and defaults to Cancel |
|`x` | Cut the entry under the cursor |
|`c` | Copy the entry under the cursor |
|`p` | Paste from the explorer clipboard into the directory under the cursor. FIFOs, sockets and devices inside a copied folder are skipped |
|`q` | Close the panel |
|`Esc` | Clear an active filter first; otherwise hand keyboard focus back to the editor (panel keeps its tree; a floating panel closes instead) |

### Git status in the tree

Changed files are marked at the right edge of their row, and a folder that contains
changes shows a `•` in the color of its strongest change (conflict, deleted, modified,
renamed, added, untracked). The marks use the glyphs of the `[icons.vcs]` set (letters
`M`, `A`, `D`, `R`, `U`, `?` when icons are off) and the `diff.plus`, `diff.delta`,
`diff.minus`, `diff.delta.moved` and `diff.delta.conflict` theme keys, the same as the
`changed_file_picker`. A file that is only staged shows its staged change.

The status is read when the panel opens, after file operations and `R`, and again while
the panel is drawn (a hidden panel costs nothing) once it is three seconds old. In a large
repository where `git status` is slow it is read less often: the wait is ten times as long as
the last `git status` took, at most 30 seconds (also the wait after `git status` failed, for
example outside a repository). In a trusted workspace it comes from `git status`; otherwise
from the built-in listing (no staged state). Turn it off with `git-status = false`.

### The filter box and the lines

The panel opens with a box titled `Explorer`: the filter prompt (`>`) with a hint while
the filter is empty (`/ to search (*.rs)`: `/` starts a filter, wildcards work too), and
on the right a count like a picker's. Without a filter it counts the entries listed (`51/51`); while
filtering, the entries matching out of those searched (`12/4318`, with `(scanning)` in
front while the search is still running). The tree starts under it with the folder it is
rooted in. A panel too short for the box shows the folder (shortened from the start when
it does not fit) on its first row instead, or the filter while there is one, with a blank
row under it before the tree.
The nesting of folders is drawn with the tree lines `├─`, `└─` and `│`, three columns per
level; `indent-guides = false` turns the lines off and indents by two columns instead.
Names that do not fit end in `…`.

### Filtering

Press `/` (or click the filter box) and type:
the input is a fuzzy search, matched and ranked like in the pickers (`mnrs` finds
`main.rs`). The query uses smart case (an uppercase letter makes it case
sensitive) and the picker operators: space separated terms must all match, `^prefix`,
`suffix$`, `'exact` and `!negated`. The matched letters are highlighted, and the
best match is selected as you type. A query containing `/` is matched against the
path relative to the root (`src/main`). Patterns with `*`, `?`, `[abc]`, `[a-z]`,
`[!x]` or `{a,b}` are matched as wildcards against the whole name instead
(`*.rs`, `test_?.py`, `*.{toml,md}`). While a filter is active the
tree shows only matching entries and the directories leading to them, searching
the whole tree (capped at 100,000 entries, `.git` skipped). Up/Down move through the results while you keep typing (then Right expands and Left collapses the selected folder, until you type again); Enter opens
the selected row (the best matching file if you haven't moved); Tab leaves the box and
keeps the filter to browse the results with the tree keys; Esc (or clearing the box) restores the
full tree and reveals the selected entry. `C-h` and `C-l` stop typing (the filter stays applied)
and move focus as usual.

### Moving focus between the tree and the buffers

Focus moves in both directions with keys that are bound by default and can be changed in your
keymap (see [Fork-specific keys](./keymap.md#fork-specific-keys)):

- `C-h` (`explorer_focus`) moves keyboard focus from the buffer to the tree. If the panel is
  hidden, it opens.
- `C-l` (`explorer_blur`) moves keyboard focus from the tree back to the buffer area. The panel
  stays open.
- Clicking in a buffer (or its gutter) moves focus to that buffer as well.

`<space>x` only toggles the panel: it shows it and takes focus when hidden, and hides it when open.

### Theming the explorer

Four theme scopes style the explorer:

- `ui.explorer.border` styles the separator of a pinned panel and the frame of the floating window.
  Without it, the separator uses `ui.virtual` and the frame uses `ui.popup.border`.
- `ui.explorer.tree` styles the tree guide lines (`├─ └─ │`) on normal rows. Without it, `ui.virtual`.
- `ui.explorer.tree.selected` styles the same guide lines on the selected row. Without it, the normal
  guide style over the selection highlight.
- `ui.explorer.path` styles the path from the root of the tree to the selected row: the root row,
  every ancestor row, and the selected row, with their own guide lines. Sibling rows are not
  highlighted. The selected row itself always uses `ui.selection`, so it stays the most visible row.
  Without `ui.explorer.path`, `ui.cursorline.primary` is used, then `ui.selection`.

```toml
"ui.explorer.border" = { fg = "#7aa2f7" }
"ui.explorer.tree" = { fg = "#565f89" }
"ui.explorer.tree.selected" = { fg = "#7aa2f7" }
"ui.explorer.path" = { bg = "#2a2f45" }
```

The floating frame has one exception: when `[editor.gradient-borders]` is enabled, the gradient is
drawn instead and `ui.explorer.border` is ignored. The priority for the floating frame is:

1. `[editor.gradient-borders]`, when enabled.
2. `ui.explorer.border`, when the theme sets it.
3. `ui.popup.border` (the previous style).

### Floating explorer

The floating window has its own toggle, independent of the pinned panel:
`<space>X` / `:explorer-float-toggle`. `<space>x` keeps toggling the pinned
panel; if the float is showing it switches to the pinned panel instead of
re-opening the float.

### Mouse

The floating window's border is a plain solid line by default. When
`[editor.gradient-borders]` is enabled it is drawn as a gradient border instead
(using its colors, direction and thickness), exactly like the pickers.

The floating window opens centered on the screen and is sized like the file
picker without its preview (`hide-preview`), as the tree is a list too: 75% of
the editor's width and 90% of its height (without the status line and the
command line). So it grows and shrinks with the terminal. Setting
`floating-width` / `floating-height` gives it that size instead (still centered),
but never more than those 75% and 90%. The size and position it gets from the mouse are
not saved, not even in the config; `floating-x` / `floating-y` are ignored.

While it is open, it can be moved and resized with the mouse: drag the top
border to move it, any other border (or a corner) to resize it. The next time
it opens it is centered again; `:explorer-float-reset` centers the open one.

The separator between a pinned panel and the editor can be dragged to resize
the panel. Dividers between editor splits can be dragged too. Notifications
are drawn in the area not covered by the pinned panel.

With mouse support enabled (`[editor]` `mouse = true`, the default), left click
selects and acts on the row under the pointer — a folder expands/collapses, a
file opens in the buffer — and the scroll wheel moves the selection. Clicks
outside the panel are unaffected.

### Window resizing and focus mode

Panels and splits can be resized from the keyboard. Holding `Alt` with the `w` key starts a
one-off command; `Alt+W` (Alt+Shift+w) enters a sticky mode in which single keys repeat:

| Key                          | Action                               |
| ---                          | ---                                  |
| `Alt+w h` / `Alt+w left`     | Shrink the window width              |
| `Alt+w l` / `Alt+w right`    | Grow the window width                |
| `Alt+w j` / `Alt+w down`     | Shrink the window height             |
| `Alt+w k` / `Alt+w up`       | Grow the window height               |
| `Alt+w f`                    | Toggle focus mode (expands the current window) |

In sticky mode (`Alt+W`), use `h`/`l`/`j`/`k`, the arrow keys, or `f`. Sticky mode stays on until
you press a key that is not a window resize command.

How far a window can grow is limited by the `[editor]` keys below. `0` for an absolute limit means
the limit is calculated from the terminal size, using the percentage keys instead:

```toml
[editor]
max-panel-width = 50            # absolute maximum, in terminal columns (0 = use the percentage)
max-panel-height = 50           # absolute maximum, in terminal rows (0 = use the percentage)
max-panel-width-percent = 0.8   # used when max-panel-width is 0 (0.0-1.0)
max-panel-height-percent = 0.8  # used when max-panel-height is 0 (0.0-1.0)
```

### Welcome screen

When Helix starts without a file to open, the editor shows a welcome screen in the empty buffer.
It is drawn in every split that shows this buffer (after `:vsplit`, say), not only the first
one, and disappears as soon as you type in it or open a file in its place.

It is as large as the view allows: the Helix mark (drawn with half blocks, two pixels per cell)
beside the "HELIX" wordmark and the help; on a shorter view, a smaller mark beside the help; then
the help alone. Without true color, the logo is drawn in the text's color. Turn it off with the
`[editor]` key `welcome-screen`:

```toml
[editor]
welcome-screen = false
```

### Window title

With `set-title` on, Helix sets the window title of the terminal (the title of its window or
tab) to `title-format`, like Vim's `title` and `titlestring`. It is off by default, which leaves
the title to the terminal or the shell.

```toml
[editor]
set-title = true
title-format = "hx %{buffer_name}:%{cursor_line}:%{cursor_column}"   # the default
```

- `title-format` supports the [expansions](./command-line.md#expansions) of the command line:
  variables such as `%{buffer_name}`, `%{cursor_line}`, `%{cursor_column}`, `%{language}`,
  `%{file_path_absolute}` and `%{current_working_directory}`, registers (`%reg{...}`) and
  Unicode characters (`%u{...}`). Shell commands (`%sh{...}`) are not run: the title is
  expanded for every frame. A format that cannot be expanded sets the title to `hx`, and the
  error is written to the log once.
- The title is only sent to the terminal when it changes. Control characters (in a file name,
  say) are left out of it.
- The terminal's own title is saved on its title stack (`XTPUSHTITLE`) and comes back when
  Helix exits or is suspended (`Ctrl-z`), and when `set-title` is turned off with
  `:config-reload` or `:set set-title false`. A terminal without a title stack keeps the last
  title Helix set.

### Ruler character

Rulers (`rulers = [80, 100]`) are drawn as a background color by default. Set `ruler-char` to a
single character to draw them as a glyph instead, such as `┊`, `│`, `.` or `|`. An empty string (the
default) keeps the background style.

```toml
[editor]
rulers = [80, 100, 120]
ruler-char = "┊"
```

### `[editor.quickfix]` Section

Lists of locations (search results, diagnostics, symbols, references, ...) in a panel at the
bottom of the editor, like Vim's quickfix window, instead of a picker. See
[Quickfix panel](./quicklist.md#quickfix-panel) for the lists and the keys.

| Key | Description | Default |
|--|--|--|
| `enable` | Open the lists of locations in the panel (otherwise they are pickers) | `false` |
| `height` | Rows of the panel, its border included (less on a short screen) | `12` |
| `preview` | Preview the selected location over the code while the panel has the focus | `true` |

### `[editor.cmdline]` Section

The command line can be shown as a centered popup, in the style of noice.nvim, instead of on the
bottom line. See [Command line](./command-line.md#popup-style) for how it is used.

| Key | Description | Default |
|--|--|--|
| `style` | `"bottom"` (the bottom line) or `"popup"` | `"bottom"` |
| `show-icons` | Show an icon for the type of command | `true` |
| `min-popup-width` | Minimum width of the popup, in columns | `40` |
| `max-popup-width` | Maximum width of the popup, in columns | `80` |
| `use-full-height` | With `style = "popup"`, use the full terminal height and drop the reserved bottom line | `false` |
| `icons.search` | Icon for search prompts (`/`, `?`) | `"🔍"` |
| `icons.command` | Icon for command mode (`:`) | `"🛠️"` |
| `icons.shell` | Icon for shell prompts (`\|`, `!`, `A-\|`, `A-!`, `$`) | `"⚡"` |
| `icons.general` | Icon for other prompts | `"💬"` |

```toml
[editor.cmdline]
style = "popup"
use-full-height = true

[editor.cmdline.icons]
search = "/"
command = ":"
shell = "$"
general = ">"
```

### `[editor.notifications]` Section

Messages that used to go to the statusline can be shown as popups instead. The history is
available with `:notifications-history`, `:notifications-clear` clears it and
`:notifications-dismiss` closes the active popups.

The popups are stacked from the corner (or the middle of the edge) of `position`, oldest first;
when they do not all fit, the oldest are left out, so the newest is always shown. They are drawn
over the views but under what you are using: the completion menu, pickers, popups and the
command line.

| Key | Description | Default |
|--|--|--|
| `enable` | Turn the notification system on | `true` |
| `style` | `"popup"` (noice-style popups) or `"statusline"` | `"popup"` |
| `position` | `"top-right"`, `"top-center"`, `"top-left"`, `"bottom-right"`, `"bottom-center"` or `"bottom-left"` | `"top-right"` |
| `max-width` | Maximum width of a popup, in columns | `60` |
| `max-height` | Maximum height of a popup, in lines; a longer message ends in `…` (the whole of it is in `:notifications-history`) | `10` |
| `default-timeout` | Time before a popup disappears, in milliseconds. `0` keeps it until dismissed | `5000` |
| `show-icons` | Show an icon for the severity (only when `show-emojis` is off) | `true` |
| `show-emojis` | Show an emoji for the severity; takes the place of the icon | `true` |
| `padding` | Blank space inside the popup, in cells | `1` |
| `max-history` | Number of notifications kept for `:notifications-history` | `100` |
| `enable-history` | Allow `:notifications-history` | `true` |
| `border.enable` | Draw a border around each popup | `true` |
| `border.width` | Number of nested border frames (ignored with gradient borders, which are one cell wide) | `1` |
| `border.radius` | Above `0`, a `plain` border (from `border-style`, with `border.style = "solid"`) and a dashed or dotted one get rounded corners (`╭╮╰╯`), so by default notifications are rounded. `0` keeps the glyphs of `border-style` | `2` |
| `border.style` | Glyphs of the notification border: `"solid"` uses those of the editor's `border-style`; `"dashed"` (`┄ ┆`) and `"dotted"` (`┈ ┊`) draw broken lines; or any `border-style` value (`"none"`, `"plain"`, `"rounded"`, `"double"`, `"thick"`, `"quadrant-inside"`, `"quadrant-outside"`) for notifications only, whatever `border.radius` says. A gradient border follows `"none"` and rounded corners only | `"solid"` |
| `border.gradient` | Draw the notification border with a gradient from `border.gradient-start` to `border.gradient-end`, even when `[editor.gradient-borders]` is off; its line weight and direction come from [`gradient-borders`](#editorgradient-borders-section). When `false`, notifications follow `gradient-borders` like every popup | `false` |
| `border.gradient-start`, `border.gradient-end` | Colors of the `border.gradient`, as `"#rrggbb"` (an invalid color falls back to the default) | `"#8A2BE2"`, `"#00BFFF"` |
| `shadow.enable` | Draw a shadow behind each popup: the cells right of and below it are darkened (what they show stays) | `true` |
| `shadow.offset-x`, `shadow.offset-y` | Offset of the shadow, in cells | `1`, `1` |
| `icons.info`, `icons.warning`, `icons.error`, `icons.success` | Icon text for each severity | `ℹ`, `⚠`, `✗`, `✓` |
| `emojis.info`, `emojis.warning`, `emojis.error`, `emojis.success` | Emoji text for each severity | `💡`, `⚠️`, `❌`, `✅` |

The colors come from the theme: `ui.popup` for the background and the `error`, `warning`,
`info` and `hint` scopes for the text; the border uses `ui.popup.border`, or the gradient when
`border.gradient` or `[editor.gradient-borders]` is enabled. The border cells follow the same
rule as the other popups (see `border-style`, or `border.style` when it names one): with
`"none"`, `"plain"` and `"quadrant-outside"` (and without a border) they take the
notification's background; with the styles drawn through the middle of their cells (rounded,
double, thick, dashed, dotted, `"quadrant-inside"`) they keep what is behind the
notification. With the style `"none"` no border is drawn, but its space is kept. See [Notification and cmdline popup
styling](./themes.md#notification-and-cmdline-popup-styling).

### `[editor.inline-blame]` Section

Shows the latest commit that changed the line under the cursor, as virtual text at the end of the
line. `<space>B` (`blame_line`) shows the same information for the cursor line in the statusline.

| Key | Description | Default |
|--|--|--|
| `show` | When to show inline blame: `"cursor-line"` (only the primary cursor's line), `"all-lines"` (every line) or `"never"` | `"never"` |
| `format` | Text to show. See the placeholders below | `"{author}, {time-ago} • {title} • {commit}"` |
| `auto-fetch` | Fetch the blame of the file in the background, so it is ready without `<space>B` | `false` |

```toml
[editor.inline-blame]
show = "cursor-line"
format = "{author} • {time-ago}"
auto-fetch = false
```

The `format` placeholders are:

- `{author}`: the author of the commit
- `{date}`: when the commit was made
- `{time-ago}`: how long ago the commit was made
- `{title}`: the title of the commit
- `{body}`: the body of the commit
- `{commit}`: the short hexadecimal SHA-1 hash of the commit
- `{email}`: the email address of the author

With `auto-fetch = false`, the blame of a file is fetched only when you ask for it with `<space>B`, so
there can be a short delay. With `auto-fetch = true` it is fetched in the background, and `<space>B`
then answers at once. When a file is opened, inline blame does not appear even with `show` set to
`"all-lines"` or `"cursor-line"` until its blame has been fetched, which `<space>B` does.

The color of the indicator is the `ui.virtual.inline-blame` theme key.

### `[editor.gradient-borders]` Section

Draws the borders of pickers, command line popups, completion menus, previews and the floating
explorer as a gradient, instead of a solid line. It is off by default.

| Key | Description | Default |
|--|--|--|
| `enable` | Turn gradient borders on | `false` |
| `thickness` | Glyphs of the line, from `1` (thin) to `5` (full blocks), see below. Other values draw a thin line | `1` |
| `direction` | `"horizontal"`, `"vertical"`, `"diagonal"` or `"radial"` | `"horizontal"` |
| `start-color` | Hex color at the start of the gradient | `"#8A2BE2"` |
| `end-color` | Hex color at the end of the gradient | `"#00BFFF"` |
| `middle-color` | Optional third color, for a three-color gradient | unset |
| `animation-speed` | `0` (off) to `10`; the gradient moves at this speed | `0` |

```toml
[editor.gradient-borders]
enable = true
thickness = 2
direction = "diagonal"
start-color = "#FF4500"
middle-color = "#FFD700"
end-color = "#FF69B4"
animation-speed = 1
```

The border is always one cell wide; `thickness` only picks its glyphs:

| `thickness` | Glyphs |
|--|--|
| `1` | Thin. With square corners it is drawn along the outer edge of its cells like a `plain` border (`▏ ▔ ▕ ▁`, corners `🭽 🭾 🭼 🭿`), so the popup's background reaches the line. With rounded corners: `─ │ ╭ ╮ ╰ ╯` |
| `2` | Thick: `━ ┃ ┏ ┓ ┗ ┛` |
| `3` | Double: `═ ║ ╔ ╗ ╚ ╝` |
| `4` | Half blocks on the inner side of the top and bottom (`▄` above, `▀` below), full blocks (`█`) for the sides and corners |
| `5` | Full blocks (`█`) all around |

Rounded corners follow `rounded-corners` and `border-style`: `"rounded"` rounds the corners of a
thickness `1` border; thicker borders and the block styles keep square corners. With
`border-style = "none"` no gradient is drawn (the space of the border is kept). Borders other
than the thin square one keep the background behind the popup in their cells, so nothing shows
between the line and the popup, nor beyond it.

Titles in a gradient border (the picker's title, the command line popup's) are drawn in the
gradient's colors when those have enough contrast (3:1) with the background under the title,
otherwise in `ui.text`; the `ui.popup.title` theme key sets their style explicitly. A title too
long for the border is cut and ends in `…`. When gradient
borders are on, they replace the solid border of every component, including the floating explorer,
which also overrides `ui.explorer.border` (see [Theming the explorer](#theming-the-explorer)).

### `[editor.buffer-picker]` Section

Set options for buffer picker.

| Key | Description | Default |
|--|--|---------|
|`start-position` | Controls behavior for which buffer is initially selected | `current` |

Example

```toml
[editor.buffer-picker]
start-position = "previous"
```

### `[editor.auto-pairs]` Section

Enables automatic insertion of pairs to parentheses, brackets, etc. Can be a
simple boolean value, or a specific mapping of pairs of single characters.

To disable auto-pairs altogether, set `auto-pairs` to `false`:

```toml
[editor]
auto-pairs = false # defaults to `true`
```

The default pairs are <code>(){}[]''""``</code>, but these can be customized by
setting `auto-pairs` to a TOML table:

```toml
[editor.auto-pairs]
'(' = ')'
'{' = '}'
'[' = ']'
'"' = '"'
'`' = '`'
'<' = '>'
```

Additionally, this setting can be used in a language config. Unless
the editor setting is `false`, this will override the editor config in
documents with this language.

Example `languages.toml` that adds `<>` and removes `''`

```toml
[[language]]
name = "rust"

[language.auto-pairs]
'(' = ')'
'{' = '}'
'[' = ']'
'"' = '"'
'`' = '`'
'<' = '>'
```

### `[editor.auto-reload]` Section

Controls auto reloading of externally modified files.

| Key | Description | Default |
|--|--|---------|
| `focus-gained` | Enable automatic reloading of externally modified files when Helix is focused. Requires [focus event support](https://github.com/helix-editor/helix/wiki/Terminal-Support) from your terminal | `false` |
| `periodic.enable` | Enable periodic auto reloading of externally modified files | `false` |
| `periodic.interval` | Time interval in milliseconds between auto reload checks | `3000` |

### `[editor.auto-save]` Section

Control auto save behavior.

| Key | Description | Default |
|--|--|---------|
| `focus-lost` | Enable automatic saving on the focus moving away from Helix. Requires [focus event support](https://github.com/helix-editor/helix/wiki/Terminal-Support) from your terminal | `false` |
| `after-delay.enable` | Enable automatic saving after `auto-save.after-delay.timeout` milliseconds have passed since last edit. | `false` |
| `after-delay.timeout` | Time in milliseconds since last edit before auto save timer triggers. | `3000` |

### `[editor.search]` Section

Search specific options.

| Key | Description | Default |
|--|--|---------|
| `smart-case` | Enable smart case regex searching (case-insensitive unless pattern contains upper case characters) | `true` |
| `wrap-around`| Whether the search should wrap after depleting the matches | `true` |

### `[editor.whitespace]` Section

Options for rendering whitespace with visible characters. Use `:set whitespace.render all` to temporarily enable visible whitespace.

| Key | Description | Default |
|-----|-------------|---------|
| `render` | Whether to render whitespace. May either be `all` or `none`, or a table with sub-keys `space`, `nbsp`, `nnbsp`, `tab`, and `newline` | `"none"` |
| `characters` | Literal characters to use when rendering whitespace. Sub-keys may be any of `tab`, `space`, `nbsp`, `nnbsp`, `newline` or `tabpad` | See example below |

Example

```toml
[editor.whitespace]
render = "all"
# or control each character
[editor.whitespace.render]
space = "all"
tab = "all"
nbsp = "none"
nnbsp = "none"
newline = "none"

[editor.whitespace.characters]
space = "·"
nbsp = "⍽"
nnbsp = "␣"
tab = "→"
newline = "⏎"
tabpad = "·" # Tabs will look like "→···" (depending on tab width)
```

### `[editor.indent-guides]` Section

Options for rendering vertical indent guides.

| Key           | Description                                             | Default |
| ---           | ---                                                     | ---     |
| `render`      | Whether to render indent guides                         | `false` |
| `character`   | Literal character to use for rendering the indent guide | `"│"`   |
| `skip-levels` | Number of indent levels to skip                         | `0`     |

Example:

```toml
[editor.indent-guides]
render = true
character = "╎" # Some characters that work well: "▏", "┆", "┊", "⸽"
skip-levels = 1
```

### `[editor.gutters]` Section

For simplicity, `editor.gutters` accepts an array of gutter types, which will
use default settings for all gutter components.

```toml
[editor]
gutters = ["diff", "diagnostics", "line-numbers", "spacer"]
```

To customize the behavior of gutters, the `[editor.gutters]` section must
be used. This section contains top level settings, as well as settings for
specific gutter components as subsections.

| Key      | Description                    | Default                                                       |
| ---      | ---                            | ---                                                           |
| `layout` | A vector of gutters to display | `["diagnostics", "spacer", "line-numbers", "spacer", "diff"]` |

Example:

```toml
[editor.gutters]
layout = ["diff", "diagnostics", "line-numbers", "spacer"]
```

#### `[editor.gutters.line-numbers]` Section

Options for the line number gutter

| Key         | Description                             | Default |
| ---         | ---                                     | ---     |
| `min-width` | The minimum number of characters to use | `3`     |

Example:

```toml
[editor.gutters.line-numbers]
min-width = 1
```

| Key         | Description                                                                                                  | Default |
| ---         | ---                                                                                                          | ---     |
| `same-color` | Draw line numbers in the color of the current mode (insert, normal, select), ignoring whether the line is selected. Style the colors with `ui.linenr.insert`, `ui.linenr.normal` and `ui.linenr.select` in your theme | `false` |

```toml
[editor.gutters.line-numbers]
same-color = true
```

#### `[editor.gutters.diagnostics]` Section

Currently unused

#### `[editor.gutters.diff]` Section

The `diff` gutter option displays colored bars indicating whether a `git` diff represents that a line was added, removed or changed.
These colors are controlled by the theme attributes `diff.plus`, `diff.minus` and `diff.delta`.

Other diff providers will eventually be supported by a future plugin system.

There are currently no options for this section.

#### `[editor.gutters.spacer]` Section

Currently unused

#### `[editor.gutters.code-action-hint]` Section

The `code-action-hint` gutter option displays an indicator for whether a code action is available at current selection.

There are currently no options for this section.

### `[editor.debugger]` Section

Options for the display of debug sessions, see [debugging](./debugging.md).

| Key             | Description                                                                                                              | Default |
| --------------- | ------------------------------------------------------------------------------------------------------------------------ | ------- |
| `inline-values` | Show the values of the variables a line uses at the end of the line while the debuggee is stopped (hidden in modified files) | `true`  |

### `[editor.image-preview]` Section

PNG, JPEG, GIF (first frame) and WebP files are shown as images in the preview of the pickers
(the file picker, the quicklist, ...) instead of `<Binary file>`. The image is scaled to fit the
preview, keeping its proportions, and is not enlarged. Images of more than a million pixels are
decoded in a thread, the preview says `Loading image…` until then. Files of more than 10 MiB
and images of more than 100 million pixels are not previewed.

| Key        | Description                                                          | Default  |
| ---------- | -------------------------------------------------------------------- | -------- |
| `enable`   | Show images in the preview                                           | `true`   |
| `protocol` | How to draw them: `"auto"`, `"kitty"`, `"iterm2"`, `"sixel"` or `"halfblocks"` | `"auto"` |

| Protocol     | Terminals                                  | Notes                                           |
| ------------ | ------------------------------------------ | ----------------------------------------------- |
| `kitty`      | Kitty, Ghostty, ...                        | the [graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) |
| `iterm2`     | iTerm2, WezTerm, ...                       | inline images                                   |
| `sixel`      | foot, mlterm, ...                          | 240 fixed colours, no dithering: gradients band  |
| `halfblocks` | any terminal with colours                  | `▀` with two colours, two pixels per cell       |

`"auto"` looks at the environment variables the terminal sets (`KITTY_WINDOW_ID`, `TERM`,
`TERM_PROGRAM`, ...) and uses half blocks for terminals it doesn't know. It does not ask the
terminal what it supports, so a terminal with Sixel support that doesn't name itself needs
`protocol = "sixel"`. Inside tmux, Zellij and screen the other protocols don't get through (Helix doesn't wrap the
sequences for a multiplexer's passthrough), so `"auto"` uses half blocks there.

```toml
[editor.image-preview]
protocol = "sixel"
```

### `[editor.soft-wrap]` Section

Options for soft wrapping lines that exceed the view width:

| Key                  | Description                                                  | Default |
| ---                  | ---                                                          | ---     |
| `enable`             | Whether soft wrapping is enabled.                            | `false` |
| `max-wrap`           | Maximum free space left at the end of the line.              | `20`    |
| `max-indent-retain`  | Maximum indentation to carry over when soft wrapping a line. | `40`    |
| `wrap-indicator`     | Text inserted before soft wrapped lines, highlighted with `ui.virtual.wrap`. The line number gutter shows the same text on the continuation rows of a wrapped line | `"↪ "`    |
| `wrap-at-text-width` | Soft wrap at `text-width` instead of using the full viewport size. | `false` |

Example:

```toml
[editor.soft-wrap]
enable = true
max-wrap = 25 # increase value to reduce forced mid-word wrapping
max-indent-retain = 0
wrap-indicator = ""  # set wrap-indicator to "" to hide it
```

### `[editor.smart-tab]` Section

Options for navigating and editing using tab key.

| Key        | Description | Default |
|------------|-------------|---------|
| `enable` | If set to true, then when the cursor is in a position with non-whitespace to its left, instead of inserting a tab, it will run `move_parent_node_end`. If there is only whitespace to the left, then it inserts a tab as normal. With the default bindings, to explicitly insert a tab character, press Shift-tab. | `true` |
| `supersede-menu` | Normally, when a menu is on screen, such as when auto complete is triggered, the tab key is bound to cycling through the items. This means when menus are on screen, one cannot use the tab key to trigger the `smart-tab` command. If this option is set to true, the `smart-tab` command always takes precedence, which means one cannot use the tab key to cycle through menu items. One of the other bindings must be used instead, such as arrow keys or `C-n`/`C-p`. | `false` |


Due to lack of support for S-tab in some terminals, the default keybindings don't fully embrace smart-tab editing experience. If you enjoy smart-tab navigation and a terminal that supports the [Enhanced Keyboard protocol](https://github.com/helix-editor/helix/wiki/Terminal-Support#enhanced-keyboard-protocol), consider setting extra keybindings:

```toml
[keys.normal]
tab = "move_parent_node_end"
S-tab = "move_parent_node_start"

[keys.insert]
S-tab = "move_parent_node_start"

[keys.select]
tab = "extend_parent_node_end"
S-tab = "extend_parent_node_start"
```

### `[editor.inline-diagnostics]` Section

Options for rendering diagnostics inside the text like shown below

```text
fn main() {
  let foo = bar;
            └─ no such value in this scope
}
```

| Key        | Description | Default |
|------------|-------------|---------|
| `show-virtual-lines` | Whether to show the connector lines and diagnostic messages below source lines. When `false`, diagnostics can still appear at line ends according to `end-of-line-diagnostics`, and their normal highlights and gutter markers remain. | `false` |
| `cursor-line` | The minimum severity that a diagnostic must have to be shown inline on the line that contains the primary cursor. Set to `disable` to not show any diagnostics inline. This option does not have any effect when in insert-mode and will only take effect 350ms after moving the cursor to a different line. | `"warning"` |
| `other-lines` | The minimum severity that a diagnostic must have to be shown inline on a line that does not contain the cursor-line. Set to `disable` to not show any diagnostics inline. | `"disable"` |
| `prefix-len` | How many horizontal bars `─` are rendered before the diagnostic text.  | `1` |
| `max-wrap` | Equivalent of the `editor.soft-wrap.max-wrap` option for diagnostics.  | `20` |
| `max-diagnostics` | Maximum number of diagnostics to render inline for a given line  | `10` |

The allowed values for `cursor-line` and `other-lines` are: `error`, `warning`, `info`, `hint`.

The (first) diagnostic with the highest severity that is not shown inline is rendered at the end of the line (as long as its severity is higher than the `end-of-line-diagnostics` config option):

```text
fn main() {
  let baz = 1;
  let foo = bar; a local variable with a similar name exists: baz
            └─ no such value in this scope
}
```

With both `cursor-line = "disable"` and `end-of-line-diagnostics = "disable"`, the diagnostics under the cursor are shown in the top right corner of the view instead, so the text never moves and no diagnostic is lost:

```toml
[editor]
end-of-line-diagnostics = "disable"

[editor.inline-diagnostics]
cursor-line = "disable"
```

The corner is not used in a view where a plugin draws annotations (such as the `inline-diagnostic` plugin in `contrib/plugins`, which shows its own panel), so diagnostics are not shown twice.

### `[editor.word-completion]` Section

Options for controlling completion of words from open buffers.

| Key                  | Description                                                    | Default  |
| ---                  | ---                                                            | ---      |
| `enable`             | Whether word completion is enabled                             | `true`   |
| `trigger-length`     | Number of word characters to type before triggering completion | `7`      |

Example:

```toml
[editor.word-completion]
enable = true
# Set the trigger length lower so that words are completed more often
trigger-length = 4
```

### `[editor.completion-highlight]` Section

Option for different coloring of completions

| Key                  | Description                                                                              | Default  |
| ---                  | ---                                                                                      | ---      |
| `highlight-type`     | What kind completion highlighting we will display ("default", "theme-colors", "vibrant") | `default`|

Example:

```toml
[editor.completion-highlight]
highlight-type = "vibrant"
```

### `[editor.scrolloff]` Section

Options for setting scrolloff shown below


| Key        | Description | Default |
|------------|-------------|---------|
| `vertical` | Number of lines of padding around the top and bottom of the screen when scrolling | `5` |
| `horizontal` | Number of lines of padding around the left and right of the screen when scrolling | `5` |

Settings the scrolloff to `999` for `vertical` will center the line with the cursor.
```toml
[editor.scrolloff]
vertical = 5
horizontal = 5
```

### `[editor.workspace-trust]` Section

Controls implicit workspace trust. See the [workspace
trust](./workspace-trust.md) chapter for the full feature.

| Key       | Description                                                              | Default     |
| ---       | ---                                                                      | ---         |
| `level`   | The default level of trust for every workspace.                         | `"servers"` |
| `prompt`  | Whether to show a modal when opening a file in an untrusted workspace.   | `true`      |
| `trusted` | Glob patterns whose matching workspaces are trusted without a grant.     | `[]`        |

Example:

```toml
[editor.workspace-trust]
# Even if `false`, the statusline `[⚠]` indicator is still shown.
prompt = false

# "none":     prompt for every workspace.
# "servers":  trust LSP and DAP launches but still gate local config and git;
#             .helix/config.toml, .helix/languages.toml, etc. need :workspace-trust.
# "insecure": trust everything (discouraged).
level = "servers"

# Discouraged: skips .helix/ change detection and trusts anything that lands
# under a matching path. `~` and environment variables are expanded.
trusted = ["~/src/github.com/me/*"]
```

### `[editor.persistence]` Section

Options for persisting editor state between sessions.

The state is formatted with bincode, and stored in files in the state directory (`~/.local/state/helix` on Unix, `~\Local Settings\Application Data\helix\state` on Windows). You can reset your persisted state (and recover from any corruption) by deleting these files.

| Key | Description | Default |
| --- | ----------- | ------- |
| `old-files` | whether to persist file locations between sessions ( when you reopen the a file, it will open at the place you last closed it) | `false` |
| `commands` | whether to persist command history between sessions | `false` |
| `search` | whether to persist search history between sessions | `false` |
| `clipboard` | whether to persist helix's internal clipboard between sessions | `false` |
| `old-files-exclusions` | a list of regexes defining file paths to exclude from persistence | `[".*/\.git/.*", ".*/COMMIT_EDITMSG"]` |
| `old-files-trim` | number of old-files entries to keep when helix trims the state files at startup | `100` |
| `commands-trim` | number of command history entries to keep when helix trims the state files at startup | `100` |
| `search-trim` | number of search history entries to keep when helix trims the state files at startup | `100` |
