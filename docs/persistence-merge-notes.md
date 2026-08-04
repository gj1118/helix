# Persistence merge notes

How cross-session history was ported from upstream PR
[helix-editor/helix#9143](https://github.com/helix-editor/helix/pull/9143)
(`intarga/helix` branch `persistent_state`) onto this gj1118-based tree.

Published branch on this fork: **`persistent_state`**
([bellhyve/helix](https://github.com/bellhyve/helix/tree/persistent_state)).
Frozen upstream tip: `vendor/intarga-persistent_state` (`ea5d2df1`).

**Purpose of this doc:** simplify rebase after a gj1118/`master` bump.

**First land commit:** `0f409a7a` (merge of `intarga/persistent_state` + conflict
resolution + one behavioral fix).

---

## What this feature is

Opt-in persistence under `[editor.persistence]`:

| Key | Effect | Default |
|-----|--------|---------|
| `commands` | `:` history survives quit | `false` |
| `search` | `/` history survives quit | `false` |
| `old-files` | reopen file at last cursor/view | `false` |
| `clipboard` | Helix internal `"` register | `false` |
| `*-trim` | max entries kept on startup trim | `100` |
| `old-files-exclusions` | regex paths skipped for old-files | git + `COMMIT_EDITMSG` |

On-disk format: **bincode**, append/rewrite under XDG state:

- Unix: `~/.local/state/helix/`
- files: `command_history`, `search_history`, `file_history`, `clipboard`

Command: `:reload-history` — re-read histfiles into the live session (and
re-queue trim jobs).

**Not** session restore of splits/buffers. **Not** a plugin — it touches
registers, `Editor::open`/`close`/`close_document`, startup, and config.

Upstream status as of 2026-08: PR closed unmerged (author fatigue / no
maintainer review). Branch still existed at `intarga/helix:persistent_state`
(`ea5d2df1`).

---

## Source map from #9143

New files from #9143 (probably keep whole):

- `helix-loader/src/persistence.rs` — bincode read/write/trim
- `helix-view/src/persistence.rs` — register + file-history helpers
- `helix-view/src/regex.rs` — `EqRegex` wrapper for config exclusions
- `helix-term/tests/test/persistence.rs` — multi-session integration test
- book section: `book/src/editor.md` → `[editor.persistence]`

Conflicts:

| Area | Why it conflicts |
|------|------------------|
| `helix-view/src/editor.rs` | Config struct, `Editor::new` signature, `open`/`close`/`close_document` |
| `helix-term/src/application.rs` | startup load of hist + trim jobs; file-open CLI positions |
| `helix-term/src/commands/typed.rs` | `:reload-history`; command table grows often in gj1118 |
| `helix-term/src/ui/prompt.rs` | push `:`/`/` lines on validate |
| `helix-term/src/commands.rs` | yank → optional clipboard file write |
| `helix-term/src/main.rs` | `initialize_*_histfile` / clipboard path |
| `helix-loader/src/lib.rs` | `state_dir`, histfile OnceCells + getters |
| Cargo.toml(s) | `bincode`, `serde_regex`, `regex` workspace, `smallvec` serde feature |
| `helix-core/src/selection.rs` | `Serialize`/`Deserialize` on `Range`/`Selection` |
| `helix-view/src/view.rs` | same on `ViewPosition` |

---

## Merge strategy that worked

1. **Remote:** `git remote add intarga https://github.com/intarga/helix.git` (if missing).
2. **Fetch:** `git fetch intarga persistent_state`.
3. **Merge** (prefer over raw patch — 3-way helps):
   ```bash
   git merge intarga/persistent_state --no-ff
   ```
4. **Do not** blindly take “theirs” on big gj1118 files (`editor.rs`,
   `application.rs`, `typed.rs`). Those carry noice cmdline, plugins,
   workspace-trust, notifications, local_search, etc.
5. **Do** take new persistence modules and loader histfile helpers intact.
6. **Cargo.lock:** restore ours if mangled, then let `cargo check -p helix-term`
   add only `bincode` / `serde_regex`. Avoid `cargo generate-lockfile`.

If `intarga/persistent_state` disappears later, recover from this branch’s
merge commit `0f409a7a`, from `vendor/intarga-persistent_state`, or from the
upstream PR patch on #9143.

---

## Conflict playbook (by file)

### Always keep both modules (loader)

```rust
pub mod workspace_trust;
pub mod persistence;
```

Keep **both** `data_dir()` (gj1118/upstream HEAD) and `state_dir()` (persistence).
Histfiles live under `state_dir()`, not cache.

### Cargo / deps

Workspace `Cargo.toml`: keep current workspace deps; add:

```toml
regex = "1"
```

`helix-core`: keep workspace `ropey` / `bitflags` / `foldhash`; use

```toml
smallvec = { version = "1.15", features = ["serde"] }  # version: match tree
regex.workspace = true
```

`helix-view`: keep workspace `toml` / `parking_lot`; add `serde_regex`,
`regex.workspace = true`.

`helix-loader`: add `bincode = "1.3.3"` (version flexible if API stable).

### `Editor::new` signature

Persistence adds a final argument:

```rust
old_file_locs: HashMap<PathBuf, (ViewPosition, Selection)>,
```

**Keep** gj1118’s `workspace_trust: WorkspaceTrust` argument. Order used on
first land:

```text
area, theme_loader, syn_loader, config, handlers, workspace_trust, old_file_locs
```

Every `Editor::new(` call site must pass both (today: `application.rs` only).

### `application.rs` startup sequence

After `ArcSwap` config exists:

1. `persistence_config = config.load().editor.persistence.clone()`
2. If `old_files`: build `old_file_locs` from `persistence::read_file_history()`
3. `Editor::new(..., workspace_trust, old_file_locs)`
4. If enabled: `registers.write(':', command_history)`, same for `/` and `"`
5. Queue trim jobs with `Job::new(...).wait_before_exiting()` (Jobs::add is
   `&self` — `mut jobs` not required)

Imports needed: `persistence`, `Job`, `HashMap`.

### Opinionated fix (not in original PR) — CLI position clobber

**Problem:** After `editor.open()`, gj1118/HEAD always applied CLI file
positions. `parse_file` yields `Position::default()` (0,0) when the user did
not pass `:line`. That **wipes** `old-files` restore from inside `open()`.

**Fix** (keep this on every rebase):

```rust
let apply_cli_pos = match pos.as_slice() {
    [] => false,
    [p] if *p == helix_core::Position::default() => false,
    _ => true,
};
if apply_cli_pos {
    // existing set_selection from CLI coords
}
```

**Tradeoff:** `hx file:1:1` is also (0,0) after saturating_sub and will not
force-apply. Do **not** drop this fix or the integration test fails on
session-2 insert order.

Original PR used `Vec<(PathBuf, Option<Position>)>` so “no line” was `None`.
This tree uses `IndexMap<PathBuf, Vec<Position>>` — keep that API; only skip
implicit default.

### `typed.rs`

- Add `reload_history` with current signature:
  `fn(..., args: Args, event: PromptEvent)` (not old `&[Cow<str>]`).
- Register at end of `TYPABLE_COMMAND_LIST` with
  `CommandCompleter::none()` + `Signature { positionals: (0, Some(0)), .. }`.
- gj1118 often appends commands (notifications, workspace-trust, plugins).
  Re-apply the table entry after those blocks; don’t delete theirs.

### `close` / `close_document`

When saving `FileHistoryEntry`, `doc.path()` is `&Path` — use
`path.to_path_buf()`, not `path.clone()` (type error).

`:wq` → `write_*` then `quit` → `editor.close(view_id)`. File locs must be
recorded in **`close`**, not only `close_document`.

### Tests

- `helix-term/tests/integration.rs`: `mod persistence;`
- `helpers.rs` `with_file`: prefer empty position list when `pos` is `None`:
  ```rust
  self.args.files.insert(path.into(), pos.map(|p| vec![p]).unwrap_or_default());
  ```
  so tests don’t inject a fake (0,0).
- Histfiles use `OnceCell` — `initialize_*_histfile(Some(temp))` only works
  once per process. The persistence test sets temps at start; fine for that
  test binary run.

### Args / parse_file

If a merge tries to change `files` to `Vec<(PathBuf, Option<Position>)>`,
**reject** and keep IndexMap + multi-position behavior unless you’re ready to
update every call site (application open loop, helpers, `:open`).

---

## Problems hit on first land

1. **Huge conflict hunks in `editor.rs` / `typed.rs` / `application.rs`**  
   Auto-merge interleaved multi-hundred-line HEAD-only UI with small PR
   inserts. Resolution: `git checkout --ours` those three, then **surgically**
   re-apply persistence (Config field, PersistenceConfig type, open/close
   hooks, startup load, reload command). Faster than hand-merging noice UI.

2. **`cargo generate-lockfile`**  
   Unlocked the world; pulled `kstring` needing rustc 1.96 while Homebrew was
   1.94. Fix: restore `Cargo.lock` from HEAD, `cargo check` to add only new
   crates.

3. **Integration test failed first run (`\nb\na\n` vs `\na\nb\n`)**  
   Root cause: CLI default position clobber (see above). Not a broken histfile.

4. **`cp target/release/hx ~/.cargo/bin/hx` → exit 137**  
   Prefer `cargo install --path helix-term --locked --force`. After install,
   `hx -V` should print.

5. **Upstream PR is not “almost merged”**  
   Closed by author after ping spam; no maintainer sign-off. Treat as
   vendored feature on this fork, not something that will land upstream soon.

---

## Config snippet

```toml
[editor.persistence]
commands = true
search = true
old-files = true
# clipboard = true
# commands-trim = 100
# search-trim = 100
# old-files-trim = 100
```

Nuke corrupted state: `rm -rf ~/.local/state/helix`

Verify after a rebase:

```bash
cargo test -p helix-term --features integration test_persistence
```

---

## Rebase checklist

- [ ] `git merge master` (or `upstream/master`) into `persistent_state`
- [ ] `helix-loader`: both `workspace_trust` + `persistence`; both `data_dir` + `state_dir`
- [ ] `Editor::new` still has `workspace_trust` **and** `old_file_locs`
- [ ] `application.rs`: load hist before new; register writes; trim jobs; **CLI pos skip**
- [ ] `prompt.rs`: push_reg_history on `:` / `/`
- [ ] `typed.rs`: `:reload-history` still in table
- [ ] `close` / `close_document`: `to_path_buf()` on paths
- [ ] `cargo check -p helix-term`
- [ ] `cargo test -p helix-term --features integration test_persistence`
- [ ] Manual: quit/reopen, history recall in `:` and `/`

---

## Credits

- Original implementation: **intarga** — [helix-editor/helix#9143](https://github.com/helix-editor/helix/pull/9143)
- Base editor fork: **gj1118/helix**
- Port + CLI position fix + these notes: **bellhyve/helix**, 2026-08
