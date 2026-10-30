# Snippets

Snippets are reusable pieces of text with tabstops and variables. They are defined in TOML, inserted from a picker, with a command, or from the completion menu by typing a prefix, and use the same syntax as language server snippets.

## Defining snippets

Snippets are written as `[[snippet]]` tables:

```toml
[[snippet]]
name = "fn"
prefix = ["fn", "func"]
scope = "rust"
description = "A function"
body = [
  "fn ${1:name}($2) {",
  "\t$0",
  "}",
]

[[snippet]]
name = "todo"
prefix = "todo"
body = "$LINE_COMMENT TODO(${1:$TM_FILENAME_BASE}): $0 ($CURRENT_YEAR-$CURRENT_MONTH-$CURRENT_DATE)"
```

| Key           | Description                                                                                                  |
| ---           | ---                                                                                                          |
| `name`        | Required. Unique name of the snippet.                                                                        |
| `body`        | Required. The snippet text, either a string or an array of lines that are joined with newlines.              |
| `prefix`      | A string or a list of strings that trigger the snippet in completion. A prefix can't contain whitespace.    |
| `scope`       | A language name or a list of language names (e.g. `rust`). Without it the snippet is offered in every language. |
| `description` | Shown in the picker and next to the snippet in the completion menu.                                          |

Unknown keys, an empty body and a prefix with whitespace are errors. Like in language server snippets, malformed placeholder syntax such as an unterminated `${1:` is inserted as plain text.

### Where snippets are loaded from

Snippets are read from

- `snippets.toml` and every `.toml` file below the `snippets/` directory (including subdirectories) in the Helix config directory, and
- `.helix/snippets.toml` and `.helix/snippets/` of the workspace. These are only loaded when the workspace is [trusted](./workspace-trust.md). A workspace snippet replaces a user snippet with the same name.

Names must be unique: two definitions with the same name in the user files (or in the workspace files) are an error naming both files. Symlinks to files are followed, symlinked directories are not.

Snippets are loaded at startup, by `:config-reload` and by `:snippets-reload`. If loading fails, the error is shown and the previously loaded snippets stay active.

## Using snippets

| Command                          | Description                                                                                |
| ---                              | ---                                                                                        |
| `snippet_picker` (`Space i`)     | Pick a snippet to insert. Only snippets for the current language are listed.               |
| `:snippet-insert <name>`         | Insert the snippet with that name.                                                         |
| `:snippets-open [file]`          | Open `snippets.toml`, or `snippets/<file>.toml` of the config directory, creating it if needed. |
| `:snippets-open-workspace [file]`| Like `:snippets-open` for `.helix/` in the workspace.                                      |
| `:snippets-reload`               | Reload the snippets from disk.                                                             |

Inserting a snippet replaces each selection that is longer than one character (in select mode any non-empty selection), otherwise the snippet is inserted at the cursor. Afterwards Helix is in insert mode and <kbd>Tab</kbd> / <kbd>Shift-Tab</kbd> move between the tabstops.

### Completion

While typing, the completion menu offers the snippets whose prefix starts with what you typed before the cursor (a prefix must start at a word boundary). Accepting one replaces the typed text, at every cursor that has the same text before it. A manual completion request (<kbd>Ctrl-x</kbd>) with nothing typed lists all snippets that have a prefix.

Set `snippet-completion = false` in `[editor]` to turn this off.

## Variables

The syntax is `$NAME`, `${NAME}`, `${NAME:default}` and `${NAME/regex/format/options}`. A variable that is unknown, or has no value (for example `$TM_FILENAME` in a buffer without a file), expands to its default or to nothing.

Variables that depend on the cursor are evaluated for every cursor, so each cursor of a multi-cursor insert gets its own value. Random values and the time are generated once per insertion: the same variable used twice in one cursor's text has the same value, but each cursor gets a different random value.

**Selection and cursor**

| Variable                                | Value                                                                       |
| ---                                     | ---                                                                         |
| `TM_SELECTED_TEXT`, `SELECTION`         | The selected text (nothing when there is no selection).                     |
| `TM_CURRENT_LINE`                       | The text of the line the cursor is on.                                      |
| `TM_CURRENT_WORD`                       | The word around the cursor.                                                 |
| `TM_LINE_INDEX`, `TM_LINE_NUMBER`       | The zero-based and one-based line of the cursor.                            |
| `CURSOR_INDEX`, `CURSOR_NUMBER`         | The zero-based and one-based number of the cursor among all the cursors.    |
| `CLIPBOARD`                             | The system clipboard.                                                       |

**File and workspace**

| Variable                                                  | Value                                                  |
| ---                                                       | ---                                                    |
| `TM_FILENAME`, `TM_FILENAME_BASE`                         | The file name, with and without the last extension.    |
| `TM_DIRECTORY`, `TM_FILEPATH`                             | The directory and the full path of the file.           |
| `RELATIVE_FILEPATH`                                       | The path relative to the workspace.                    |
| `WORKSPACE_NAME`, `WORKSPACE_FOLDER`                      | The name and the path of the workspace.                |
| `LINE_COMMENT`, `BLOCK_COMMENT_START`, `BLOCK_COMMENT_END`| The comment tokens of the buffer's language.           |

**Date and time** (local time)

`CURRENT_YEAR`, `CURRENT_YEAR_SHORT`, `CURRENT_MONTH`, `CURRENT_MONTH_NAME`, `CURRENT_MONTH_NAME_SHORT`, `CURRENT_DATE` (the day of the month), `CURRENT_DAY_NAME`, `CURRENT_DAY_NAME_SHORT`, `CURRENT_HOUR`, `CURRENT_MINUTE`, `CURRENT_SECOND`, `CURRENT_MILLISECOND`, `CURRENT_SECONDS_UNIX`, `CURRENT_MILLISECONDS_UNIX`, `CURRENT_TIMEZONE_OFFSET` (`+02:00`) and `CURRENT_TIMEZONE_NAME`.

On platforms where the local time zone can't be determined (currently everything but Unix) the time is UTC.

**Random**

| Variable     | Value                           |
| ---          | ---                             |
| `RANDOM`     | Six random decimal digits.      |
| `RANDOM_HEX` | Six random hexadecimal digits.  |
| `UUID`       | A random version 4 UUID.        |

The random values are not suitable for anything security sensitive.

### Wrapping the selection

```toml
[[snippet]]
name = "dbg"
body = "dbg!(${TM_SELECTED_TEXT})$0"
```

Select an expression and run `:snippet-insert dbg`: the selection is replaced by `dbg!(<selection>)`.
