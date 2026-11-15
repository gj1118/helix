# termina 0.3.3, patched for this fork

The crate as published on crates.io (version 0.3.3, MIT/MPL-2.0), with one change to its input
parser (`src/parse.rs`, marked `PATCH (helix fork)`), used through `[patch.crates-io]` in the
workspace's `Cargo.toml`. The examples are left out.

## Keys after an `Esc ]` or `Esc P` that is not a reply

`Esc ]` (OSC) and `Esc P` (DCS) start strings that end only with a terminator (`BEL`, or
`Esc \`). termina read every byte after them into the string until that terminator: when Esc
and the next key were read together (the editor busy for a moment, a slow SSH connection, a
fast typist: `Esc` then `]d`, `Esc` then `P`), or Alt-] / Alt-P was pressed, every key after
them was held, and the editor seemed frozen until a terminator the keyboard never sends.

Now, as soon as the bytes can no longer be a reply a terminal sends to an application (an OSC
is `Esc ] <number> ; <text>`, a DCS starts with a number or one of `> = + ! $`, and their text
has no control characters before its terminator), they are keys again: Alt-] or Alt-P, then
the bytes after it, parsed again (`not_a_reply`, and the rescan in `Parser::process_bytes`).
Replies parse as before. Tests: `keys_after_esc_bracket_are_keys`,
`keys_after_esc_p_are_keys`, `replies_still_parse`.
