## Key remapping

Helix currently supports one-way key remapping through a simple TOML configuration
file. (More powerful solutions such as rebinding via commands will be
available in the future).

There are three kinds of commands that can be used in keymaps:

* Static commands: commands like `move_char_right` which are usually bound to
  keys and used for movement and editing. A list of static commands is
  available in the [Keymap](./keymap.md) documentation and in the source code
  in [`helix-term/src/commands.rs`](https://github.com/helix-editor/helix/blob/master/helix-term/src/commands.rs)
  at the invocation of `static_commands!` macro.
* Typable commands: commands that can be executed from command mode (`:`), for
  example `:write!`. See the [Commands](./commands.md) documentation for a
  list of available typeable commands or the `TypableCommandList` declaration in
  the source code at [`helix-term/src/commands/typed.rs`](https://github.com/helix-editor/helix/blob/master/helix-term/src/commands/typed.rs).
* Macros: sequences of keys that are executed in order. These keybindings
  start with `@` and then list any number of keys to be executed. For example
  `@miw` can be used to select the surrounding word. For now, macro keybindings
  are not allowed in keybinding sequences due to limitations in the way that
  command sequences are executed. Modifier keys (e.g. Alt+o) can be used
  like `"<A-o>"`, e.g. `"@miw<A-o>"`

To remap keys, create a `config.toml` file in your `helix` configuration
directory (default `~/.config/helix` on Linux systems) with a structure like
this:

> 💡 To set a modifier + key as a keymap, type `A-X = ...` or  `C-X = ...` for Alt + X or Ctrl + X. Combine with Shift using a dash, e.g. `C-S-esc`. 
> Within macros, wrap them in `<>`, e.g. `<A-X>` and `<C-X>` to distinguish from the `A` or `C` keys.

```toml
# At most one section each of 'keys.normal', 'keys.insert' and 'keys.select'
[keys.normal]
C-s = ":w" # Maps Ctrl-s to the typable command :w which is an alias for :write (save file)
C-o = ":open ~/.config/helix/config.toml" # Maps Ctrl-o to opening of the helix config file
a = "move_char_left" # Maps the 'a' key to the move_char_left command
w = "move_line_up" # Maps the 'w' key move_line_up
"C-S-esc" = "extend_line" # Maps Ctrl-Shift-Escape to extend_line
g = { a = "code_action" } # Maps `ga` to show possible code actions
"ret" = ["open_below", "normal_mode"] # Maps the enter key to open_below then re-enter normal mode
"A-x" = "@x<A-d>" # Maps Alt-x to a macro selecting the whole line and deleting it without yanking it

[keys.insert]
"A-x" = "normal_mode"     # Maps Alt-X to enter normal mode
j = { k = "normal_mode" } # Maps `jk` to exit insert mode
```

## Minor modes

Minor modes are accessed by pressing a key (usually from normal mode), giving access to dedicated bindings. Bindings
can be modified or added by nesting definitions.

```toml
[keys.insert.j]
k = "normal_mode" # Maps `jk` to exit insert mode

[keys.normal.g]
a = "code_action" # Maps `ga` to show possible code actions

# invert `j` and `k` in view mode
[keys.normal.z]
j = "scroll_up"
k = "scroll_down"

# create a new minor mode bound to `+`
[keys.normal."+"]
m = ":run-shell-command make"
c = ":run-shell-command cargo build"
t = ":run-shell-command cargo test"
```

## Keys of pickers, menus, popups and prompts

The pickers (`Space f`, `Space /`, ...), the completion menu, popups (hover, signature help, ...),
the prompts (the `:` command line, the search prompt, ...), the
[explorer](./editor.md#editorexplorer-section), the [quickfix panel](./quicklist.md#quickfix-panel)
and a few more parts of the interface handle their keys themselves.
Their keys are configured in tables of their own: `[keys.picker]`, `[keys.menu]`, `[keys.popup]`,
`[keys.prompt]`, `[keys.explorer]` (the tree), `[keys.explorer-filter]` (the explorer's filter
box, while you type in it), `[keys.quickfix]`, and the others listed below. Every key such a
part handles is the key of an action in its table (or does what one does, like `Tab` beside
`Down` in a picker), so every one of them can be given to another key or taken away. A table
maps a key to the name of an *action* of that component:

```toml
[keys.picker]
C-j = "move_down"
C-k = "move_up"
C-o = "select_horizontal"
# take a key away from the picker
C-t = "no_op"

[keys.prompt]
C-o = "delete_word_back"

[keys.explorer]
space = "explorer_toggle_mark"   # mark with Space as well as m
D = "explorer_delete"
d = "no_op"                      # d no longer deletes
C-f = "explorer_filter"

[keys.explorer-filter]
C-j = "next_result"
C-k = "prev_result"

[keys.quickfix]
n = "move_down"
e = "move_up"
";" = "editor_key"      # `;` is your key for the command line: it goes to the normal keymap

[keys.changed-files]
C-a = "stage"

[keys.confirm]
o = "confirm"
```

The keys of the default bindings stay bound unless you bind them to another action or to
`no_op`. The tables of a workspace's `.helix/config.toml` are merged with the global ones; for
the same key the workspace wins. An unknown action or key is an error in the config, and the
message lists the actions that exist.

A table only changes keys of its own component: a key bound in `[keys.prompt]` also applies to
the prompt at the top of a picker, but not to the picker's own keys.

| Component | Action (default key) |
| --------- | -------------------- |
| `picker`  | `move_up` (`Up`), `move_down` (`Down`), `half_page_up` (`C-u`), `half_page_down` (`C-d`), `page_up` (`PageUp`), `page_down` (`PageDown`), `to_start` (`Home`), `to_end` (`End`), `select` (`Enter`), `select_in_background` (`A-Enter`), `select_horizontal` (`C-s`), `select_vertical` (`C-v`), `toggle_preview` (`C-t`), `copy_to_quicklist` (`C-q`), `copy_to_location_list` (`A-q`), `scroll_preview_up` (`A-k`), `scroll_preview_down` (`A-j`), `toggle_replace` (`A-r`, in global search), `replace_all` (`A-a`, also `C-Enter`), `close` (`Esc`) |
| `menu`    | `move_up` (`Up`), `move_down` (`Down`), `half_page_up` (`C-u`), `half_page_down` (`C-d`), `select` (`Enter`), `abort` (`Esc`) |
| `popup`   | `scroll_half_page_up` (`PageUp`), `scroll_half_page_down` (`PageDown`), `previous` (`A-p`) and `next` (`A-n`): the previous / next signature of signature help, or hover of another language server, `close` (`Esc`) |
| `explorer` | `explorer_open` (`l`, also `Right` and `Enter`), `explorer_open_vsplit` (`v`), `explorer_open_hsplit` (`s`), `explorer_collapse` (`h`, also `Left`), `explorer_parent_folder` (`-`), `explorer_first_row` (`Home`), `explorer_last_row` (`End`, also `G`), `explorer_reveal` (`.`), `explorer_filter` (`/`), `explorer_refresh` (`R`), `explorer_toggle_hidden` (`H`), `explorer_create` (`a`), `explorer_create_folder` (`A`), `explorer_rename` (`r`), `explorer_delete` (`d`), `explorer_cut` (`x`), `explorer_copy` (`c`), `explorer_paste` (`p`), `explorer_toggle_mark` (`m`), `explorer_mark_range` (`M`), `explorer_unmark_all` (`u`), `explorer_close` (`q`), `explorer_leave` (`Esc`). Each is also a command of the same name, to run it from anywhere (`[keys.normal]`, the command palette), and the names without `explorer_` (`create`, `reveal_buffer` for `explorer_reveal`) still work here |
| `explorer-filter` | `accept` (`Enter`), `next_result` (`Down`), `prev_result` (`Up`), `browse_results` (`Tab`), `clear` (`Esc`), `move_char_back` (`Left`), `move_char_forward` (`Right`), `move_start` (`Home`), `move_end` (`End`), `delete_char_back` (`Backspace`), `delete_char_forward` (`Delete`), `delete_word_back` (`C-w`), `kill_to_start` (`C-u`) |
| `prompt`  | `abort` (`Esc`), `submit` (`Enter`), `move_char_back` (`Left`), `move_char_forward` (`Right`), `move_word_back` (`A-b`), `move_word_forward` (`A-f`), `move_start` (`Home`), `move_end` (`End`), `delete_char_back` (`Backspace`), `delete_char_forward` (`Delete`), `delete_word_back` (`C-w`), `delete_word_forward` (`A-d`), `kill_to_start` (`C-u`), `kill_to_end` (`C-k`), `history_prev` (`Up`), `history_next` (`Down`), `complete_next` (`Tab`), `complete_prev` (`S-Tab`), `insert_word_under_cursor` (`C-s`), `insert_register` (`C-r`), `exit_selection` (`C-q`) |

| `quickfix` | The list of the [quickfix panel](./quicklist.md#quickfix-panel): `move_down` (`j`), `move_up` (`k`), `to_start` (`g`, also `Home`), `to_end` (`G`, also `End`), `open_and_stay` (`o`), `filter` (`/`), `window` (`C-w`, then a key of `quickfix-window`), `editor_key` (`:`, also `Space`, `C-h` and `C-l`: the key goes to the normal keymap as it is), `leave` (`Esc`), `close` (`q`). The other keys of the list, and all of them while its query is typed, are the picker's and the prompt's |
| `quickfix-window` | The key after the quickfix panel's `window` key: `up` (`k`: to the code), `left` (`h`) and `right` (`l`): to the explorer pinned on that side |
| `confirm` | A confirmation box: `confirm` (`y`), `cancel` (`n`, also `q` and `Esc`), `accept` (`Enter`: the button that has the focus), `switch` (`Tab`, also `Left`, `Right`, `h` and `l`: the other button) |
| `blame` | The blame details popup: `copy_commit` (`y`), `copy_message` (`Y`) |
| `plugin-window` | A [floating window of a plugin](./plugins.md): `move_down` (`j`), `move_up` (`k`), `half_page_down` (`C-d`), `half_page_up` (`C-u`), `page_down` (`PageDown`), `page_up` (`PageUp`), `to_start` (`g`, also `Home`), `to_end` (`G`, also `End`), `close` (`q`, also `Esc`). The keys the plugin binds come first |
| `file-explorer` | The file explorer picker (`Space e`, `Space .`): `create` (`A-n`), `move` (`A-m`), `delete` (`A-d`), `copy` (`A-c`), `yank_path` (`A-y`) |
| `changed-files` | The changed files picker (`Space g`): `stage` (`A-s`), `unstage` (`A-u`), `stash` (`A-z`), `stashes` (`A-l`), `commit` (`A-c`) |
| `stashes` | The stash picker: `apply` (`A-s`), `apply_with_index` (`A-i`) |
| `git-history` | The commits of a file or of the repository, and a commit's files: `yank_commit` (`A-y`), `open_version` (`A-o`), `back_to_commits` (`A-h`) |
| `quicklist` | The quicklist and location list pickers: `remove` (`C-x`) |

In all of them, `no_op` is also an action. The table of a picker's own keys comes before
`[keys.picker]`, and `[keys.quickfix]` before it for the keys of the panel's list.

In the normal keymap itself, `.` (repeat the last insert) is the command `repeat_last_insert`:
bind it to another key, or `.` to something else, like any command. The keys of the
[diff views](./diff.md#keys) are in `[editor.diff.keys]`.

Moving up and down the explorer's tree (`j`/`k`, `gg`, `C-d`, ...) is not in `[keys.explorer]`:
it uses your `[keys.normal]` keymap, so remapping those motions there moves the tree too. Keys
that `[keys.explorer]` does not take go on to the normal keymap (for example `C-h` /
`C-l`, `explorer_focus` / `explorer_blur`). A key taken away with `no_op` does nothing while the
tree has the keyboard: it doesn't go on to the buffer either. The status message after the
first mark and the hint in the empty filter box show your keys. Keys are written as described [below](#special-keys-and-modifiers).

## Special keys and modifiers

Ctrl, Shift and Alt modifiers are encoded respectively with the prefixes `C-`, `S-` and `A-`.

The [Super key](https://en.wikipedia.org/wiki/Super_key_(keyboard_button)) - the Windows/Linux
key or the Command key on Mac keyboards - is also supported when using a terminal emulator that
supports the [enhanced keyboard protocol](https://github.com/helix-editor/helix/wiki/Terminal-Support#enhanced-keyboard-protocol).
The super key is encoded with prefixes `Meta-`, `Cmd-` or `Win-`. These are all synonyms for the
super modifier - binding a key with a `Win-` modifier will mean it can be used with the
Windows/Linux key or the Command key.

```toml
[keys.normal]
C-s = ":write" # Ctrl and 's' to write
Cmd-s = ":write" # Cmd or Win or Meta and 's' to write
```

Special keys are encoded as follows:

| Key name         | Representation |
| ---              | ---            |
| Backspace        | `"backspace"`  |
| Space            | `"space"`      |
| Return/Enter     | `"ret"`        |
| Left             | `"left"`       |
| Right            | `"right"`      |
| Up               | `"up"`         |
| Down             | `"down"`       |
| Home             | `"home"`       |
| End              | `"end"`        |
| Page Up          | `"pageup"`     |
| Page Down        | `"pagedown"`   |
| Tab              | `"tab"`        |
| Delete           | `"del"`        |
| Insert           | `"ins"`        |
| Null             | `"null"`       |
| Escape           | `"esc"`        |
| Less Than (<)    | `"lt"`         |
| Greater Than (>) | `"gt"`         |

Keys can be disabled by binding them to the `no_op` command.

All other keys such as `?`, `!`, `-` etc. can be used literally:

```toml
[keys.normal]
"?" = ":write"
"!" = ":write"
"-" = ":write"
```

Note: `-` can't be used when combined with a modifier, for example `Alt` + `-` should be written as `A-minus`. `A--` is not accepted.
