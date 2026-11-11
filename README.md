This is a just a fork of Helix. All claims rest with Helix maintainers and its copyright holders. I am just maintaining a fork. 
For more information please see Helix's readme file. 

New Features
1. Explorer (in built)
   <img width="1287" height="1182" alt="image" src="https://github.com/user-attachments/assets/20904c9c-0119-44a1-96da-36c1cade1988" />
You can also change the explorer to be present on the right hand side
   <img width="1280" height="1190" alt="image" src="https://github.com/user-attachments/assets/fe5513d3-ac2a-467e-98c7-a36e245ec1c7" />
Or even a floating one - like so
   <img width="1283" height="1179" alt="image" src="https://github.com/user-attachments/assets/f8b8de0e-f5d6-45a1-9598-7450b597496e" />
Mouse events are supported too !
Add these icons for customized folder icons
```
[icons.mime]
enabled = true
directory = " "
directory-open = " "  # expanded folder icon (default: open-folder glyph, or `directory` if that is customised)

```
3. Noice Cmdline
   
   <img width="580" height="424" alt="image" src="https://github.com/user-attachments/assets/a91ef59a-0923-47ab-b35a-2d9cb22a6462" />
   <img width="1395" height="624" alt="image" src="https://github.com/user-attachments/assets/42c01975-b4a0-4d80-ab0e-8171dbf4df72" />


4. Noice Notifications
   
   <img width="713" height="760" alt="image" src="https://github.com/user-attachments/assets/cf78a977-1fd1-421f-acb2-fe359791b3d9" />

3. Cutomizable Picker border changes (now supports the gradients and the border thickness)

   <img width="1916" height="980" alt="image" src="https://github.com/user-attachments/assets/abfeef4a-e633-489f-9236-3e9adbad05bb" />

4. Show function name in the status bar
   Add the `function-name` element to your editor.statusline config, e.g. `right = ["function-name", "diagnostics", "position"]`
   <img width="428" height="220" alt="image" src="https://github.com/user-attachments/assets/dea63278-a649-4561-a29a-b1c0eddacf9f" />

5. Explorer file tree panel — toggle with `<space>x`, pinned or floating, with file operations (see [Explorer](#explorer) below)
6. LSP code lenses — `Run`, `Debug`, `12 references` labels at the end of lines, off by default (see [Code Lenses](#code-lenses) below)
7. Lua plugins — event handlers, commands, buffer edits, annotations, overlay panels, prompts and pickers (see [Plugins](#plugins) below)
8. Live-updating pickers — the file picker, global search, changed files, buffers, diagnostics and quicklist pickers follow changes while open; big previews load in the background
9. EditorConfig — `.editorconfig` files applied when a file opens and again as soon as one is saved (see [EditorConfig](#editorconfig) below)


   

This fork of Helix implements the following pull requests. Future pull requests that are merged will be merged and listed here.
1. https://github.com/helix-editor/helix/pull/13354 (index command)
2. https://github.com/helix-editor/helix/pull/13053 (local search in buffer)
3. https://github.com/helix-editor/helix/pull/12369 (basic support for icons)
4. https://github.com/helix-editor/helix/pull/13306 (customizable color swatches)
5. https://github.com/helix-editor/helix/pull/13430 (showing vertical preview)
6. https://github.com/helix-editor/helix/pull/11497 (support for rounded corners)
7. https://github.com/helix-editor/helix/pull/13197 (welcome screen)
8. https://github.com/helix-editor/helix/pull/12520 (picker titles)
9. https://github.com/helix-editor/helix/pull/12173 (buffer click)
10. https://github.com/helix-editor/helix/pull/7988 (inactive panes background color)
11. https://github.com/helix-editor/helix/pull/8546 (flex resize, focus mode) - updated with some of my code.
12. https://github.com/helix-editor/helix/pull/12208 (goto hover command)
13. https://github.com/helix-editor/helix/pull/13113 (add file path to the file names for similar file names)
14. https://github.com/helix-editor/helix/pull/12574 (remove code column from diagnotics buffer)
15. https://github.com/helix-editor/helix/pull/9875 (add code file picker)
16. https://github.com/helix-editor/helix/pull/14121 (move lines - no more macros to move lines)
17. https://github.com/helix-editor/helix/pull/14072 (auto-scrolling bufferline)
18. https://github.com/helix-editor/helix/pull/13821 (fix block cursor in terminal)
19. https://github.com/helix-editor/helix/pull/13760 (support workspace commands)
20.  https://github.com/helix-editor/helix/pull/13988 (add support to swap splits)
21. https://github.com/helix-editor/helix/pull/13133 (Inline Git Blame - show commit info for current line)
22. https://github.com/helix-editor/helix/pull/14453 (ruler chars)
23. https://github.com/helix-editor/helix/pull/12902 (File Explorer: create/delete/copy/move/rename)
24. https://github.com/helix-editor/helix/pull/13963 (auto reload buffers on focus / changes)
25. https://github.com/helix-editor/helix/pull/14825 (soft-wrap indicators in gutter)
26. https://github.com/helix-editor/helix/pull/11695 (Add ability to match in next and previous char pairs)
27. https://github.com/helix-editor/helix/pull/14593 (code folding)
28. https://github.com/helix-editor/helix/pull/12043 (6 new case conversion commands: PascalCase, camelCase, snake_case, Title Case, kebab-case, Sentence case)
29. https://github.com/helix-editor/helix/pull/15854 (scrollable file preview in picker)
30. https://github.com/helix-editor/helix/pull/15573 (breadcrumb navigation bar - with tree-sitter fallback, depth/name truncation, debounced LSP requests and icon support)
31. https://github.com/helix-editor/helix/pull/16065 (don't block the UI on prompt completions - file, directory, theme and shell completions of `:open`, `:cd`, `:theme`, `:sh`, ... run in the background, one at a time per prompt; also with the popup command line)
32. https://github.com/helix-editor/helix/pull/16460 (clear completion before switching views with the mouse - fixes the "Savepoint must not be used with a different view!" crash; also covers this fork's bufferline clicks and files opened from the explorer)
33. https://github.com/helix-editor/helix/pull/14757 (kitty multiple cursors protocol - with a bar or underline cursor, terminals supporting it (kitty 0.43+) draw the secondary cursors natively; detected automatically, block cursors stay editor-drawn; cursors under the completion menu or popups are left out)
34. https://github.com/helix-editor/helix/pull/15777 (clear stale LSP data on language server stop/refresh - `:lsp-stop`, `:lang` and path changes drop the old server's document links, color swatches, highlights, code lenses, lightbulbs, symbols and ghost text; the servers that serve the buffer afterwards are asked again right away, so e.g. saving under a new name keeps links and swatches)
35. https://github.com/helix-editor/helix/pull/15325 (set the terminal window title - off by default, `set-title = true` and `title-format` (with expansions) in `[editor]`; the terminal's own title is restored on exit, `Ctrl-z` and when turned off; only sent when it changes; `%sh{}` is not run per frame and control characters are stripped; see [Window Title](#window-title))
36. https://github.com/helix-editor/helix/pull/15022 (recent file picker - `Space O` ("oldfiles"; `Space l` is the buffer search here) lists the files opened or saved, most recent first, without the current one nor deleted ones; kept in `recents.txt` in the cache directory, shared by the editors running; configured in `[editor.recent-picker]` (`enable`, `max-show-entries`, `max-save-entries`, `recents-path`); written in the background and merged atomically, never on the editor's thread, and listed from a thread; files opened are noted too, not only saved ones; no new dependency (the PR's unused `path-clean` is left out))

## Explorer

A keyboard-driven file tree sidebar. Toggle it with `<space>x` or
`:explorer`: when the panel is hidden it opens on the side set in the config (`side`,
below) and takes focus, and when it is
open it hides, whether the tree or the editor has focus. `C-h` moves focus to the panel
(opening it if hidden) and `C-l` moves it back to the buffer, whatever side the panel is on
(see below).
While the panel is focused (in normal mode) a bare `x` cuts the file under the
cursor, which is why the toggle itself is a two-key sequence. Moving back to a
buffer is symmetrical: opening a file with `l`/Enter lands focus on that buffer
directly, `Esc` hands focus back to the editor while keeping the tree open, and
`q` hides the panel entirely. When the panel floats, anything that hands focus
back into an opened buffer (`l`/Enter, a split with `v`/`s`) — or plain `Esc` —
also closes the float, since it would otherwise cover the editor.

The panel is pinned to one edge of the terminal (the editor views resize around
it), or it can float over the editor. Opening it does not scan the whole
workspace — directory children are read from disk on first expansion only.

Default settings (`[editor.explorer]` in your `config.toml`):

| Key | Description | Default |
|--|--|---------|
|`side` | Where the panel lives: `left`, `right`, or `floating` | `left` |
|`width` | Width of the pinned panel in columns | `30` |
|`floating-width` | Width of the floating window in columns. Unset: as wide as the file picker without its preview (75% of the screen), so it follows the screen size | unset |
|`floating-height` | Height of the floating window in lines. Unset: as high as the file picker (90% of the screen) | unset |
|`floating-x`, `floating-y` | No longer used: the floating window is always centered. Still accepted, so older configs keep loading | unset |
|`icons` | Show per-file-type icons (honors the global `[icons.mime]` settings, see below) | `true` |
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

The side comes from `side` and is read each time the panel opens: change it in the
config and run `:config-reload`, or set it for the session with
`:set explorer.side right` (`left`, `right` or `floating`). `:explorer-width <columns>`
sets the width for the session. `:explorer-reveal` selects the current buffer's file,
and `:explorer-refresh` re-reads the expanded directories from disk. `:explorer-focus`
moves focus back to an open panel.

### Overriding the icons

The tree uses the global `[icons]` configuration, shared with the buffer picker and the
bufferline (nothing explorer-specific to set up; see Icons in the book's Configuration page). The folder glyph comes from the `directory`
override in `[icons.mime]`; file icons are keyed by **exact file name**
(`README.md`, `Cargo.toml`) or **file extension** (`rs`, `toml`) — language
names like `rust` are not matched, use the extension instead:

```toml
[icons.mime]
enabled = true
directory = "📁"  # collapsed folder icon in the tree (default: built-in nerd-font folder)
directory-open = "📂"  # expanded folder icon (default: open-folder glyph, or `directory` if that is customised)
directory-empty = "🗀"  # folder with nothing in it (default: outline folder glyph, or `directory` if that is customised)
rs = "🦀"
toml = "⚙"
```

Setting `[editor.explorer] icons = false` disables icons in the tree only;
everything else keeps the global `[icons.*]` settings.

### The panel background

The panel background is styled by the `ui.popup.explorer` theme scope; when a
theme does not define it, it falls back to the popup style (`ui.popup`). To
give the floating window (or the pinned sidebar) its own background, create a
theme that inherits from yours and override it:

```toml
# ~/.config/helix/themes/mine.toml
inherits = "mytheme"
"ui.popup.explorer" = { bg = "#202030" }
```

Then select it with `theme = "mine"` in `config.toml` or `:theme mine`.
The same tweak also tints the panel's border, since the border inherits the
backdrop color.

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

`C-h` (`explorer_focus`) moves the keyboard to the tree and `C-l` (`explorer_blur`) back to
the buffer, from any view and from the quickfix panel, whatever side the panel is on (the
floating window too). `C-h` opens a hidden panel; neither closes anything. Clicking in a
buffer also moves focus there. `<space>x` only toggles the panel.

The window keys move by direction, like between splits: `Ctrl-w h` / `Ctrl-w l` past the last
view on the explorer's side give the keyboard to the explorer, and the opposite direction
gives it back to the code; `Ctrl-w j` from the bottom view reaches the quickfix panel. A
floating explorer is in no direction.

All of these are ordinary keymap entries, so they can be changed in `[keys.normal]`, and the
explorer and the quickfix panel hand them to the editor's keymap. For example:

```toml
[keys.normal]
"C-e" = "explorer_focus"   # to the explorer
"C-b" = "explorer_blur"    # back to the buffer
"C-h" = "jump_view_left"   # and C-h / C-l for splits instead
"C-l" = "jump_view_right"
```

### Theming the explorer

- `ui.explorer.border`: the separator of a pinned panel and the frame of the floating window.
  Without it, the separator uses `ui.virtual` and the frame `ui.popup.border`. For the frame,
  `[editor.gradient-borders]` has the highest priority: when enabled, the gradient is drawn and
  `ui.explorer.border` is not used.
- `ui.explorer.tree`: the tree guide lines (`├─ └─ │`) on normal rows. Falls back to `ui.virtual`.
- `ui.explorer.tree.selected`: the same guide lines on the selected row. Falls back to the normal
  guide style over the selection highlight.
- `ui.explorer.path`: the path from the root of the tree to the selected row: the root row, every
  ancestor row, and the selected row, with their own guide lines. Sibling rows are not highlighted.
  The selected row itself always uses `ui.selection`, so it stays the most visible row. Falls back
  to `ui.cursorline.primary`, then to `ui.selection`.

```toml
"ui.explorer.border" = { fg = "#7aa2f7" }
"ui.explorer.tree" = { fg = "#565f89" }
"ui.explorer.tree.selected" = { fg = "#7aa2f7" }
"ui.explorer.path" = { bg = "#2a2f45" }
```

### Floating explorer

`<space>X` / `:explorer-float` shows the tree as a floating window, whatever side the
config sets, and hides it when it is one. With `side = "floating"`, `<space>x` opens the
floating window too. Otherwise, `<space>x` while a floating window of `<space>X` is
showing switches to the configured side instead of hiding it.

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
it opens it is centered again (close and reopen it with `<space>X` to center it).

The separator between a pinned panel and the editor can be dragged to resize
the panel. Dividers between editor splits can be dragged too. Notifications
are drawn in the area not covered by the pinned panel.

With mouse support enabled (`[editor] mouse = true`, the default), left click
selects and acts on the row under the pointer — a folder expands/collapses, a
file opens in the buffer — and the scroll wheel moves the selection. Clicks
outside the panel keep working as usual.

## Plugins

Plugins are written in Lua 5.4 and live in `~/.config/helix/plugins/<name>/init.lua`. They
are set up in the `[plugins]` section of the global `config.toml` (a workspace's
`.helix/config.toml` cannot set it), and changes to that section need a restart:

```toml
[plugins]
enabled = true

[[plugins.plugins]]
name = "greeter"
config = { greeting = "hi" }        # read by the plugin with helix.get_config()
permissions = ["edit", "commands"]  # what it may do beyond looking ("read") and showing ("ui")
```

- **Permissions** - a plugin may only read the editor and show things unless its entry grants
  more: `edit`, `windows`, `files`, `commands`, `config`, `keymaps`, `explorer`, `shell` (or
  `"all"`). A plugin's `plugin.toml` can list what it needs (it then gets no more); what it asks
  for and is not granted is reported when it loads, and a refused call says which permission
  to grant. `helix.editor.execute_command` checks the command too: `:sh` needs `shell`, `:w`
  needs `files`, `:set shell ...` needs `shell`.
- **Control** - plugins edit any open buffer (shown or not) and create scratch ones, read and
  set selections with their primary, split, scroll, focus and close windows and show any
  buffer in them, bind keys to commands or Lua functions (`helix.keymap.set("normal",
  "<space>z", fn)`), run any command of key bindings (`helix.editor.run("goto_definition")`),
  and read and change settings (`get_option`/`set_option`) and the working directory.
- **Pickers, quickfix, explorer** - pickers with columns, previews and the plugin's own items
  (`on_select(item, action)`), items added while open, docked in the quickfix panel; the
  quicklist set, read and shown from Lua; the explorer opened, revealed at a path, read (root,
  selection) and decorated per row, with events for its cursor and for the files it creates,
  renames, copies and deletes.
- **Drawing** - plugins set data and Helix draws it every frame, without running plugin code:
  status line segments (`helix.statusline.set("branch", { text = "main", position = "left" })`,
  for every buffer or one), gutter signs, highlights of ranges with theme keys, explorer row
  texts, and theme styles (`helix.ui.set_style("ui.statusline", { bg = "#202020" })`, kept
  over theme changes).
- **Background work** - timers (`helix.defer`, `helix.every`), programs whose output comes
  back to Lua (`helix.process.run`, needs `shell`), and requests to the language servers
  (`helix.lsp.request`, with the cursor's position in the server's encoding); the callbacks
  run on the editor's thread when the result is in.
- **Popups** - text, styled lines or markdown, at the cursor, centered or anywhere, updated
  and closed through a handle.
- Signs, highlights, annotations and panels move with the text as it is edited.
- **Isolation** - each plugin runs in a Lua state of its own: one cannot change what another
  calls, nor act with another's permissions, nor take over its `:` commands.
- **Events** - 40 of them: buffers (open, change, rename, save, failed save, close, focus
  lost, language), modes, keys, typed characters, every command run, selections, scrolling,
  windows (open, close, focus), the terminal (resize, focus, paste, mouse), idle, settings,
  theme, working directory, language servers (start, attach, detach, exit, diagnostics), the
  explorer (cursor, files created, renamed, copied, deleted), exit.
  Helix does not even queue an event that no plugin listens to.

Plugin code runs in a sandbox (no `io`, no `os.execute`, `require` only from the plugin
directories) with a 2 second limit per call and 512 MiB of memory. `:reload-all-plugins`
reloads them. See the book's Plugins chapter (`book/src/plugins.md`) for the events and the
API, and [helix-plugin/README.md](helix-plugin/README.md).

## Quickfix Panel

Lists of locations can open in a panel at the bottom of the editor, like Neovim's quickfix
window (`:copen`), instead of in a picker. It is off by default:

```toml
[editor.quickfix]
enable = true   # global search, diagnostics, symbols, references, changed files, jumplist,
                # task tags and the quicklist open in the panel; other pickers stay pickers
height = 12     # rows, border included
preview = true  # preview the selected location over the code while the panel is focused
```

The panel sits under the views (nothing is covered) and stays open while you visit its
entries: `j`/`k` move, `Enter` jumps and gives the focus to the code, `Esc` (or `Ctrl-w k`)
gives the focus to the code without jumping, `Ctrl-w j` (from the bottom view) focuses the
panel again, `]q`/`[q` visit the next and previous entry from the code, `/` filters, and `q`
closes it. `<space>Q` / `:quickfix` shows it with the focus (the last list it showed, or the
quicklist) and hides it, like `<space>x` for the explorer; each key only toggles its own
panel. `:copen` shows it again, `:cclose` closes it. From the panel, `C-h` goes to the
explorer and `C-l` to the code; `Ctrl-w h` (or `l`) reaches the explorer pinned on that side. The floating explorer and the pickers that choose one thing (files, buffers,
commands, ...) are unchanged. See the book's Quicklist page (`book/src/quicklist.md`).

## EditorConfig

`.editorconfig` files ([editorconfig.org](https://editorconfig.org)) set the indentation, tab
width, line ending, charset, trimming, final newline and text width of the files under them;
`editor-config = false` in `[editor]` turns them off. On top of upstream's support:

- **Live** - saving a `.editorconfig` in Helix applies it at once to the open files under its
  directory; `:config-reload` and `:set editor-config` read them all again
- **`indent_size` alone** - sets the width of a file indented with spaces
- **`charset = utf-8-bom`** - a new file starts with the byte order mark
- **Number ranges** - section globs like `[file{1..10}.txt]`
- **A broken section is skipped alone** - instead of the whole file being ignored (logged)

See the book's editor configuration (`book/src/editor.md#editorconfig`).

## Window Title

Helix can set the window (or tab) title of the terminal, like Vim's `title`/`titlestring`. It is
off by default:

```toml
[editor]
set-title = true
title-format = "hx %{buffer_name}:%{cursor_line}:%{cursor_column}"   # the default
```

`title-format` takes the command line's expansions (`%{buffer_name}`, `%{cursor_line}`,
`%{language}`, `%{current_working_directory}`, ...) but not shell commands. The terminal's own
title comes back when Helix exits or is suspended, and with `:set set-title false`. See the
book's editor configuration (`book/src/editor.md#window-title`).

## Zenbones Themes

The `runtime/themes` directory includes Helix ports of the Zenbones collection: `zenbones`, `zenwritten`, `neobones`, `vimbones`, `rosebones`, `forestbones`, `nordbones`, `tokyobones`, `seoulbones`, `duckbones`, `zenburned`, and `kanagawabones`. Light variants are available with the `_light` suffix for `zenbones`, `zenwritten`, `neobones`, `rosebones`, `forestbones`, `tokyobones`, and `seoulbones`. The upstream `randombones` scheme is a random selector rather than a palette, so it is not included as a static Helix theme.

## Setup

[Docs](https://docs.helix-editor.com/building-from-source.html) on build it from source.
Summary for commands:

```bash
git clone https://github.com/gj1118/helix.git
cd helix
cargo install --path helix-term --locked

```

Or if you have `just` installed, you can simply run `just` recipes in the folder where you checked the source code. For more information please see the `justfile` 

THEN THE IMPORTANT PART that is missing from the docs page(this way you get all the default themes and gramars), [source](https://github.com/helix-editor/helix/discussions/7549#discussioncomment-6948696)
```bash
cp -R runtime ~/.config/helix/
hx -g fetch
hx -g build
```

**Full Height Mode:**

When `use-full-height = true` is set along with `style = "popup"`, the command line popup uses the full terminal height by removing the traditionally reserved bottom line.

```toml
# Maximum screen space (recommended for popup style)
[editor.cmdline]
style = "popup"
use-full-height = true

# Traditional with reserved space (default)
[editor.cmdline]
style = "popup"
use-full-height = false
```

## Window Resizing and Focus Mode Commands

**Window Resizing:**
- `Alt+w h` or `Alt+w left` - Shrink window width
- `Alt+w l` or `Alt+w right` - Grow window width  
- `Alt+w j` or `Alt+w down` - Shrink window height
- `Alt+w k` or `Alt+w up` - Grow window height

**Focus Mode:**
- `Alt+w f` - Toggle focus mode (expands current window)

**Sticky Mode:**
Activate sticky mode with `Alt+W` (Alt + Shift + w), then use single keys for repeated resizing:
- `h` or `left` - Shrink width
- `l` or `right` - Grow width  
- `j` or `down` - Shrink height
- `k` or `up` - Grow height
- `f` - Toggle focus mode

Sticky mode stays active until you press a key that's not part of the window resize commands.

**Window Resizing Configuration:**

Configure panel resizing limits in your `config.toml`:

```toml
[editor]
# Absolute maximum limits (in terminal character units)
max-panel-width = 50      # Set to 0 for dynamic limit based on terminal size
max-panel-height = 50     # Set to 0 for dynamic limit based on terminal size

# Percentage-based limits (used when absolute limits are set to 0)
max-panel-width-percent = 0.8   # 80% of terminal width (0.0-1.0)
max-panel-height-percent = 0.8  # 80% of terminal height (0.0-1.0)
```

**Configuration Examples:**

```toml
# Conservative: limit panels to 60% of terminal size
[editor]
max-panel-width = 0
max-panel-height = 0
max-panel-width-percent = 0.6
max-panel-height-percent = 0.6

# Aggressive: allow panels up to 95% of terminal size
[editor]
max-panel-width-percent = 0.95
max-panel-height-percent = 0.95

# Hybrid: absolute width limit, percentage height limit
[editor]
max-panel-width = 100
max-panel-height = 0
max-panel-height-percent = 0.8
```

### Inline Diagnostics

The connector lines and diagnostic messages displayed below source lines can be toggled with
`editor.inline-diagnostics.show-virtual-lines`. It defaults to `false` to keep the display close
to upstream Helix. Set it to `true` to enable the inline diagnostic UI:

```toml
[editor.inline-diagnostics]
show-virtual-lines = true
```

When disabled, diagnostic highlights and gutter markers remain; end-of-line messages are controlled
separately by `editor.end-of-line-diagnostics`.

### Line Numbers

Show line numbers with the same color based on the current mode (insert/normal/select), ignoring the selection state.

```toml
[editor]
# Structured format (recommended):
gutters = { 
    layout = ["line-numbers", "diagnostics"], 
    "line-numbers" = { same-color = true } 
}

# Or array format (existing configs):
# gutters = ["line-numbers", "diagnostics"]
```

When using the structured format, you can also customize mode-specific line number colors in your theme:

```toml
"ui.linenr.insert" = { fg = "color_for_insert" }
"ui.linenr.select" = { fg = "color_for_select" }
"ui.linenr.normal" = { fg = "color_for_normal" }
```

**Benefits:**
- Prevents performance issues with very large panels
- Automatically adapts to your terminal size
- Smooth resizing throughout the entire range
- Configurable limits for different workflows

### Ruler Character

Choose the character used to render rulers in the foreground. The default is an empty string,
which draws background-style rulers; set a character such as `┊` to draw rulers as glyphs.

```toml
[editor]
rulers = [80, 100, 120]
ruler-char = "┊"   # examples: "┊", "│", ".", "|"; "" (the default) draws background-style rulers
```

## Hover Documentation Commands

**Hover Documentation:**
- `Space + k` - Show hover documentation in popup
- `Space + K` - Open hover documentation in navigable buffer (goto_hover)

The `goto_hover` command opens documentation in a new scratch buffer where you can navigate, search, and copy text from long documentation.

## Customizable Color Swatches

**Color Swatches Configuration:**

Configure color swatches appearance in your `config.toml`:

```toml
[editor.lsp]
# Enable/disable color swatches display (default: true)
display-color-swatches = true

# Customize the color swatch symbol (default: "■")
color-swatches-string = "●"
```

**Configuration Examples:**

```toml
# Circle symbols
[editor.lsp]
color-swatches-string = "●"

# Diamond symbols
[editor.lsp]
color-swatches-string = "◆"

# Hexagon symbols
[editor.lsp]
color-swatches-string = "⬢"

# Alternative hexagon
[editor.lsp]
color-swatches-string = "⬣"

# Default square (explicit)
[editor.lsp]
color-swatches-string = "■"
```

Color swatches appear next to color values in your code (CSS, configuration files, etc.) when LSP support is available, making it easier to visualize colors at a glance.

## Code Lenses

Language servers such as rust-analyzer attach actions and facts to lines of code: `Run`,
`Debug`, `Run Test`, `3 implementations`, `12 references`. With code lenses on, they are shown
at the end of the line they belong to, and `<space>A` runs one.

Code lenses cost an extra request to the language server for every document, so they are
**off by default**. Turn them on in your `config.toml`:

```toml
[editor.lsp]
display-code-lenses = true
```

```text
fn main() {                        ▶ Run · Debug
fn parse(input: &str) -> Ast {     ▶ 12 references
```

- `<space>A` (`code_lens`) opens a menu with the lenses of the cursor line; choosing one runs its
  command on the server.
- `<space>o` (`code_lens_picker`) lists the lenses of the whole file in a picker (with a code
  preview, and the same border as the other pickers); choosing one jumps to it and runs it.
- Lenses are fetched on open, shortly after you stop typing, when a server starts and when it
  sends `workspace/codeLens/refresh`. Lenses without a title are resolved once they scroll into
  view.
- Style them with the `ui.virtual.code-lens` theme key (falls back to `ui.virtual.inlay-hint`).
- Per language, `code-lens` is also a language server feature name, so a server can be excluded
  with `except-features = ["code-lens"]` in `languages.toml`.

The idea and the protocol handling follow the upstream proposal
(https://github.com/helix-editor/helix/pull/5063) and the Mitos fork; the code here is written
for this fork's handler, decoration and keymap structure.

## Line Movement Commands

**Move Lines Up/Down:**
- `Ctrl+k` - Move current line or selected lines up
- `Ctrl+j` - Move current line or selected lines down

The line movement feature allows you to easily move the current line or multiple selected lines up and down in your document. This works with:
- Single line: When cursor is on a line, moves that entire line
- Multiple selections: Moves all selected lines while preserving their relative positions
- Discontinuous selections: Handles multiple separate line selections correctly
- Unicode content: Properly handles files with Unicode characters

## Noice.nvim-like Command Line (Cmdline)

**Command Line Popup Configuration:**

This fork includes a modern, noice.nvim-inspired command line with customizable icons and popup-style interface.

```toml
[editor.cmdline]
# Command line style: "bottom" (default) or "popup" (noice.nvim style)
style = "popup"

# Show command type icons (default: true)
show-icons = true

# Popup dimensions
min-popup-width = 40    # Minimum width for popup cmdline
max-popup-width = 80    # Maximum width for popup cmdline

# Use full height when style is popup (removes bottom space, default: false)
# Only applies when style = "popup"
use-full-height = true

# Customize command icons
[editor.cmdline.icons]
search = "🔍"    # For search commands (/,?)
command = "⚙"    # For command mode (:)
shell = "⚡"      # For shell commands (!)
general = "💬"   # For other prompts
```

**Icon Theme Examples:**

```toml
# Minimalist ASCII Style
[editor.cmdline.icons]
search = "/"
command = ":"
shell = "$"
general = ">"

# Nerd Font Icons
[editor.cmdline.icons]
search = ""    # nf-fa-search
command = ""    # nf-fa-cog
shell = ""     # nf-fa-terminal
general = ""   # nf-fa-comment

# Fun Emoji Theme
[editor.cmdline.icons]
search = "🔎"
command = "🛠️"
shell = "🖥️"
general = "📝"

# Disable all icons
[editor.cmdline]
show-icons = false
```

**Features:**
- **Popup-style command line** - Centered floating window instead of bottom line
- **Command type icons** - Visual indicators for different command types
- **Enhanced completion** - Better visual feedback and layout
- **Customizable appearance** - Full control over icons and dimensions (`min-popup-width` and `max-popup-width` are honoured)
- **Like the bottom command line** - the documentation of the command typed (below the popup), the last history entry suggested while nothing is typed, and search regexes highlighted
- **Backward compatibility** - Traditional bottom style still available

## Noice-style Notifications

**Notifications Configuration:**

This fork includes a modern notification system inspired by noice.nvim, displaying messages as popup windows instead of in the statusline.

```toml
[editor.notifications]
enable = true              # Enable notification system
style = "popup"            # "popup" (noice-style) or "statusline" (traditional)
position = "top-right"     # "top-right", "top-center", "top-left", "bottom-right", "bottom-center", "bottom-left"
max-width = 60             # Maximum width for notification popups
max-height = 10            # Maximum height for notification popups
default-timeout = 5000     # Default timeout in milliseconds (0 = no timeout)
show-icons = true          # Show notification type icons
show-emojis = true         # Show notification type emojis
max-history = 100          # Maximum notifications to keep in history
enable-history = true      # Allow :notifications-history
padding = 1                # Blank cells inside the popup
shadow = { enable = true, offset-x = 1, offset-y = 1 }   # darkens the cells beside the popup

# Border configuration
[editor.notifications.border]
enable = true              # Show borders around notifications
width = 1                  # Border width
radius = 0                 # Corner radius (0 for square corners)
style = "solid"            # "solid" (the editor's border-style), "dashed", "dotted",
                           # or a border-style value for notifications only
gradient = false           # Gradient border from gradient-start to gradient-end
gradient-start = "#8A2BE2"
gradient-end = "#00BFFF"

# Customize notification icons
[editor.notifications.icons]
info = "i"
warning = "!"
error = "x"
success = "ok"

# Customize notification emojis
[editor.notifications.emojis]
info = "info"
warning = "warn"
error = "err"
success = "ok"
```

**Styling Notifications:**

Notification appearance is controlled via theme settings. If notifications look poor with your theme, add these to your theme file:

```toml
# In ~/.config/helix/themes/my-theme.toml
inherits = "your-base-theme"

# Popup background and border
"ui.popup" = { bg = "#1a1a1a", fg = "#ffffff" }
"ui.popup.border" = { fg = "#888888" }

# Notification text colors by severity
"error" = { fg = "#ff5555" }
"warning" = { fg = "#ffaa00" }
"info" = { fg = "#55aaff" }
"hint" = { fg = "#888888" }
```

**Commands:**
- `:notifications-history` - Show notification history
- `:notifications-clear` - Clear notification history
- `:notifications-dismiss` - Dismiss all active notifications

**Features:**
- **Popup-style notifications** - Non-intrusive floating windows
- **Severity-based styling** - Different colors for errors, warnings, info, hints
- **Configurable position** - Place notifications where you prefer
- **Auto-dismiss** - Notifications disappear after their timeout
- **Newest first to stay** - when the stack does not fit, the oldest notifications are left out
- **Layering** - over the views, under the completion menu, pickers, popups and the command line
- **History** - Review past notifications with `:notifications-history`

## Aesthetic Gradient Borders

**Gradient Borders Configuration:**

Transform your Helix interface with beautiful, configurable gradient borders for all pickers and UI components.

```toml
[editor.gradient-borders]
enable = true                    # Enable/disable gradient borders
thickness = 2                   # Border thickness (1-5)
direction = "horizontal"        # "horizontal", "vertical", "diagonal", "radial"
start-color = "#8A2BE2"        # Start color (hex format)
end-color = "#00BFFF"          # End color (hex format)
middle-color = "#FF69B4"       # Optional middle color for 3-color gradients
animation-speed = 3            # Animation speed (0-10, 0 = disabled)
```

**Aesthetic Theme Examples:**

```toml
# Cyberpunk Theme
[editor.gradient-borders]
enable = true
thickness = 2
direction = "horizontal"
start-color = "#FF0080"        # Hot Pink
end-color = "#00FFFF"          # Cyan
animation-speed = 2

# Sunset Theme
[editor.gradient-borders]
enable = true
thickness = 3
direction = "diagonal"
start-color = "#FF4500"        # Orange Red
middle-color = "#FFD700"       # Gold
end-color = "#FF69B4"          # Hot Pink
animation-speed = 1

# Ocean Wave
[editor.gradient-borders]
enable = true
thickness = 2
direction = "vertical"
start-color = "#00CED1"        # Dark Turquoise
end-color = "#4169E1"          # Royal Blue
animation-speed = 4

# Matrix Style
[editor.gradient-borders]
enable = true
thickness = 1
direction = "radial"
start-color = "#00FF00"        # Lime Green
end-color = "#008000"          # Dark Green
animation-speed = 5

# Minimalist (No Animation)
[editor.gradient-borders]
enable = true
thickness = 1
direction = "horizontal"
start-color = "#6A5ACD"        # Slate Blue
end-color = "#9370DB"          # Medium Purple
animation-speed = 0
```

**Border Thickness Styles:**
The border is always one cell wide; the thickness picks its glyphs:
- **1**: Thin lines. Square corners hug the outer edge of the cells like a `plain` border (▏▔▕▁ with 🭽🭾🭼🭿); rounded: ─│╭╮╰╯
- **2**: Thick Unicode lines (━┃┏┓┗┛)
- **3**: Double Unicode lines (═║╔╗╚╝)
- **4**: Half blocks on the inner side of the top and bottom (▄ above, ▀ below), full blocks (█) at the sides and corners
- **5**: Full blocks (█) all around

Titles too long for the border are cut and end in `…`. Title colors: see `ui.popup.title` in the themes documentation.

**Rounded Corners Support:**
Gradient borders automatically respect your existing `rounded_corners` setting (and `border-style`: `"rounded"` rounds them, `"none"` hides them):

```toml
[editor]
# Enable rounded corners for all borders (traditional and gradient)
rounded-corners = true

[editor.gradient-borders]
enable = true
thickness = 1    # Thin borders work best with rounded corners
direction = "horizontal"
start-color = "#6A5ACD"
end-color = "#9370DB"
```

- **Thickness 1**: Full rounded corner support (╭╮╰╯)
- **Thickness 2+**: Uses square corners (no Unicode rounded equivalents)
- **Block styles**: Rounded corners don't apply to block characters

**Features:**
- **Applied to all components**: Pickers, command line popups, completion menus, preview panels
- **Dynamic gradients**: Smooth color transitions across any direction
- **Animation support**: Animated gradients with configurable speed
- **Configurable thickness**: From thin lines to chunky block borders
- **Multiple directions**: Horizontal, vertical, diagonal, and radial patterns
- **3-color gradients**: Optional middle color for more complex gradients
- **Performance optimized**: Efficient rendering with minimal overhead

**Note**: Gradient borders are applied to file pickers, command palettes, completion menus, preview panels, and the noice.nvim-style command line popup. Traditional borders are used when gradient borders are disabled.

**Local Development on MacOS**:

To run the app locally on MacOS systems, run the following command on the terminal:
`xattr -d com.apple.quarantine /path/to/your/app`
(this removes the quarantine attribute)

## Inline Git Blame

**Inline Blame Configuration:**

Show git blame information as virtual text next to the current line you're editing. This feature displays the latest commit information for the line your cursor is on.

```toml
[editor]
# Inline blame configuration (inline table form)
inline-blame = { show = "cursor-line", format = "{author} • {time-ago} • {title}", auto-fetch = false }
```

Or in expanded format:

```toml
[editor.inline-blame]
# Show inline blame on specific lines (default: "never")
# Options: "cursor-line", "all-lines", "never"
show = "cursor-line"

# Format string for blame display
# Available placeholders: {author}, {commit}, {time-ago}, {title}
format = "{author} • {time-ago} • {title}"

# Auto-fetch blame information (default: false)
auto-fetch = false
```

**Keybindings:**

- `<space>B` - Show git blame for current line in status line (manual blame)

**Configuration Examples:**

```toml
# Minimal blame display
[editor.inline-blame]
show = "cursor-line"
format = "{author} • {time-ago}"

# Detailed blame information
[editor.inline-blame]
show = "cursor-line"
format = "{commit} - {author} ({time-ago}): {title}"

# Show blame for all lines (can be noisy)
[editor.inline-blame]
show = "all-lines"
format = "{author}"
auto-fetch = true

# Manual blame only (no inline display, use <space>B)
[editor.inline-blame]
show = "never"
```

**Features:**
- **Virtual text display** - Non-intrusive blame info that doesn't affect text editing
- **Cursor-based** - Shows blame only for the line you're currently on
- **Customizable format** - Control what information is displayed
- **Manual fallback** - Use `<space>B` to check blame without enabling inline display
- **Smart caching** - Efficiently caches blame data to avoid repeated git operations

## Signature Help Position

Control where signature help popups appear:

```toml
[editor.lsp]
# Position signature help above cursor (default)
signature-help-position = "above"

# Or below cursor
signature-help-position = "below"

```

## Auto reload buffers on focus / changes
Implements automatic reloading of buffers when their corresponding files are changed on disk. This behavior can be configured to trigger when the editor window gains focus, at a periodic interval, or both.


Here are some example snippets of configuration for this feature:
```toml

# Disable auto-reloading (default)
[editor]
auto-reload = false

#or
[editor.auto-reload]
focus-gained = false

# Auto-reload on focus
[editor]
auto-reload = true
# or
[editor.auto-reload]
focus-gained = true

# Auto-reload at some periodically at time interval (5 seconds in this example)
[editor.auto-reload]
periodic.enable = true
periodic.interval = 5000

# Of course, you could have it reload on focus and at an interval too:
[editor.auto-reload]
focus-gained = true
periodic.enable = true
periodic.interval = 5000
```

## File Picker Configuration

**Hide Preview:**

You can choose to hide the file preview in the file picker to save screen space or improve performance.

```toml
[editor.file-picker]
hide-preview = true
```

## Code Folding

**Code Folding Commands:**
- `fold` - Fold text objects
- `unfold` - Unfold text objects
- `toggle_fold` - Toggle fold for the text object at the primary cursor

**Configuration:**
To automatically fold specific text objects when a document is opened:

```toml
[editor]
fold-textobjects = ["comment"] # Example: fold all comments on open
```
