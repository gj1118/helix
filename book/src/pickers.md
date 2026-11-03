## Using pickers

Helix has a variety of pickers, which are interactive windows used to select various kinds of items. These include a file picker, global search picker, and more. Most pickers are accessed via keybindings in [space mode](./keymap.md#space-mode). Pickers have their own [keymap](./keymap.md#picker) for navigation.

### Filtering Picker Results

Most pickers perform fuzzy matching using [fzf syntax](https://github.com/junegunn/fzf?tab=readme-ov-file#search-syntax). Two exceptions are the global search picker, which uses regex, and the workspace symbol picker, which passes search terms to the language server. Note that OR operations (`|`) are not currently supported.

If a picker shows multiple columns, you may apply the filter to a specific column by prefixing the column name with `%`. Column names can be shortened to any prefix, so `%p`, `%pa` or `%pat` all mean the same as `%path`. For example, a query of `helix %p .toml !lang` in the global search picker searches for the term "helix" within files with paths ending in ".toml" but not including "lang".

You can insert the contents of a [register](./registers.md) using `Ctrl-r` followed by a register name. For example, one could insert the currently selected text using `Ctrl-r`-`.`, or the directory of the current file using `Ctrl-r`-`%` followed by `Ctrl-w` to remove the last path section. The global search picker will use the contents of the [search register](./registers.md#default-registers) if you press `Enter` without typing a filter. For example, pressing `*`-`Space-/`-`Enter` will start a global search for the currently selected text.

### Replacing from global search

The global search picker (`Space-/`) can also replace what it found. `Alt-r` shows a
replacement input under the query (and switches the focus between the two). While it is
shown:

| Key | Action |
| --- | --- |
| `Enter` | Replace the matches of the selected result's line |
| `Ctrl-Enter`, `Alt-a` | Replace the matches of every result that matches the picker's filters |
| `Alt-r` | Move the focus between the query and the replacement |

Regex captures expand in the replacement (`$1`, `${name}`) and `$$` is a literal `$`. Open
buffers are edited in place, unsaved changes included, as one undo step per buffer; other
files are rewritten on disk in the background; files that aren't UTF-8 are rewritten byte for
byte, keeping the bytes around the matches as they were. The status line reports how many
matches in how many files were replaced, and the picker runs its query again.

A file whose text changed since it was searched (a buffer edited since, or opened with unsaved
changes after its file was searched on disk) is skipped, since its results may point at other
lines now; the status line says how many were, and replacing again uses the refreshed results.

### File explorer

`Space-e` opens an interactive file explorer for browsing and opening files, rooted at the workspace; `Space-.` opens one rooted at the current buffer's directory. Unlike the file picker, the explorer does not ignore most files by default; its ignore behaviour is configured separately in the [`[editor.file-explorer]`](./editor.md#editorfile-explorer-section) section.

### Preview on narrow terminals

The preview sits beside the list when there is room for both. When the terminal is narrower than
about 144 columns, the preview is placed below the list instead, so both stay readable. There is
no option for this: the layout is chosen from the width of the terminal.

### Size

Pickers take 90% of the width and height of the screen (without the status line and the command
line). With [`fullscreen-overlay = true`](./editor.md#editor-section) they take all of it on
terminals narrower than 200 columns. The file picker with
[`file-picker.hide-preview = true`](./editor.md#editorfile-picker-section) shows only its list,
so it is narrower: 75% of the width and 90% of the height, whatever `fullscreen-overlay` says.
The [floating explorer](./editor.md#floating-explorer) has the same size.

### Previews

The preview shows the file under the selected item, scrolled to its line. Files bigger than
256 KiB are read in the background, so moving through big files does not stall the picker: the
preview says `<Loading…>` until the file is read. Files bigger than 10 MiB are not previewed.
Images are shown as images, see [`[editor.image-preview]`](./editor.md#editorimage-preview-section).
The preview can be scrolled with `PageUp`/`PageDown` and `Alt-k`/`Alt-j` (see the
[picker keys](./keymap.md#picker)).

### Live updates

Some pickers follow their data while they are open: the results are rebuilt when it changes,
the query stays as typed, and the selection stays on the same item when it still exists.

| Picker | Follows |
| --- | --- |
| File picker (`Space-f`, `Space-F`, ...) | Files created, deleted or renamed in the workspace |
| Global search (`Space-/`) | Changes to files in the workspace (the search runs again) |
| Changed files (`Space-g`) | The git status of the workspace |
| Buffer picker (`Space-b`) | Buffers opened, closed or modified |
| Diagnostics pickers (`Space-d`, `Space-D`) | New diagnostics from the language servers |
| Quicklist picker (`Space-q`) | Changes to the [quicklist](./quicklist.md) |

Changes on disk come from a file system watcher, started in the background when the picker
opens and stopped when it closes. On macOS and Windows it is one recursive watch on the
workspace. On Linux and the BSDs it watches each directory, walking the tree like the file
picker: ignored directories (`.gitignore`, `.ignore`), noise such as `target/` and
`node_modules/` and the inside of `.git` (except its `index` and `HEAD`, so staging is
noticed) are skipped, and at most 4096 directories are watched. In a linked git worktree or a
submodule, whose `.git` is a file pointing elsewhere, the `index` and `HEAD` of that git
directory are watched too. The results are checked at most every 250 ms.

### Opening in a split

`Enter` opens the selected item in the current window, `Ctrl-s` / `Ctrl-v` in a new
horizontal / vertical split. A file picker opened by `:vsplit <directory>` or
`:hsplit <directory>` opens files in that kind of split with `Enter` (and `Alt-Enter`) too.

### Command palette

`Space-?` lists every command once: the static commands, the typable commands (shown with a
leading `:`), your [custom commands](./custom-commands.md) (except hidden ones, with their
`desc`; one named like a built-in typable command replaces it in the list) and the commands
registered by [plugins](./plugins.md), with their key bindings.
Commands that open a picker or a prompt (`:open`, `file_picker`, `rename_symbol`, ...) work
from the palette: the picker or prompt opens once the palette has closed. Commands that wait
for another key (`replace`, `select_register`, the `m` and `t` motions, ...) cannot run from
the palette, which says so in the status line (`'replace' waits for another key and can't run
from the command palette`).
