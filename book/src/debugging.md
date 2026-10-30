# Debugging

Helix talks to debug adapters (DAP). Adapters and the launch templates are configured per
language, see [languages.md](./languages.md#debugger-configuration). Everything here is under
`Space G` (see the [keymap](./keymap.md#space-mode)); the commands that take arguments are
typable (`:debug-start`, `:debug-eval`, ...).

## Variables

`Space G v` opens the variables of the selected stack frame in a picker. Scopes and variables
that have children are marked `▸`; pressing `Enter` on one opens its children, and `.. (back)`
(the first row of every level below the scopes) goes back up. The filter you typed and the
selected row of a level are restored when you come back to it.

Variables with many children (a 250 element array, say) are loaded 100 at a time when the
adapter says how many there are: the last row, `Load more… (N more)`, fetches the next page.

`:debug-eval <expression>` evaluates an expression in the selected frame and shows
`expression = result (type)` in the status line. When the result has children it also opens the
variable browser on them.

## Stack frames

`Space G s f` lists the frames of the current thread. Deep stacks are fetched 20 frames at a time,
the last row (`Load more…`) fetches the next 20.

## Inline values

While the debuggee is stopped, the lines above and including the current line of the selected
frame show the values of the variables they use at their end:

```
total = price * count      total = 30, price = 10, count = 3
```

Names are matched as words, without parsing the language, so a name in a comment or in a string
shows a value too. At most four values are shown per line, long values are cut. Values of
expensive scopes (globals, registers) are not shown. The values are hidden for a file with
unsaved changes (the line numbers would not match the running program) and removed when the
program continues, steps or ends.

They are on by default:

```toml
[editor.debugger]
inline-values = true
```

`:toggle debugger.inline-values` switches them for the session. The style comes from the theme
key `ui.virtual.inline-value` and falls back to `ui.virtual.inlay-hint`.

## Several sessions

More than one debug session can run at the same time: start another with `:debug-start` while
one is running, or let the adapter start child sessions (`startDebugging`, for adapters that use
the TCP transport). The newest session is selected.

- `Space G s s` opens the session picker. The selected session is marked with `*`, child
  sessions are indented below their parent, the state is `running` or `stopped`. Choosing a
  session selects it and jumps to where it is stopped.
- Stepping, continuing, evaluating, the variable browser and terminating (`Space G t`) apply to
  the selected session.
- A session that stops while the selected one is running becomes the selected one.
- When the selected session ends, another one (a stopped one if there is one) is selected.
- The statusline element `debug-session` (on the right of the default statusline) shows
  `dbg <name> <state>` and the number of sessions when there is more than one.

## Sources from the adapter

Code that does not exist as a file (generated code, eval'd strings, decompiled classes) is
described by the adapter with a `sourceReference` instead of a path. When a frame is in such a
source, Helix requests its text and opens it in a read-only buffer. The buffer is not a file:
it cannot be saved. Breakpoints can be set in it with `Space G b`; they are sent to the adapter
by reference.

The buffer, and the breakpoints in it, last until the adapter reports (`loadedSource` event) that
the source changed or was removed, or until the session ends.
