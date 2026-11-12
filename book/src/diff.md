# Diffs

Helix shows the changes between two versions of a file side by side: the old version on
the left, the new one on the right, and a connector between them that links each change on
the left to the same change on the right.

Each side keeps its own lines and line numbers. Nothing is inserted to line the sides up;
the connector bends instead, from a change's lines on the left to its lines on the right.
The two sides scroll together: scroll one (with keys, the mouse wheel, a jump) and the other
follows, the line matching the cursor's line level with it (`scroll-sync`).
Changed lines have a colored background (added, removed, changed), and inside a changed line
the words that changed are emphasized (not in very big changes: more than 200 lines, or more
than 16,000 characters on a side, such as a minified file, where it would cost a lot and show
little).

The left side is a snapshot of the old version, with syntax highlighting, and it can't be
changed: typing, deleting or pasting there is refused ("The old version is read-only"). The
right side is the document itself, editable as usual: completion, diagnostics, undo and
`:write` all work, and the diff follows every edit. While you type, the changes shown move
along with your edits; the diff is worked out again when you pause (after `idle-timeout`),
when you leave insert mode, and before `]c`, `>>` and the other diff keys.

## Opening a diff

| Command | Compares |
| --- | --- |
| `:diff <old> <new>` | Two files |
| `:diff <old>` | A file with the current buffer |
| `:diff-buffers <old> [<new>]` | Two buffers, by name or path (`<new>` defaults to the current buffer; `<old>` is a snapshot taken now) |
| `:diff-git` | The current file with its version in the git index: what `git diff` shows |
| `:diff-git <revision>` | The current file with its version at a revision: `:diff-git HEAD` (staged and unstaged changes), `:diff-git main`, `:diff-git HEAD~3` |
| `hx --diff <old> <new>` | Two files, from the command line |

A file git doesn't have at that version is compared with nothing: all of it is added.

A diff opens in the current window, with a new window on its left for the old version. The
cursor stays on the right side. `:diff-close` (or `q` in the diff) closes the old version's
window and gives the right one back its gutters; closing either window does the same.

## Reviewing changed files

The changed files of a repository can be gone through one after the other, each in its diff:
`]f` and `[f` show the next and previous one in the same windows (with a count: `3]f`), and
the status line says which (`File 2 of 7: src/main.rs`).

| How | Reviews |
| --- | --- |
| Open a file from the changed files list (`Space g`) | The changed files, from that one |
| `:diff-review` | The changed files, from the first |
| `:diff-review <revision>` | The files changed since a revision (and the new ones): `:diff-review main` |

The changed files list shows in the quickfix panel when `[editor.quickfix] enable` is on, and
as a picker otherwise. Each file is compared with `HEAD` (a new file with nothing), so its
staged and not staged changes all show, each marked (see [Staging](#staging)). With
`:diff-review <revision>` the files are compared with that revision. The review covers the
working directory, as the changed files list does, or the repository of the current file when
that file is elsewhere.

## Staging

In a diff against `HEAD` (a review, or `:diff-git HEAD`), a mark in front of each change says
whether it is staged: `▣` staged, `□` not staged, `◩` partly staged (staged, then changed
again). `-` stages the change under the cursor, or unstages it when it is staged; a partly
staged change is staged whole. What is staged is the change as the right side shows it, saved
or not: the index takes those lines, as `git add -p` would put them (through the file's
filters, line endings and the like). Staging a new file puts it in the index; unstaging all of
it takes it out again.

In a diff against the index (`:diff-git`), only what isn't staged shows: `-` stages the change
under the cursor, which then leaves the diff. Against another revision (`:diff-review main`,
`:diff-git HEAD~2`) nothing can be staged.

Staging runs git, which needs a trusted workspace (see `:workspace-trust`).

With `open-changed-files-in-diff = false` (see [Settings](#settings)), opening a file from the
changed files list opens it as a plain buffer; `:diff-review` still reviews.

Outside a review, `]f` and `[f` keep their usual meaning (the next and previous function).

## Keys

In the windows of a diff, these keys come first (normal mode):

| Key | Action |
| --- | --- |
| `]c`, `[c` | The next / previous change (with a count: `3]c`). The other side scrolls to the same change, at the same height |
| `]f`, `[f` | In a review of changed files: the next / previous file (see [Reviewing changed files](#reviewing-changed-files)) |
| `-` | Stage the change under the cursor, or unstage it (see [Staging](#staging)) |
| `]s` | Line the other side up with the cursor's line (for when `scroll-sync` is off) |
| `[d`, `]d` | Both sides to their first / last line |
| `>>` | Revert the change under the cursor: the right side gets the left side's lines, from either side (undo with `u`, as any edit) |
| `q` | Close the diff |

Other keys work as usual, those that begin like a diff key too: `]f` still goes to the next
function, and `>` followed by a motion still indents. They are the actions of the
`[editor.diff.keys]` table, which takes other keys, or `""` to turn one off:

```toml
[editor.diff.keys]
close = "Q"            # `q` replays a macro again in diffs
next-change = "<C-n>"
previous-change = "<C-p>"
apply-left-to-right = "" # no key
```

The actions are `next-change`, `previous-change`, `next-file`, `previous-file`, `snap`,
`align-top`, `align-bottom`, `apply-left-to-right`, `stage` and `close` (`>` and `<` are
written `<gt>` and `<lt>`). They are also commands, which can be bound to keys anywhere like
any other: `diff_next_change`, `diff_previous_change`, `diff_next_file`,
`diff_previous_file`, `diff_snap`, `diff_align_top`, `diff_align_bottom`,
`diff_apply_left_to_right`, `diff_stage` and `diff_close`.

## Settings

```toml
[editor.diff]
ignore-whitespace = false  # lines that differ only in whitespace are not changes
connector-width = 9        # columns of the connector
align-on-jump = true       # `]c`/`[c` scroll the other side to the same change
scroll-sync = true         # scrolling one side scrolls the other; false: each on its own
open-changed-files-in-diff = true # files opened from `Space g` show their diff; false: a buffer

[editor.diff.stage-markers] # the marks of staged, not staged and partly staged changes
staged = "▣"
unstaged = "□"
partial = "◩"
```

The colors come from the theme: `ui.diff.add`, `ui.diff.delete`, `ui.diff.change` and
`ui.diff.change.emphasis` (backgrounds). A theme without them gets backgrounds mixed from its
`diff.plus`, `diff.minus` and `diff.delta` colors (the ones of the diff gutter) and its
background, never so faint that they don't show. A monochrome theme, whose diff colors are
greys, gets green, red and blue tints instead (greys would all look the same); to keep it
grey, set the `ui.diff.*` keys in the theme:

```toml
"ui.diff.add" = { bg = "#2e2e2c" }
"ui.diff.delete" = { bg = "#1a1a19" }
"ui.diff.change" = { bg = "#262624" }
"ui.diff.change.emphasis" = { bg = "#444442" }
```

## Helix as git's diff tool

`git difftool` can show each changed file in Helix, one after the other (`:q` goes to the
next):

```sh
git config --global difftool.helix.cmd 'hx --diff "$LOCAL" "$REMOTE"'
git config --global diff.tool helix
git config --global difftool.prompt false

git difftool              # the changes not staged
git difftool --staged     # the staged ones
git difftool main         # against a branch
git difftool HEAD~1 HEAD  # a commit
```

The right side is then the file in your working tree when git compares with it, so edits
there are kept; otherwise it is a temporary file of git's.
