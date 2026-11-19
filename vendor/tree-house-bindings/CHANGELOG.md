# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- ## [Unreleased] -->

## [v0.3.2] - 2026-06-01

### Fixed

* Fixed NULL handling when attempting to use `QueryCursor::next_match` / `QueryCursor::next_matched_node` on a query where all captures had been disabled with `Query::disable_capture` ([10f61ff](https://github.com/helix-editor/tree-house/commit/10f61ff))

## [v0.3.1] - 2026-05-31

### Fixed

* Fixed `ParseState` and `ParseOptions` not being re-exported from the crate root, making `parse_with_options` unusable ([10f61ff](https://github.com/helix-editor/tree-house/commit/10f61ff))
* Fixed a panic in `Query::start_byte_for_pattern` for queries that have patterns but no text predicates: the bounds assertion was incorrectly checking against the number of text predicates rather than the number of patterns ([4501ded](https://github.com/helix-editor/tree-house/commit/4501ded))

## [v0.3.0] - 2026-05-30

### Added

* Added `Parser::parse_with_options` for parsing with a progress/cancellation callback ([c8e4308](https://github.com/helix-editor/tree-house/commit/c8e4308))
* Added `Parser::parse_with_timeout` as a convenience wrapper around `parse_with_options` ([c8e4308](https://github.com/helix-editor/tree-house/commit/c8e4308))
* Added `ParseState` (passed to the progress callback) and `ParseOptions` types ([c8e4308](https://github.com/helix-editor/tree-house/commit/c8e4308))

### Removed

* Removed `Parser::set_timeout`. Use `Parser::parse_with_timeout` instead ([c8e4308](https://github.com/helix-editor/tree-house/commit/c8e4308))

### Updated

* Updated the tree-sitter C library to v0.26.9 ([47d87a2](https://github.com/helix-editor/tree-house/commit/47d87a2))

## [v0.2.4] - 2026-05-30

### Added

* Added `Range::new` with debug assertions that the end comes after the start positions ([4d81f5ae](https://github.com/helix-editor/tree-house/commit/4d81f5ae4a06995240244ee8930a42161d728331))

## [v0.2.3] - 2026-02-18

### Updated

* Included inner `libloading::Error` in `grammar::Error` message ([#28](https://github.com/helix-editor/tree-house/pull/28))
* Separated lifetimes of the tree cursor and tree in `TreeCursor::into_iter` ([5544c6c2](https://github.com/helix-editor/tree-house/commit/5544c6c2fbc66b3a26adbdf9c8f3b46770b2a362))
* Added Redox in `endian.h` in the C library ([#32](https://github.com/helix-editor/tree-house/pull/32))

## [v0.2.2] - 2025-08-31

### Added

* Added an optional feature to load `Grammar`s from [`LanguageFn`](https://docs.rs/tree-sitter-language/0.1.5/tree_sitter_language/struct.LanguageFn.html) from the [`tree-sitter-language` crate](https://crates.io/crates/tree-sitter-language) ([#24](https://github.com/helix-editor/tree-house/pull/24))

### Updated

* Updated the tree-sitter C library to v0.25.8 ([da576cf74e04](https://github.com/helix-editor/tree-house/commit/da576cf74e04))

### Fixed

* Fixed message for the impossible pattern error message in query analysis failures. ([9fe0be04c306](https://github.com/helix-editor/tree-house/commit/9fe0be04c306))

## [v0.2.1] - 2025-07-12

### Added

* Added `Node::is_extra`

### Updated

* Updated the tree-sitter C library to v0.25.7

## [v0.2.0] - 2025-06-06

### Added

* Added `TreeCursor::reset`
* Added an iterator for recursively walking over the nodes in a `TreeCursor`: `TreeRecursiveWalker`

### Updated

* Updated the tree-sitter C library to v0.25.6

## [v0.1.1] - 2025-05-14

### Fixed

* Patched `endian.h` to include IllumOS

## [v0.1.0] - 2025-05-13

### Added

* Initial publish
