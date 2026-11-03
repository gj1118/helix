# Configuration

To override global configuration parameters, create a `config.toml` file located in your config directory:

- Linux and Mac: `~/.config/helix/config.toml`
- Windows: `%AppData%\helix\config.toml`

> 💡 You can easily open the config file by typing `:config-open` within Helix normal mode.

Example config:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse = false

[editor.cursor-shape]
insert = "bar"
normal = "block"
select = "underline"

[editor.file-picker]
hidden = false
```

You can use a custom configuration file by specifying it with the `-c` or
`--config` command line argument, for example `hx -c path/to/custom-config.toml`.
You can reload the config file by issuing the `:config-reload` command. Alternatively, on Unix operating systems, you can reload it by sending the USR1
signal to the Helix process, such as by using the command `pkill -USR1 hx`.

Finally, you can have a `config.toml` and a `languages.toml` local to a project by putting it under a `.helix` directory in your repository.
Its settings will be merged with the configuration directory and the built-in configuration.


## Sections of `config.toml`

| Section | Purpose |
| --- | --- |
| `theme` | The [theme](./themes.md) |
| `[editor]` | Editor settings, see [Editor](./editor.md) |
| `[keys.normal]`, `[keys.insert]`, `[keys.select]` | Key bindings, see [Key remapping](./remapping.md) |
| `[keys.picker]`, `[keys.menu]`, `[keys.popup]`, `[keys.prompt]` | Keys of pickers, menus, popups and prompts, see [Key remapping](./remapping.md#keys-of-pickers-menus-popups-and-prompts) |
| `[commands]` | Your own `:` commands, see [Custom commands](./custom-commands.md) |
| `[icons]` | Icons of files, symbols, diagnostics, git and breakpoints, see [below](#icons) |
| `[plugins]` | Lua plugins, see [Plugins](./plugins.md). Read from the global `config.toml` only |

## Icons

Icons are configured in the top-level `[icons]` table (not under `[editor]`). They are off
until a group is enabled, and need a font with the glyphs, such as a
[Nerd Font](https://www.nerdfonts.com/).

```toml
[icons.mime]
enabled = true            # file type icons
directory = "📁"          # folders (also the explorer's closed folders)
directory-open = "📂"     # expanded folders in the explorer (default: open folder glyph, or `directory` when only that is set)
directory-empty = "🗀"    # empty folders in the explorer (default: outline folder glyph, or `directory`)
rs = "🦀"                 # by file extension
"README.md" = { glyph = "", color = "#7bab43" }   # by exact file name, with a color

[icons.kind]
enabled = true            # symbol kind icons
function = "ƒ"
struct = { glyph = "S", color = "#e5c07b" }

[icons.vcs]
enabled = true
branch = ""

[icons.diagnostic]
error = "■"

[icons.dap]
verified = "●"
unverified = "◯"

[icons.gutter]
added = "▍"
```

An icon is a string, or a table with `glyph` and an optional `color` (`"#rrggbb"`). Overrides
are merged with the built-in set.

| Table | Used in | Keys |
| --- | --- | --- |
| `[icons.mime]` | The buffer and jumplist pickers, the [bufferline](./editor.md#editorbufferline-section), the `file-type` statusline element and the [explorer](./editor.md#editorexplorer-section) | `enabled` (default `false`), `directory`, `directory-open`, `directory-empty`, and icons keyed by exact file name (`"Cargo.toml"`) or by file extension (`rs`), not by language name. A file without a match gets the `text` icon |
| `[icons.kind]` | The completion menu, the symbol pickers, the [breadcrumb bar](./editor.md#editorbreadcrumb-section) and the function name in the statusline | `enabled` (default `false`) and one key per kind: `file`, `folder`, `text`, `module`, `namespace`, `package`, `class`, `method`, `property`, `field`, `constructor`, `enum`, `interface`, `function`, `variable`, `constant`, `string`, `number`, `boolean`, `array`, `object`, `key`, `null`, `enum_member`, `struct`, `event`, `operator`, `type_parameter`, `color`, `keyword`, `value`, `snippet`, `reference`, `unit`, `word`, `spellcheck`, `workspace` |
| `[icons.vcs]` | The `version-control` statusline element (`branch`), the changed files picker and the explorer's git marks | `enabled` (default `false`), `branch`, `added`, `removed`, `modified`, `renamed`, `conflict`, `ignored` |
| `[icons.diagnostic]` | The diagnostics gutter, inline diagnostics and the statusline counts (always used) | `hint` (`○`), `info` (`●`), `warning` (`▲`), `error` (`■`) |
| `[icons.dap]` | Breakpoints in the gutter (always used) | `verified` (`●`), `unverified` (`◯`) |
| `[icons.gutter]` | The `diff` gutter (always used) | `added` (`▍`), `modified` (`▍`), `deleted` (`▔`) |

The `[icons]` tables of a workspace's `.helix/config.toml` are merged with the global ones.
