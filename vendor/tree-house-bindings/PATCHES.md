# tree-house-bindings 0.3.2, patched for this fork

The crate as published on crates.io (version 0.3.2, MPL-2.0, with the tree-sitter C library),
used through `[patch.crates-io]` in the workspace's `Cargo.toml`, with the changes below
(marked `PATCH (helix fork)`).

## Text predicates on text spanning two chunks of the rope

`#eq?`, `#any-of?` and the other string predicates compared the text of a node against a
string chunk by chunk, but after the first chunk kept comparing the *start* of the string
instead of the rest: a node whose text spans two chunks of the rope (about one in a thousand
bytes is a chunk boundary) never matched. Highlights, injections (`regexp.MustCompile("...")`
not highlighted as a regex), locals and text objects depending on such predicates failed at
random places of large files, and moved as the file was edited
(`src/query/predicate.rs`, `input_matches_str`). Test:
`predicates_on_text_spanning_rope_chunks` in Helix's `helix-core`.

## A cancelled parse is discarded

A parse stopped by its timeout is resumed by the next call to `ts_parser_parse` unless the
parser is reset, and parsers are shared through a cache: the next document parsed with that
parser (any document) got a tree made partly of the cancelled one. The parser is now reset
when a parse is cancelled (`src/parser.rs`).

## For incremental updates

- `Tree::changed_ranges`: `ts_tree_get_changed_ranges`, the ranges whose structure differs
  between a tree and the tree parsed from it.
- `Query::end_byte_for_pattern` and `Query::is_pattern_rooted`.
- `Node::subtree_id`: what tells a subtree the parser kept from one it made anew, read from
  the `Subtree` that `TSNode.id` points to.
- `InactiveQueryCursor::set_max_start_depth` (`ts_query_cursor_set_max_start_depth`), reset
  when a cursor is taken from the cache.

## For plugins

`Node::has_error` (`ts_node_has_error`): whether a node is or contains a syntax error, for the
`has_error` field of the syntax nodes given to plugins.

## Tree-sitter optimized in debug builds

`build.rs` compiles tree-sitter with optimizations when the build has none (`OPT_LEVEL=0`):
unoptimized, it dominated the time of every debug test touching syntax.

The test `next_match_with_all_captures_disabled` uses Helix's Python grammar
(`runtime/grammars/python.so`) instead of tree-house's test grammars, which are not here.
