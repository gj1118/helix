## Selecting and manipulating text with textobjects

In Helix, textobjects are a way to select, manipulate and operate on a piece of
text in a structured way. They allow you to refer to blocks of text based on
their structure or purpose, such as a word, sentence, paragraph, or even a
function or block of code.

![Textobject demo](https://user-images.githubusercontent.com/23398472/124231131-81a4bb00-db2d-11eb-9d10-8e577ca7b177.gif)
![Textobject tree-sitter demo](https://user-images.githubusercontent.com/23398472/132537398-2a2e0a54-582b-44ab-a77f-eb818942203d.gif)

- `ma` - Select around the object (`va` in Vim, `<alt-a>` in Kakoune)
- `mi` - Select inside the object (`vi` in Vim, `<alt-i>` in Kakoune)

| Key after `mi` or `ma` | Textobject selected      |
| ---                    | ---                      |
| `w`                    | Word                     |
| `W`                    | WORD                     |
| `p`                    | Paragraph                |
| `(`, `[`, `'`, etc.    | Specified surround pairs |
| `m`                    | The closest surround pair    |
| `f`                    | Function                 |
| `t`                    | Type (or Class)          |
| `a`                    | Argument/parameter       |
| `c`                    | Comment                  |
| `T`                    | Test                     |
| `e`                    | Entry (of a list, map, ...) |
| `g`                    | Change                   |
| `x`                    | (X)HTML element          |

> 💡 `f`, `t`, etc. need a tree-sitter grammar active for the current
document and a special tree-sitter query file to work properly. [Only
some grammars](./lang-support.md) currently have the query file implemented.
Contributions are welcome!

`mi` and `ma` are submodes in this fork: the keys above each run their own command
(`select_textobject_inside_function`, ...), and any other key selects inside or around the
pair of that character. `mA` / `mI` select all the textobjects of a kind around or inside the
selections.

`]` and `[` followed by a pair character select inside the **next** / **previous** pair of it,
without the cursor being inside one: `](` selects the inside of the next `(...)`, `["` the
inside of the previous `"..."`. A count picks the n-th pair. Keys that are bound under `]` and
`[` (`]f`, `[d`, ...) keep their meaning.

## Navigating using tree-sitter textobjects

Navigating between functions, classes, parameters, and other elements is
possible using tree-sitter and textobject queries. For
example to move to the next function use `]f`, to move to previous
type use `[t`, and so on.

![Tree-sitter-nav-demo](https://user-images.githubusercontent.com/23398472/152332550-7dfff043-36a2-4aec-b8f2-77c13eb56d6f.gif)

For the full reference see the [unimpaired](./keymap.md#unimpaired) section of the key bind
documentation.

> 💡 This feature relies on tree-sitter textobjects
> and requires the corresponding query file to work properly.

## Folding text objects

Folding hides the body of a text object (a function, a comment, a block) and shows only its first line,
so you can read the structure of a file without the details. Folded text still takes part in movement
and selection: moving over a fold steps past its hidden lines.

| Key                    | Command       | Action                                                          |
| ---                    | ---           | ---                                                             |
| `z f` / `Z f`          | `fold`        | Fold the text object under the cursor                           |
| `z F` / `Z F`          | `unfold`      | Unfold the folds under the cursor                               |
| `z Alt-f` / `Z Alt-f`  | `toggle_fold` | Fold or unfold the text object at the primary cursor            |
| `z i` / `Z i`          | `fold_indent` | Fold the block of more indented lines the cursor is in          |
| `z M` / `Z M`          | `fold_all`    | Fold everything in the document                                 |
| `z R` / `Z R`          | `unfold_all`  | Unfold everything                                               |

Languages without tree-sitter text objects (plain text, YAML, many config formats) fold
too: there `z f` and `z M` use the folding ranges of the buffer's language server when it has
them (the `folding-ranges` [feature](./languages.md)), else the indentation. A block by
indentation is a line and the lines after it that are more indented (blank lines inside
included): `z f` folds the innermost block the cursor is in, the line it heads staying visible.
`z i` folds by indentation in any language.

`:fold` and `:unfold` do the same from the command line. `:fold` takes the names of text objects as
positional arguments, and these flags:

- `-s` / `--selection`: fold the lines of the current selection (it must span at least two lines and end
  with a line ending). It cannot be combined with the other flags or with text object names.
- `-d` / `--document`: fold every text object of the names given in the entire document.
- `-a` / `--all`: fold all text objects in the document, except the names given.
- `-i` / `--indent`: fold the block of more indented lines the cursor is in (with `-d`, every
  block, nested ones included).
- `-l` / `--lsp`: fold the language server's folding range the cursor is in (with `-d`, every
  range). The ranges are asked for when the command runs; the folds appear when they come back.
- `--auto`: text objects when the language has them, else the language server's folding ranges,
  else the indentation. `z f` and `z M` use it.

`:unfold` takes `-s` (the folds made from a selection, by indentation or from the language
server), `-d`, `-a`, `-r` (nested folds too) and `--auto` (folds of every kind: `z F` and
`z R`).

```text
:fold -d comment
:fold -a comment
:fold -s
:fold --indent --document
:fold --lsp
```

To fold text objects automatically whenever a document is opened, list them in `fold-textobjects` (see
[`[editor]`](./editor.md)):

```toml
[editor]
fold-textobjects = ["comment"]  # fold all comments on open
```

