# Language servers

Helix has built-in support for the [Language Server Protocol][lsp], providing
IDE-like features — diagnostics, completion, goto-definition, rename and more,
for any language that has a server configured. Language servers are separate
programs you install yourself: the [language support](./lang-support.md) page
lists which languages ship with a server configured, and the
[language server configuration wiki][wiki] has per-server installation notes.

Run `hx --health <language>` to check whether Helix found the configured server
for a language. After changing a server's configuration, `:lsp-restart` reloads
it and `:lsp-stop` stops it; `:lsp-workspace-command` runs a command the server
exposes for the workspace.

## Features

Most features are bound by default (see the [keymap](./keymap.md)); the relevant
keys are listed below.

| Feature | Default key / command |
| --- | --- |
| Diagnostics — shown inline and in the gutter | navigate with `[d` / `]d`; `Space-d` / `Space-D` open the document / workspace pickers |
| Completion | automatic while typing; `Ctrl-x` triggers it manually |
| Hover documentation | `Space-k` |
| Signature help (parameter hints) | automatic while typing arguments |
| Goto definition / declaration / type / references / implementation | `gd` / `gD` / `gy` / `gr` / `gi` |
| Rename symbol | `Space-r` |
| Code actions | `Space-a` |
| Code lenses (run, test, reference counts) | enable with `display-code-lenses`; run one with `Space-A`, or pick one of the file with `Space-o` (see [below](#code-lenses)) |
| Document / workspace symbols | `Space-s` / `Space-S` |
| Format document | `:format`, or set `auto-format` to format on save |
| Inlay hints | enable with `display-inlay-hints` (see below) |

## Code lenses

Some language servers attach actions and facts to lines of code: rust-analyzer offers
`Run`, `Debug` and `Run Test` on functions and tests, and `3 implementations` or
`12 references` on definitions. These are *code lenses*. Showing them costs an extra
request to the server per document, so they are **off by default**:

```toml
[editor.lsp]
display-code-lenses = true
```

With the setting on, the lenses of a line appear after the end of the line, styled like inlay
hints and joined with `·`:

```text
fn main() {                        ▶ Run · Debug
fn parse(input: &str) -> Ast {     ▶ 12 references
```

Press `Space-A` (the `code_lens` command) to open a
menu with the lenses of the cursor line and run the chosen one on the server, the same way a
code action's command is run. The label is cut off when the line leaves no room for it, and
it is placed after inline blame when both are shown.

`Space-o` (`code_lens_picker`) lists the lenses of the whole file in a picker with a preview
of the code; it has the same border as the other pickers (including `[editor.gradient-borders]`).
Choosing an entry jumps to the lens and runs it.

How it works:

- Lenses are requested when a file is opened, shortly after you stop typing, when a server
  starts or exits, and when the server sends `workspace/codeLens/refresh` (rust-analyzer does
  this once it has finished indexing).
- A lens whose server did not fill in a title is resolved with `codeLens/resolve` once it
  scrolls into view; at most four resolves are in flight at a time.
- Between an edit and the server's new answer the labels stay where they were (they move
  with the text), but they are not run: the menu asks you to try again in a moment.
- A language can leave lenses to one of several servers, or opt a server out, with the
  `code-lens` [language server feature](./languages.md#configuring-language-servers-for-a-language).
- Style the labels with the `ui.virtual.code-lens` [theme](./themes.md) key; it falls back
  to `ui.virtual.inlay-hint`.

## Configuration

The [`[editor.lsp]`](./editor.md#editorlsp-section) section of `config.toml`
toggles LSP behaviour — `display-inlay-hints`, `auto-signature-help`,
`auto-document-highlight`, `snippets`, and `enable` to disable LSP entirely. How
much diagnostic detail is shown inline at the cursor is configured separately in
[`[editor.inline-diagnostics]`](./editor.md#editorinline-diagnostics-section).

Which server(s) a language uses, the server's command and arguments, and any
language-specific settings are configured in `languages.toml` (see
[Language Server configuration](./languages.md#language-server-configuration)). A
language may use [several servers](./languages.md#configuring-language-servers-for-a-language):
for each request the first server in the list that supports it is used, and a
server's features can be limited with `only-features` / `except-features`.

[lsp]: https://microsoft.github.io/language-server-protocol/
[wiki]: https://github.com/helix-editor/helix/wiki/Language-Server-Configurations
