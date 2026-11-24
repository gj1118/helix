# tree-house 0.4.0, patched for this fork

The crate as published on crates.io (version 0.4.0, MPL-2.0), used through `[patch.crates-io]`
in the workspace's `Cargo.toml`, with the changes below (marked `PATCH (helix fork)`). Its
tests over tree-house's own test grammars (`src/tests.rs`) are left out: they need grammars
this repository does not have. Helix's tests in `helix-core/src/syntax/incremental_tests.rs`
cover the changes with Helix's grammars and queries.

## Updates cost what an edit changed

Upstream, every update ran the injections query and the locals query over the whole of each
changed layer: on every key typed, both went over the whole file (and over every injected
document an edit touched). In a 10,000-line Rust file that was 40 ms a key.

Now each layer keeps the matches of its queries between updates (`InjectionMatch`,
`LocalCapture`), and a changed layer is queried only where the edit changed it
(`src/incremental.rs` explains why that finds every match that can differ):

- The parse of a changed layer gives the ranges whose structure changed
  (`Tree::changed_ranges` in the bindings); with the edits, a byte wider on both sides, they
  are the *pieces* of the layer to query again.
- A match is kept when its *anchor* (the node of the pattern's root, or the parent of a
  pattern of siblings) is away from every piece, moved by the edits; the others are replaced
  by what queries limited to the pieces find (the match API: the captures API skips captures
  outside the range).
- The anchor of a match is found from any of its captures that is at one depth in its pattern
  (`anchor_captures`), with one walk of the tree for all of a query's matches (`ancestors`).
  A layer with a match whose anchor cannot be told, a layer whose ranges changed, a layer
  given the tree of another one and a layer never queried are queried whole, as before.
- The injections and the local scopes are built from the kept matches as before
  (`resolve` is the old precedence logic of `InjectionsQuery::execute`).
- A layer is parsed again only when its ranges or the text in them changed. Upstream, layers
  of several ranges (comments injected together) were parsed again on every edit anywhere
  before one of their ranges.

### A node made anew over children that stayed

Some edits change a node with many children without changing the children. Typing `let x =`
between two functions of a Rust file makes the file's root an `ERROR` node until the statement
is whole, and typing `{` there swallows the rest of the file. Tree-sitter then reports the
whole node as changed, so every such key queried the whole file again: 40 to 60 ms in a
10,000-line file, on the keys that break or mend the tree.

`incremental::refine` looks at such a node (the *shell*: the smallest node spanning what
tree-sitter reports, when that is 4 KiB or more under a node with 16 children or more) and
tells the children that are what they were:

- the subtrees the parser kept (`Node::subtree_id` in the bindings), and
- the ones it made anew exactly as they were. After an error the parser reads the rest of the
  file again and makes the same nodes: `same_structure` walks the old and the new subtree
  together and compares kinds, ranges and fields, which costs a fraction of querying them. A
  child an edit is in is not compared (the same nodes over another text).

What is queried is then one byte at the shell's start (the matches anchored at the shell and
above it), the children that did change (as ranges, runs of them as one), and the shell
itself with a query that starts patterns no deeper than the children of its children
(`set_max_start_depth(2)`): the matches anchored *at* a child, rooted there or among its
children. The matches anchored *below* a child that stayed are kept. With more than half of
the shell changed, or too many steps to take, the reported ranges are queried as before.

In the same file those keys now take 3 to 4 ms.

`Syntax::update_full` updates the old way, querying every changed layer whole: the tests
compare the two after random edits (layers, their ranges, syntax trees, injections and local
scopes), on small documents and on large ones whose root changes over its children
(`incremental_updates_of_a_changed_root_match_full_updates`, which also checks that such
updates happen). With the predicate fix of the bindings undone, `update_full` gives exactly
what upstream did. `Syntax::structure` prints a syntax tree for those comparisons.
