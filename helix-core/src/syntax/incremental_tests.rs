//! Syntax trees updated by re-reading only what an edit changed are the ones updated by re-reading
//! every changed layer whole: random edits on documents with many injections and locals, compared
//! after every edit (layers, their ranges and trees, injections and local scopes).

use std::sync::LazyLock;

use super::{generate_edits, Loader, Syntax, PARSE_TIMEOUT};
use crate::{Rope, Transaction};

static LOADER: LazyLock<Loader> = LazyLock::new(crate::config::default_lang_loader);

/// xorshift64*: reproducible edits without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Text typed or pasted: syntax that opens and closes blocks, strings, comments and injections.
const TOKENS: &[&str] = &[
    "a",
    "x",
    "foo",
    " ",
    "  ",
    "\n",
    "\n    ",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "<",
    ">",
    "\"",
    "'",
    "`",
    ",",
    ";",
    ":",
    "=",
    ".",
    "!",
    "#",
    "$",
    "\\",
    "//",
    "/*",
    "*/",
    "// TODO: fix\n",
    "/// doc `x`\n",
    "<!--",
    "-->",
    "```",
    "\n```rust\n",
    "\n```\n",
    "<script>",
    "</script>",
    "<style>",
    "</style>",
    "fn x() {}",
    "println!(\"{} {x}\", y);",
    "format!(\"a\")",
    "Regex::new(r\"a+\")",
    "regexp.MustCompile(`x+`)",
    "fmt.Printf(\"%d\", 1)",
    "func g() {}\n",
    "/* sql */ \"select 1\"",
    "html`<b>`",
    "# Heading\n",
    "*em*",
    "[l](u)",
    "<div onclick=\"f()\">",
];

/// One edit: chars `from..to` replaced by `text`.
type Edit = (usize, usize, String);

/// The next edits, each applied and checked on its own: typing a token types it a char at a time.
fn random_edits(rng: &mut Rng, text: &Rope, original_len: usize) -> Vec<Vec<Edit>> {
    let len = text.len_chars();
    let pos = rng.below(len + 1);
    let token = TOKENS[rng.below(TOKENS.len())];
    let deletion = |rng: &mut Rng, pos: usize| (pos, (pos + 1 + rng.below(30)).min(len));
    // Grown documents shrink again, so the edits stay as fast as the document is small.
    let roll = if len > 2 * original_len {
        40 + rng.below(30)
    } else {
        rng.below(100)
    };
    match roll {
        0..40 => token
            .chars()
            .enumerate()
            .map(|(i, c)| vec![(pos + i, pos + i, c.to_string())])
            .collect(),
        40..55 => vec![vec![(pos, pos, token.to_string())]],
        55..75 => {
            let (from, to) = deletion(rng, pos);
            vec![vec![(from, to, String::new())]]
        }
        75..85 => {
            let (from, to) = deletion(rng, pos);
            vec![vec![(from, to, token.to_string())]]
        }
        85..95 => {
            // Several cursors: edits far apart in one change.
            let mut positions: Vec<usize> =
                (0..2 + rng.below(3)).map(|_| rng.below(len + 1)).collect();
            positions.sort_unstable();
            positions.dedup();
            let mut edits = Vec::new();
            let mut end = 0;
            for pos in positions {
                if pos < end {
                    continue;
                }
                let to = if rng.below(2) == 0 {
                    pos
                } else {
                    (pos + 1 + rng.below(5)).min(len)
                };
                let insert = if rng.below(3) == 0 {
                    String::new()
                } else {
                    TOKENS[rng.below(TOKENS.len())].to_string()
                };
                edits.push((pos, to, insert));
                end = to + 1;
            }
            vec![edits]
        }
        _ => {
            // A line moved elsewhere in one change.
            let line = rng.below(text.len_lines());
            let (from, to) = (
                text.line_to_char(line),
                text.line_to_char((line + 1).min(text.len_lines())),
            );
            let moved: String = text.slice(from..to).into();
            let target = rng.below(len + 1);
            if (from..=to).contains(&target) {
                return vec![];
            }
            let mut edits = vec![(from, to, String::new()), (target, target, moved)];
            edits.sort_by_key(|edit| edit.0);
            vec![edits]
        }
    }
}

fn first_lines(text: &str, count: usize) -> String {
    text.lines()
        .take(count)
        .flat_map(|line| [line, "\n"])
        .collect()
}

const GO: &str = r#"// Package demo shows injections.
package demo

import (
	"fmt"
	"regexp"
)

//go:generate stringer -type=Kind

// Kind is a kind.
// It has two lines of docs.
type Kind int

// Matcher matches things.
var Matcher = regexp.MustCompile(`^[a-z]+(\d{2,})?$`)

// Greet says hello.
func Greet(name string) string {
	// a comment inside
	s := fmt.Sprintf("hello %s, %d", name, 3)
	fmt.Printf("%v\n", s)
	return s
}

/* block comment
   spanning lines */
func other() {
	re := regexp.MustCompile("a|b")
	_ = re
}
"#;

const RUST: &str = r#"//! Module docs with `code` and [links](https://example.com).

/// Doc comment with a list:
/// - one
/// - two
fn demo(a: u32, b: u32) -> u32 {
    let re = Regex::new(r"^a+(b|c)*$").unwrap();
    println!("value {} and {:?}", a, re);
    let q = sqlx::query!("SELECT * FROM users WHERE id = $1", a);
    let s = format!("{b:>5}");
    assert_eq!(a, b, "message {}", s);
    // TODO: comment tag
    let closure = |x: u32| { let y = x + a; y * 2 };
    closure(b)
}

macro_rules! m { ($x:expr) => { $x + 1 }; }
"#;

const MARKDOWN: &str = r#"# Title

Some *inline* `code` and [a link](http://x).

```rust
fn main() { println!("hi {}", 1); }
```

```html
<div><script>let a = 1;</script><style>a { color: red; }</style></div>
```

- a list
  - nested `code`

```js
const s = /* sql */ "select 1";
```
"#;

const HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<style>
body { margin: 0; }
.a > .b { color: red; }
</style>
<script>
// comment
function f(x) { return `t ${x}`; }
</script>
<script type="module">import x from "y";</script>
</head>
<body onload="f(1)">
<!-- comment -->
<div style="color: blue">text</div>
</body>
</html>
"#;

const JAVASCRIPT: &str = r#"/** JSDoc @param {number} x */
function f(x) {
  const q = /* sql */ "SELECT * FROM t";
  const h = html`<div>${x}</div>`;
  const style = css`a { color: red; }`;
  const re = /ab+c/gi;
  // TODO: something
  return q + h + style + re;
}
"#;

/// Documents of the languages with the most injections and locals, nested several levels deep.
fn corpus() -> Vec<(&'static str, String)> {
    let comments: String = (0..60)
        .map(|i| format!("// TODO: item {i}\nfn f{i}() {{}}\n"))
        .collect();
    vec![
        ("go", GO.repeat(3)),
        (
            "rust",
            format!(
                "{RUST}{}",
                first_lines(include_str!("../../../helix-term/src/ui/editor.rs"), 220)
            ),
        ),
        ("rust", comments),
        (
            "markdown",
            format!(
                "{MARKDOWN}{}",
                first_lines(include_str!("../../../book/src/editor.md"), 120)
            ),
        ),
        ("html", HTML.repeat(2)),
        ("javascript", JAVASCRIPT.repeat(3)),
        (
            "python",
            first_lines(
                include_str!("../../../helix-term/tests/test/fixtures/mock_dap.py"),
                150,
            ),
        ),
    ]
}

/// Applies `steps` random edits to `source`, updating one syntax tree with `update` and one with
/// `update_full`, and compares them after every edit.
fn check(language: &str, source: &str, steps: usize, seed: u64) {
    let lang = LOADER.language_for_name(language).unwrap();
    let mut text = Rope::from(source);
    let mut incremental = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
    let mut full = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
    let mut rng = Rng(seed);
    let mut step = 0;

    while step < steps {
        for edits in random_edits(&mut rng, &text, source.len()) {
            step += 1;
            let changes = edits
                .iter()
                .map(|(from, to, insert)| (*from, *to, Some(insert.as_str().into())));
            let transaction = Transaction::change(&text, changes);
            let old = text.clone();
            transaction.apply(&mut text);
            let input_edits = generate_edits(old.slice(..), transaction.changes());
            let updated =
                incremental
                    .inner
                    .update(text.slice(..), PARSE_TIMEOUT, &input_edits, &*LOADER);
            let updated_full =
                full.inner
                    .update_full(text.slice(..), PARSE_TIMEOUT, &input_edits, &*LOADER);
            assert_eq!(updated, updated_full, "{language} step {step}: {edits:?}");
            if updated.is_err() {
                incremental = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
                full = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
                continue;
            }
            let (got, expected) = (incremental.inner.structure(), full.inner.structure());
            if got != expected {
                let dir = std::env::temp_dir();
                std::fs::write(dir.join("syntax-incremental.txt"), &got).unwrap();
                std::fs::write(dir.join("syntax-full.txt"), &expected).unwrap();
                std::fs::write(dir.join("syntax-text.txt"), text.to_string()).unwrap();
                if let Ok(fresh) = Syntax::new(text.slice(..), lang, &LOADER) {
                    std::fs::write(dir.join("syntax-fresh.txt"), fresh.inner.structure()).unwrap();
                }
                let line = got.lines().zip(expected.lines()).position(|(a, b)| a != b);
                panic!(
                    "{language} (seed {seed}) differs after step {step}, edits {edits:?}, from line {line:?} of {}",
                    dir.join("syntax-incremental.txt").display()
                );
            }
        }
    }
}

#[test]
fn incremental_updates_match_full_updates() {
    let steps = std::env::var("SYNTAX_STEPS").map_or(150, |steps| steps.parse().unwrap());
    let seeds = std::env::var("SYNTAX_SEEDS").map_or(1, |seeds| seeds.parse().unwrap());
    for (language, source) in corpus() {
        for seed in 1..=seeds {
            check(language, &source, steps, seed * 7919 + source.len() as u64);
        }
    }
}

/// Large documents with many top-level items: an edit that breaks or mends the tree at the top
/// level changes the root (a Rust file's root is an `ERROR` while `let x =` is typed between
/// two functions) over children that stay what they were. Such updates query the root's
/// children that changed instead of the whole file (tree-house's `incremental::refine`), and
/// give what a full update gives.
#[test]
fn incremental_updates_of_a_changed_root_match_full_updates() {
    use std::sync::atomic::Ordering;
    let steps = std::env::var("SYNTAX_STEPS").map_or(120, |steps| steps.parse().unwrap());
    let seeds = std::env::var("SYNTAX_SEEDS").map_or(1, |seeds| seeds.parse().unwrap());
    let comments: String = (0..400)
        .map(|i| {
            format!("// TODO: item {i} with a [link](x)\nfn f{i}(a: u32) -> u32 {{ a + {i} }}\n\n")
        })
        .collect();
    let documents = [
        (
            "rust",
            first_lines(
                include_str!("../../../helix-term/src/ui/statusline.rs"),
                700,
            ),
        ),
        ("rust", comments),
        ("go", GO.repeat(30)),
        ("javascript", JAVASCRIPT.repeat(25)),
        (
            "python",
            include_str!("../../../helix-term/tests/test/fixtures/mock_dap.py").to_string(),
        ),
        ("html", HTML.repeat(12)),
    ];
    let shells = &tree_house::incremental::SHELL_UPDATES;
    let before = shells.load(Ordering::Relaxed);
    for (language, source) in documents {
        for seed in 1..=seeds {
            check(
                language,
                &source,
                steps,
                seed * 104_729 + source.len() as u64,
            );
        }
    }
    let refined: usize = shells.load(Ordering::Relaxed) - before;
    assert!(
        refined > 20,
        "only {refined} updates went by a changed node's children"
    );
}

/// The time an update takes in large documents, re-reading what changed and re-reading every
/// changed layer whole: `cargo test --release -p helix-core --lib update_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn update_speed() {
    let go = GO.repeat(150);
    let todo: String = (0..3000)
        .map(|i| {
            format!(
                "// TODO: item {i} with a [link](x)\nfn f{i}() {{ println!(\"{{}}\", {i}); }}\n"
            )
        })
        .collect();
    let documents = [
        (
            "rust",
            "commands.rs",
            include_str!("../../../helix-term/src/commands.rs").to_string(),
        ),
        (
            "rust",
            "ui/editor.rs",
            include_str!("../../../helix-term/src/ui/editor.rs").to_string(),
        ),
        ("rust", "comments", todo),
        ("go", "go", go),
        (
            "markdown",
            "CHANGELOG.md",
            include_str!("../../../CHANGELOG.md").to_string(),
        ),
        ("html", "html", HTML.repeat(200)),
    ];
    for (language, name, source) in documents {
        let lang = LOADER.language_for_name(language).unwrap();
        let mut times = [Vec::new(), Vec::new()];
        for (full, times) in times.iter_mut().enumerate() {
            let mut text = Rope::from(source.as_str());
            let mut syntax = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
            let mut rng = Rng(42);
            for _ in 0..30 {
                // Type a word somewhere, a char at a time, like typing.
                let mut pos = rng.below(text.len_chars());
                for c in "item ".chars() {
                    let transaction = Transaction::change(
                        &text,
                        std::iter::once((pos, pos, Some(c.to_string().into()))),
                    );
                    let old = text.clone();
                    transaction.apply(&mut text);
                    let edits = generate_edits(old.slice(..), transaction.changes());
                    let start = std::time::Instant::now();
                    if full == 1 {
                        syntax
                            .inner
                            .update_full(text.slice(..), PARSE_TIMEOUT, &edits, &*LOADER)
                            .unwrap();
                    } else {
                        syntax
                            .inner
                            .update(text.slice(..), PARSE_TIMEOUT, &edits, &*LOADER)
                            .unwrap();
                    }
                    times.push(start.elapsed());
                    pos += 1;
                }
            }
            times.sort();
        }
        let median = |times: &[std::time::Duration]| times[times.len() / 2];
        let p90 = |times: &[std::time::Duration]| times[times.len() * 9 / 10];
        println!(
            "{name:14} {:6} lines  incremental median {:>9.2?} p90 {:>9.2?}   full median {:>9.2?} p90 {:>9.2?}",
            source.lines().count(),
            median(&times[0]),
            p90(&times[0]),
            median(&times[1]),
            p90(&times[1]),
        );
    }
}

/// The time of each update while a statement is typed in a function of a large file, a char at
/// a time (the tree is broken, then whole again, as it is typed):
/// `cargo test --release -p helix-core --lib typing_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn typing_speed() {
    let source = match std::env::var("TYPING_FILE") {
        Ok(path) => std::fs::read_to_string(path).unwrap(),
        Err(_) => include_str!("../../../helix-term/src/commands.rs").to_string(),
    };
    let source = source.as_str();
    let lang = LOADER.language_for_name("rust").unwrap();
    // (1-based, like `:goto`: the text is typed at the end of that line)
    let line = std::env::var("TYPING_LINE").map_or(7000, |line| line.parse().unwrap());
    let typed = std::env::var("TYPING_TEXT").unwrap_or("\n    let x = 1;\n    foo(a, b);".into());
    for full in [false, true] {
        let mut text = Rope::from(source);
        let mut syntax = Syntax::new(text.slice(..), lang, &LOADER).unwrap();
        let mut pos = text.line_to_char(line) - 1;
        let mut report = String::new();
        for c in typed.chars() {
            let transaction = Transaction::change(
                &text,
                std::iter::once((pos, pos, Some(c.to_string().into()))),
            );
            let old = text.clone();
            transaction.apply(&mut text);
            let edits = generate_edits(old.slice(..), transaction.changes());
            let start = std::time::Instant::now();
            if full {
                syntax
                    .inner
                    .update_full(text.slice(..), PARSE_TIMEOUT, &edits, &*LOADER)
                    .unwrap();
            } else {
                syntax
                    .inner
                    .update(text.slice(..), PARSE_TIMEOUT, &edits, &*LOADER)
                    .unwrap();
            }
            report.push_str(&format!(
                "{c:?}:{:.1} ",
                start.elapsed().as_secs_f64() * 1000.0
            ));
            pos += 1;
        }
        println!(
            "{} (ms): {report}",
            if full { "full       " } else { "incremental" }
        );
    }
}

/// Text predicates (`#eq?`, `#any-of?`) see the text of a node spanning two chunks of the rope:
/// every `regexp.MustCompile` call injects regex, wherever the rope's chunks end.
#[test]
fn predicates_on_text_spanning_rope_chunks() {
    let calls = 300;
    let source: String = std::iter::once("package demo\n\nimport \"regexp\"\n\n".to_string())
        .chain((0..calls).map(|i| format!("var r{i} = regexp.MustCompile(\"a{i}+\")\n")))
        .collect();
    let text = Rope::from(source.as_str());
    assert!(text.chunks().count() > 10, "the document spans many chunks");
    let go = LOADER.language_for_name("go").unwrap();
    let regex = LOADER.language_for_name("regex").unwrap();
    let syntax = Syntax::new(text.slice(..), go, &LOADER).unwrap();
    let structure = syntax.inner.structure();
    let injected = structure
        .matches(&format!("language {} ranges", regex.idx()))
        .count();
    assert_eq!(injected, calls);
}
