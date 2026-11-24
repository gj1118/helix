//! Random commands, keys, mouse events and resizes against an editor with most features on:
//! nothing may panic, and no step may hang.
//!
//! The test is ignored by default: it moves the process to a directory of its own (the steps
//! write files), so it cannot run beside the other tests. Run it alone:
//!
//! ```sh
//! FUZZ_SEED=7 FUZZ_STEPS=20000 FUZZ_LOG=/tmp/fuzz.log cargo test --features integration \
//!     --profile integration -p helix-term --test integration -- --ignored fuzz_long
//! ```
//!
//! Every step is written to `FUZZ_LOG` before it runs, so the last line of the log is the step
//! that panicked or hung; `FUZZ_REPLAY=<log>` runs the steps of a log again (cut it down to
//! find the few steps that matter, then write a test for them). The commands that run
//! programs are left out, but the steps do create, rename and delete files: run it as an
//! unprivileged user.

use std::{
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use helix_term::{
    application::Application,
    commands::{typable_command_names, MappableCommand},
    config::{Config, ConfigLoadError},
};
use termina::event::{Event, Modifiers, MouseButton, MouseEvent, MouseEventKind};

use futures_util::FutureExt as _;

use super::helpers;

/// A step taking longer than this is a hang (the editor waits on something on its own thread,
/// or is kept busy without end). `FUZZ_STEP_TIMEOUT` (seconds) sets another limit: a long one
/// leaves the time to attach a debugger.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// The keys commands are bound under (`<F12>`, then two of these).
const SLOTS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The static commands the fuzzer runs: all of them but those that run programs, stop the
/// process or wait for a debugger.
fn static_commands() -> Vec<&'static str> {
    MappableCommand::STATIC_COMMAND_LIST
        .iter()
        .map(|command| command.name())
        .filter(|name| {
            !(name.starts_with("shell_")
                || name.starts_with("dap_")
                || *name == "suspend"
                || *name == "goto_file"
                || name.contains("url"))
        })
        .collect()
}

/// The typable commands the fuzzer runs: not those that run programs, reach the network, quit
/// or change the process (`:cd`).
fn typable_commands() -> Vec<&'static str> {
    const EXCLUDED: &[&str] = &[
        "sh",
        "shell",
        "pipe",
        "output",
        "quit",
        "exit",
        "url",
        "debug",
        "git",
        "remote",
        "cd",
        "run",
        "lsp-workspace-command",
        "workspace-trust",
        "tutor",
        "plugin",
        "blame",
        "review",
    ];
    typable_command_names()
        .filter(|name| {
            !EXCLUDED.iter().any(|excluded| name.contains(excluded))
                && !matches!(*name, "q" | "q!" | "x" | "x!" | "wq" | "wq!" | "cq" | "cq!")
        })
        .collect()
}

/// The slot keys (`<F12>xy`) of the `index`th command.
fn slot(index: usize) -> (char, char) {
    let slots: Vec<char> = SLOTS.chars().collect();
    (slots[index / slots.len()], slots[index % slots.len()])
}

/// A config like a heavy user's, as TOML: most of the fork's features on, every static
/// command bound under `<F12>` in the three modes. As in the other tests: no language
/// servers, no word index shared by the apps of a process, no trust prompts.
fn fuzz_config(commands: &[&str]) -> String {
    let mut toml = String::from(
        r##"
[editor]
idle-timeout = 20
scrolloff = 3
subword-motions = true
rainbow-brackets = true
rounded-corners = true
cursorline = true
cursorcolumn = true
line-number = "relative"
rulers = [40]
mouse = true
color-modes = true
welcome-screen = false
completion-replace = true
end-of-line-diagnostics = "hint"
gutters = { layout = ["diff", "diagnostics", "spacer", "line-numbers", "spacer"] }

[editor.lsp]
enable = false
[editor.word-completion]
enable = false
[editor.workspace-trust]
level = "insecure"
[editor.diff]
connector-graphics = "cells"
[editor.scrollbar]
enable = true
[editor.smooth-scroll]
enable = true
duration = 5
[editor.gradient-borders]
enable = true
thickness = 1
[editor.bufferline]
render-mode = "always"
[editor.soft-wrap]
enable = true
[editor.inline-diagnostics]
cursor-line = "hint"
other-lines = "error"
[editor.indent-guides]
render = true
[editor.whitespace]
render = "all"
[editor.cmdline]
style = "popup"
[editor.notifications]
enable = true
style = "popup"
[editor.quickfix]
enable = true
[editor.explorer]
side = "right"
[editor.statusline]
left = ["mode", "spinner", "file-name", "function-name", "file-modification-indicator", "read-only-indicator"]
center = ["diagnostics", "workspace-diagnostics", "version-control"]
right = ["register", "position", "primary-selection-length", "selections", "language-servers",
    "file-encoding", "file-indent-style", "file-line-ending", "position-percentage", "file-type",
    "total-line-numbers", "spacer"]
"##,
    );
    // the plugins of the repository, with what they ask for
    let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contrib/plugins");
    write!(
        toml,
        r#"
[plugins]
enabled = true
plugin_dirs = [{plugins:?}]
[[plugins.plugins]]
name = "pile"
permissions = ["commands", "keymaps", "edit"]
config = {{ chunk = {{ enable = true }}, line_num = {{ enable = true }} }}
[[plugins.plugins]]
name = "hlslens"
permissions = ["commands"]
[[plugins.plugins]]
name = "plaintasks"
permissions = ["edit", "commands", "keymaps", "windows", "files"]
[[plugins.plugins]]
name = "inline-diagnostic"
config = {{ other_lines = true }}
"#
    )
    .unwrap();
    for mode in ["normal", "select", "insert"] {
        let mut groups: Vec<String> = vec![String::new(); SLOTS.len()];
        for (index, command) in commands.iter().enumerate() {
            let (first, second) = slot(index);
            let group = SLOTS.find(first).unwrap();
            write!(groups[group], "\"{second}\" = \"{command}\", ").unwrap();
        }
        writeln!(toml, "[keys.{mode}.F12]").unwrap();
        for (group, bindings) in groups.iter().enumerate() {
            if !bindings.is_empty() {
                let first = SLOTS.chars().nth(group).unwrap();
                writeln!(toml, "\"{first}\" = {{ {bindings} }}").unwrap();
            }
        }
    }
    toml
}

/// The files the editor starts with: code, prose with wide and combining characters, long
/// lines, tabs and CRLF, an empty file.
fn fixtures(dir: &Path) -> Vec<PathBuf> {
    let mut rust = String::new();
    for n in 0..40 {
        writeln!(
            rust,
            "/// Item {n}.\npub fn item_{n}(values: &[u32]) -> Option<u32> {{\n    let mut total = 0;\n    \
             for value in values {{\n        if *value > {n} {{\n            total += value;\n        }}\n    }}\n    \
             // TODO: check {n}\n    (total > 0).then_some(total)\n}}\n"
        )
        .unwrap();
    }
    let mut prose = String::from("# Notes\n\n- [ ] one\n- [x] two\n\n");
    for n in 0..30 {
        writeln!(
            prose,
            "Line {n}: naïve café 日本語のテキスト 🦀👩‍👩‍👧 e\u{301} \t tab {}",
            "word ".repeat(n * 3)
        )
        .unwrap();
    }
    let crlf = "one\r\n\ttwo\r\n\r\nthree {\r\n  four(\"five\")\r\n}\r\n".repeat(20);
    let long = format!("{}\n{}\n", "x".repeat(5000), "(a [b {c} d] e) ".repeat(300));
    let tasks =
        "Project:\n ☐ one @today\n ✔ two @done (26-01-01 10:00)\n ✘ three\n\nOther:\n ☐ four\n"
            .to_string();
    let files = [
        ("code.rs", rust),
        ("notes.md", prose),
        ("tasks.todo", tasks),
        ("crlf.txt", crlf),
        ("long.json", long),
        ("empty.py", String::new()),
    ];
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    std::fs::write(dir.join("sub/deeper/inner.toml"), "[a]\nb = 1\n").unwrap();
    files
        .into_iter()
        .map(|(name, text)| {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect()
}

fn build(config: &Config, files: &[PathBuf]) -> Application {
    let mut builder = helpers::AppBuilder::new().with_config(config.clone());
    for file in files {
        builder = builder.with_file(file.clone(), None);
    }
    builder.build().expect("the app builds")
}

/// One step, as a line of the log: `K <keys>` (a key macro), `R <width> <height>` (a resize)
/// or `M <kind> <column> <row>` (a mouse event).
fn step(rng: &mut Rng, statics: &[&str], typables: &[&str], files: &[PathBuf]) -> String {
    const RAW: &[&str] = &[
        "<esc>",
        "<ret>",
        "<tab>",
        "<backspace>",
        "<del>",
        "<up>",
        "<down>",
        "<left>",
        "<right>",
        "<home>",
        "<end>",
        "<pageup>",
        "<pagedown>",
        "<space>",
        "<C-w>",
        "<C-d>",
        "<C-u>",
        "<C-o>",
        "<C-i>",
        "<C-a>",
        "<C-x>",
        "<C-s>",
        "<C-c>",
        "<C-n>",
        "<C-p>",
        "<A-.>",
        "<A-o>",
        "<A-i>",
        "<A-n>",
        "<A-p>",
        "<A-s>",
        "<A-minus>",
        "<A-C>",
        "<A-w>",
        "<S-tab>",
        "<lt>",
        "<gt>",
        "a",
        "b",
        "c",
        "d",
        "e",
        "f",
        "g",
        "h",
        "i",
        "j",
        "k",
        "l",
        "m",
        "n",
        "o",
        "p",
        "q",
        "r",
        "s",
        "t",
        "u",
        "v",
        "w",
        "x",
        "y",
        "z",
        "A",
        "B",
        "C",
        "D",
        "E",
        "F",
        "G",
        "H",
        "I",
        "J",
        "K",
        "L",
        "M",
        "N",
        "O",
        "P",
        "Q",
        "R",
        "S",
        "T",
        "U",
        "V",
        "W",
        "X",
        "Y",
        "Z",
        "0",
        "1",
        "2",
        "3",
        "5",
        "9",
        "%",
        "/",
        "?",
        "*",
        "\"",
        "'",
        "`",
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
        ";",
        ",",
        ".",
        "_",
        "&",
        "@",
        "~",
        "=",
        "+",
        ":",
        "<minus>",
        "#",
        "^",
    ];
    const TEXT: &[&str] = &[
        "x",
        "fn ",
        "(",
        ")",
        "{",
        "}",
        "\"",
        "<ret>",
        "<tab>",
        " ",
        "é",
        "日",
        "🦀",
        "<backspace>",
        "<C-w>",
        "0",
        ".",
        ";",
        "[",
        "]",
        "<lt>",
        "<gt>",
        "<del>",
        "<left>",
        "<up>",
    ];
    const ARGS: &[&str] = &[
        "",
        "1",
        "0",
        "-1",
        "999999",
        "true",
        "false",
        "rust",
        "markdown",
        "text",
        "%",
        ".",
        "..",
        "sub",
        "missing/file.txt",
        "--all",
        "--no-format",
        "scrolloff 0",
        "scrolloff 9999",
        "soft-wrap.enable false",
        "soft-wrap.enable true",
        "scrollbar.enable false",
        "scrollbar.enable true",
        "line-number absolute",
        "cursorline false",
        "gutters.layout [\"line-numbers\"]",
        "bufferline never",
        "bufferline always",
        "rainbow-brackets false",
        "indent-guides.render false",
        "text-width 10",
        "rulers [1,2]",
        "default",
        "simpledark",
        "simplelight",
        "a b c",
        "x",
        "2 3",
        "utf-16",
        "crlf",
        "lf",
    ];
    match rng.below(100) {
        0..=44 => {
            let (first, second) = slot(rng.below(statics.len()));
            format!("K <F12>{first}{second}")
        }
        45..=62 => {
            let count = 1 + rng.below(4);
            let keys: String = (0..count).map(|_| *rng.pick(RAW)).collect();
            format!("K {keys}")
        }
        63..=72 => {
            let name = rng.pick(typables);
            let arg = match rng.below(4) {
                0 => files[rng.below(files.len())].display().to_string(),
                _ => rng.pick(ARGS).to_string(),
            };
            format!("K <esc><esc>:{name} {arg}<ret>")
        }
        73..=80 => {
            let count = 1 + rng.below(12);
            let keys: String = (0..count).map(|_| *rng.pick(TEXT)).collect();
            format!("K <esc>i{keys}<esc>")
        }
        81..=86 => {
            let sizes: &[(u16, u16)] = &[
                (120, 40),
                (80, 24),
                (40, 12),
                (20, 6),
                (10, 3),
                (5, 2),
                (2, 1),
                (1, 1),
                (200, 60),
                (3, 30),
                (90, 4),
            ];
            let (width, height) = *rng.pick(sizes);
            format!("R {width} {height}")
        }
        87..=93 => {
            const KINDS: &[&str] = &[
                "down",
                "up",
                "drag",
                "scrollup",
                "scrolldown",
                "moved",
                "right",
            ];
            format!("M {} {} {}", rng.pick(KINDS), rng.below(130), rng.below(45))
        }
        _ => "K <esc>".to_string(),
    }
}

async fn run(app: &mut Application, step: &str) -> anyhow::Result<()> {
    let mut words = step.split(' ');
    match words.next() {
        Some("K") => helpers::send_keys(app, step[2..].trim_end_matches('\n')).await?,
        Some("R") => {
            let width = words.next().unwrap_or("80").parse()?;
            let height = words.next().unwrap_or("24").parse()?;
            app.resize_terminal(width, height).await;
            helpers::send_events(app, Vec::new()).await?;
        }
        Some("M") => {
            let kind = match words.next() {
                Some("down") => MouseEventKind::Down(MouseButton::Left),
                Some("up") => MouseEventKind::Up(MouseButton::Left),
                Some("drag") => MouseEventKind::Drag(MouseButton::Left),
                Some("right") => MouseEventKind::Down(MouseButton::Right),
                Some("scrollup") => MouseEventKind::ScrollUp,
                Some("scrolldown") => MouseEventKind::ScrollDown,
                _ => MouseEventKind::Moved,
            };
            let column = words.next().unwrap_or("0").parse()?;
            let row = words.next().unwrap_or("0").parse()?;
            let event = Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: Modifiers::empty(),
            });
            helpers::send_events(app, vec![event]).await?;
        }
        _ => {}
    }
    Ok(())
}

/// How a run went.
struct Outcome {
    /// The steps run.
    steps: usize,
    /// The steps that took long, slowest first.
    slowest: Vec<(Duration, String)>,
    /// The step that panicked or hung, which ended the run.
    failure: Option<String>,
}

/// Runs `steps` random steps from `seed` (or the steps of `replay`), logging each to `log`
/// before it runs, until one panics or hangs, or the editor quits.
async fn fuzz(
    seed: u64,
    steps: usize,
    replay: Option<Vec<String>>,
    mut log: Option<std::fs::File>,
) -> anyhow::Result<Outcome> {
    let dir = tempfile::tempdir()?;
    let dir_path = helix_stdx::path::canonicalize(dir.path());
    // relative paths (`:write x`, the explorer's root) stay in the directory
    helix_stdx::env::set_current_working_dir(&dir_path)?;
    let files = fixtures(&dir_path);
    let statics = static_commands();
    let typables = typable_commands();
    assert!(
        statics.len() <= SLOTS.len() * SLOTS.len(),
        "more slots needed"
    );
    // The config is the config file too (`:config-reload` reads it again), and the state
    // files are in the directory.
    let toml = fuzz_config(&statics);
    let config = Config::load(Ok(&toml), Err(ConfigLoadError::default()))
        .map_err(|err| anyhow::anyhow!("the fuzz config does not load: {err}"))?;
    let state = dir_path.join(".state");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("config.toml"), &toml)?;
    helix_loader::initialize_config_file(Some(state.join("config.toml")));
    helix_loader::initialize_log_file(Some(state.join("helix.log")));
    helix_loader::initialize_command_histfile(Some(state.join("command_history")));
    helix_loader::initialize_search_histfile(Some(state.join("search_history")));
    helix_loader::initialize_file_histfile(Some(state.join("file_history")));
    helix_loader::initialize_clipboard_file(Some(state.join("clipboard")));
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut app = build(&config, &files);
    let mut slowest: Vec<(Duration, String)> = Vec::new();

    let count = replay.as_ref().map_or(steps, Vec::len);
    let mut ran = count;
    let step_timeout = std::env::var("FUZZ_STEP_TIMEOUT")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(STEP_TIMEOUT, Duration::from_secs);
    for index in 0..count {
        let step = match &replay {
            Some(lines) => lines[index].clone(),
            None => step(&mut rng, &statics, &typables, &files),
        };
        if step.starts_with('#') || step.is_empty() {
            continue;
        }
        let named = match step.strip_prefix("K <F12>") {
            Some(keys) if keys.chars().count() == 2 => {
                let mut keys = keys.chars();
                let first = SLOTS.find(keys.next().unwrap()).unwrap_or(0);
                let second = SLOTS.find(keys.next().unwrap()).unwrap_or(0);
                statics
                    .get(first * SLOTS.len() + second)
                    .map(|name| format!("# {name}"))
            }
            _ => None,
        };
        // closing the only window quits: the run goes on instead
        if named.as_deref() == Some("# wclose") && app.editor.tree.views().count() < 2 {
            continue;
        }
        if let Some(log) = &mut log {
            if let Some(named) = &named {
                writeln!(log, "{named}")?;
            }
            writeln!(log, "{step}")?;
            log.flush()?;
        }
        let started = Instant::now();
        let outcome =
            std::panic::AssertUnwindSafe(tokio::time::timeout(step_timeout, run(&mut app, &step)))
                .catch_unwind()
                .await;
        let failure = match outcome {
            Ok(Ok(result)) => {
                result?;
                None
            }
            Ok(Err(_)) => Some("hung"),
            Err(_) => Some("panicked"),
        };
        if let Some(failure) = failure {
            // (the panic's message and backtrace are on stderr) What is left of the editor is
            // not run again, nor dropped.
            std::mem::forget(app);
            return Ok(Outcome {
                steps: index,
                slowest,
                failure: Some(format!(
                    "step {index} {failure}: {step} {}",
                    named.unwrap_or_default()
                )),
            });
        }
        let took = started.elapsed();
        if took > Duration::from_millis(150) {
            slowest.push((took, format!("{step} {}", named.unwrap_or_default())));
        }
        // The editor quit (the last window closed): the run ends, as the handlers and the job
        // queue of a process belong to its first editor.
        if app.editor.should_close() || app.editor.tree.views().next().is_none() {
            ran = index + 1;
            break;
        }
    }
    let _ = app.close().await;
    slowest.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Outcome {
        steps: ran,
        slowest,
        failure: None,
    })
}

/// The long run: `FUZZ_SEED`, `FUZZ_STEPS`, `FUZZ_LOG` and `FUZZ_REPLAY` (see the module).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "long: run it with FUZZ_SEED and FUZZ_STEPS"]
async fn fuzz_long() -> anyhow::Result<()> {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let seed = number("FUZZ_SEED", 1);
    let steps = number("FUZZ_STEPS", 5000) as usize;
    let log = match std::env::var("FUZZ_LOG") {
        Ok(path) => Some(std::fs::File::create(path)?),
        Err(_) => None,
    };
    let replay = match std::env::var("FUZZ_REPLAY") {
        Ok(path) => Some(
            std::fs::read_to_string(path)?
                .lines()
                .map(str::to_string)
                .collect(),
        ),
        Err(_) => None,
    };
    let outcome = fuzz(seed, steps, replay, log).await?;
    println!("ran {} steps", outcome.steps);
    for (took, step) in outcome.slowest.iter().take(25) {
        println!("slow: {took:?} {step}");
    }
    assert_eq!(outcome.failure, None);
    Ok(())
}
