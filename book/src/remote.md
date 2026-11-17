# Remote control

A running Helix can be told to open files and run commands from outside: from another
terminal, a file manager, a git tool, or a program started from Helix itself.

```sh
hx --remote src/main.rs:42          # open the file at line 42 in the running Helix
hx --remote --vsplit a.rs b.rs      # open them in vertical splits there
hx --remote-command 'write-all'     # run a `:` command there (with or without the `:`)
```

`--remote` prints what was opened and exits. When no Helix is running, `hx --remote file` opens
the file itself, like plain `hx file`: it can be your `EDITOR`. `--remote-command` without a
running Helix is an error.

## Which Helix

1. The one named with `--server <socket>`.
2. Else the one in `HELIX_LISTEN`. Every Helix sets it, for the programs it starts (`:sh`,
   `:insert-output`, shell pipes, a file manager run from it), to its own socket: those reach the
   Helix that started them.
3. Else the most recently started Helix that answers.

```toml
# for example, yazi's `yazi.toml`: when yazi runs from Helix, the files it opens go to that Helix
[opener]
edit = [{ run = 'hx --remote "$@"', block = false }]
```

## The socket

Each Helix listens on a Unix socket of its own, `hx-<process id>.sock` in
`$XDG_RUNTIME_DIR/helix` (or `helix-<user id>` in the temporary folder when that is not set).
The folder is made readable by its user only, and a folder of that name that belongs to someone
else is not used; the socket goes when Helix exits. `--listen <socket>` picks another path.

Anything that can use the socket can run any `:` command, `:sh` included: it is as powerful as
your user. To not listen at all:

```toml
[editor]
remote-control = false
```

Remote control is not available on Windows.

## The protocol

One request per connection, as a line of JSON, answered by a line of JSON
(`{"ok": true, "message": "Opened src/main.rs"}`):

```json
{"open": {"files": [["/abs/path/main.rs", [[41, 0]]]], "how": "replace"}}
{"command": {"line": "write-all"}}
"ping"
```

Positions are 0-based `[row, column]` pairs; `how` is `replace`, `vertical_split` or
`horizontal_split`.
