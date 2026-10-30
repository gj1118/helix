# Custom commands

Custom commands define short command-mode (`:`) commands in `config.toml`. A custom command can run one or more built-in typable or static commands, or a single key macro.

```toml
[commands]
":wq" = [":write", ":quit"]
":f" = ":format"
":W" = ":write!"
":hints" = ":toggle lsp.display-inlay-hints"
```

Typable commands start with `:`. Static commands, such as `move_char_right`, use their bare names. Key macros start with `@`, for example `@miw`. A macro must be the only item in a custom command.

The available typable and static commands are listed in the [Commands](./commands.md) and [Keymap](./keymap.md) documentation.

Mapped typable commands must be built-ins; custom commands cannot invoke other custom commands. Commands run in order and stop at the first one that fails. Definitions are checked when the configuration loads, so a misspelled command is reported then rather than when you run it.

Custom commands can also be used in key bindings: `X = ":wq"` under `[keys.normal]` runs the custom command.

## Arguments

Use `%arg{n}` to pass a positional argument to a mapped typable command. Argument indexes start at zero.

```toml
[commands]
":line" = ":goto %arg{0}"
```

Running `:line 42` executes `:goto 42`. Missing arguments expand to an empty
string. An argument is substituted as one value, so `:say "two words"` passes a
single argument to `%arg{0}` even when the mapped command is not quoted. See
[Command mode syntax](./command-line.md#expansions) for other expansions.

## Descriptions and completion

Use the detailed form to document accepted arguments and inherit completion from a built-in command:

```toml
[commands.":save-as"]
commands = [':write "%arg{0}"']
desc = "Save the current buffer to a path"
accepts = "<path>"
completer = ":write"
```

The description is shown in the command prompt once the whole command name is typed
(with the `bottom` command line style), followed by the accepted arguments.
Here `:save-as` uses `:write`'s path completion. For example,
`:save-as "meeting notes.txt"` saves to a path containing a space.

## Shadowing built-ins

Custom commands take precedence over built-ins. Prefix a command with `^` to bypass a custom definition:

```toml
[commands]
":w" = ":write!"
```

With this configuration, `:w` force-writes while `:^w` retains the built-in `:write` behavior.

## Hidden commands

Commands whose configuration key starts with `:` appear in command-mode completion. Omit that prefix to keep a command usable but hidden:

```toml
[commands]
"0" = ":goto 1"
```

The command remains available as `:0` but does not appear in completion. A custom command named like a number takes precedence over `:<line>`.

## Workspace configuration

A workspace's `.helix/config.toml` can define commands too; they are loaded only for [trusted workspaces](./workspace-trust.md) and replace a global command of the same name.

## Limits

* Static commands that wait for another key (such as `r` or `f`) cannot be run from a custom command; use a key macro (`@rx`) instead.
* Static commands and macros run after the command prompt has closed, so a later typable command in the same list does not wait for them.
* The command prompt's description box is not shown with `cmdline.style = "popup"`; completion is.
