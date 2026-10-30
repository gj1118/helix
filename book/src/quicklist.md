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
