# Quicklist

The quicklist collects locations you want to revisit, so you can jump between them without reopening a search. Unlike the [jumplist](./jumplist.md), which records places you leave, you choose what goes into the quicklist.

## Populating the quicklist

While a picker whose results are locations (it shows a preview) is open, press `Ctrl-q` to copy its currently matched entries into the quicklist. The entries keep the order of the picker and the picker stays open. For example, filter the global search, diagnostics or symbol results and capture only those matches.

The quicklist is shared by all views and lasts until you quit Helix. Copying picker results *replaces* the list; copying a picker with no matching locations clears it. Pickers whose results are not locations (such as the command palette) leave the list alone.

Press `Alt-q` in normal or select mode to add the current location, including your selections, to the end of the list (`add_to_quicklist`). The existing entries and the current position are kept. Buffers without a file are supported; their entries work for as long as the buffer is open.

Entries do not follow later edits. Visiting an entry added with `Alt-q` restores the selections saved at that time, visiting one copied from a picker selects the line of the result.

## Navigating entries

| Key       | Action                                                                  | Command                  |
| ---       | ---                                                                     | ---                      |
| `]q`      | Go to the next entry, wrapping at the end                               | `quicklist_next`         |
| `[q`      | Go to the previous entry, wrapping at the beginning                     | `quicklist_prev`         |
| `]l`      | Go to the next entry in the current file or buffer                      | `quicklist_next_in_file` |
| `[l`      | Go to the previous entry in the current file or buffer                  | `quicklist_prev_in_file` |
| `Space q` | Open the quicklist picker                                               | `quicklist_picker`       |
| `Alt-q`   | Add the current location                                                | `add_to_quicklist`       |

The navigation commands accept a count: `3]q` goes forward three entries. The place you jump from is added to the jumplist, so `Ctrl-o` goes back. Before the first jump the list has no position, so `]q` goes to the first entry and `[q` to the last.

The quicklist picker lists every entry and marks the current one with `*`. Selecting an entry opens it and makes it the current one.

## Removing entries

In the quicklist picker press `Ctrl-x` to remove the highlighted entry. The picker stays open and keeps its filter. This only removes the list entry; the file or buffer is not touched.

Removing the current entry leaves the editor cursor where it is. After closing the picker `]q` visits the entry that followed the removed one and `[q` the one before it.

## Typical workflow

1. Open a picker that shows locations, such as global search, diagnostics or symbols.
2. Filter it until it contains the locations you care about.
3. Press `Ctrl-q` to copy the matches into the quicklist.
4. Close the picker and walk through the locations with `]q` and `[q`.
5. Use `]l` and `[l` to stay in the current file, or `Space q` to browse the whole list.

## Quickfix panel

With `[editor.quickfix] enable = true`, the lists of locations open in a panel at the bottom of
the editor, like Vim's quickfix window, instead of in a picker: the global search, the buffer
search (`Space l`; the fuzzy `Space L` stays a picker), diagnostics (of the buffer and the workspace), document and workspace symbols (from the
language server or tree-sitter), the results of goto definition, declaration, type definition,
implementation and references when there are several, the changed files, the jumplist, the
task tags (TODO, ...) and the quicklist (`Space q`). The pickers that choose one thing (files,
buffers, commands, themes, ...) stay pickers, and so does the floating explorer.

```toml
[editor.quickfix]
enable = true   # off by default: the lists are pickers
height = 12     # rows of the panel, its border included
preview = true  # preview the selected location over the code while the panel has the focus
```

The panel takes the bottom rows of the views (it does not cover the code, and the statusline
stays above it) and keeps the columns, filtering and live updates of the picker. Being part
of the editor's frame rather than a window over it, the panel has no border (its title is on
the rule at its top, the query under it), and its preview takes the place of the code, above
the statusline, without a border either (also with `gradient-borders` on). A new list
replaces the one it shows. It opens with the focus: on its list, or on its query for a list
that needs one to show anything (the global search).

| Key (in the panel) | Action |
| --- | --- |
| `j`, `k`, `Down`, `Up`, `Ctrl-n`, `Ctrl-p`, `Tab`, `Shift-Tab` | Move through the list |
| `g`, `G`, `Home`, `End`, `Ctrl-d`, `Ctrl-u`, `PageDown`, `PageUp` | First and last entry, page down and up |
| `Enter` | Visit the entry; the focus goes to the code and the panel stays |
| `o`, `Alt-Enter` | Visit the entry, keeping the focus in the panel |
| `Ctrl-s`, `Ctrl-v` | Visit the entry in a horizontal or vertical split |
| `/` | Type the query (filter the list); `Esc` or `Enter` gives the keys back to the list |
| `Esc`, `Ctrl-l`, `Ctrl-w k` (or `Ctrl-w w`, `Ctrl-w p`) | Give the focus to the code; the panel stays |
| `Ctrl-h` | Give the focus to the explorer (opening it if hidden) |
| `Ctrl-w h` / `Ctrl-w l` | Give the focus to the explorer, when it is pinned on that side |
| `Space Q` | Hide the panel (the space menu works from the panel) |
| `Ctrl-q` | Copy the matched entries to the quicklist |
| `Ctrl-t` | Turn the preview on or off |
| `q`, `Ctrl-c` | Close the panel |
| `:` | The command line |

`Space Q` (`quickfix`, `:quickfix`) shows the panel with the focus (the last list it showed,
or the quicklist) and hides it when it is open, like `Space x` for the explorer; each only
toggles its own panel. `]q` / `[q` visit the next and previous entry of the panel from the
code (while it is shown; `]l` and `[l` keep walking the quicklist in the file). `:copen`
focuses the panel, or shows again the last list it showed, or else shows the quicklist,
whether `enable` is on or not; `:cclose` closes it.

### Moving between the code and the panel

**From the code to the panel:**

- `Ctrl-w j` from the bottom view (with splits stacked, press it until you reach the bottom one).
- `Space Q` (`:quickfix`) when the panel is hidden: it opens with the focus. When the panel is
  open, `Space Q` hides it instead, so use `Ctrl-w j` then.
- `:copen` focuses the panel, opening it if needed.
- A click in the panel focuses it and selects the entry clicked.

**From the panel back to the code:**

- `Esc`, `Ctrl-l` or `Ctrl-w k` (also `Ctrl-w w`, `Ctrl-w p`): the focus goes to the code and
  the panel stays open. While typing a filter (after `/`), the first `Esc` leaves the filter
  and the second one goes to the code.
- `Enter` visits the selected entry and gives the focus to the code.
- `q` (or `Ctrl-c`) closes the panel.
- A click in the code gives the focus back.

For one key that always goes to the panel (opening it if needed, never hiding it), bind
`quickfix_focus`:

```toml
[keys.normal]
"C-j" = "quickfix_focus"   # any free key; C-j is move_lines_down by default
```
