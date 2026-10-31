//! File tree panel ("explorer"):
//! lazy-loaded directory tree model, configuration, and runtime state.
//!
//! Disk reads happen when a directory is first expanded (`Tree::expand_at`)
//! and in `Tree::refresh`; rendering only touches already materialized
//! state, so opening the explorer is O(1) in project size and every frame
//! renders straight out of cached rows.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use nucleo::pattern::{CaseMatching, Normalization, Pattern as FuzzyPattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};

use crate::graphics::Rect;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Where the explorer panel lives. Pinned panels `left`/`right` reserve a
/// column from the editor viewports; `floating` draws a window over the
/// editor instead.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExplorerSide {
    #[default]
    Left,
    Right,
    Floating,
}

/// Configuration of the explorer panel (`[editor.explorer]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExplorerConfig {
    /// Pinned `left`/`right` or `floating`. Defaults to `left`.
    pub side: ExplorerSide,
    /// Width of the pinned panel in columns. Defaults to `30`.
    pub width: u16,
    /// Width of the floating window in columns. Defaults to `40`.
    pub floating_width: u16,
    /// Height of the floating window in lines. Defaults to `20`.
    pub floating_height: u16,
    /// Column of the floating window's left edge. Centered when unset.
    pub floating_x: Option<u16>,
    /// Row of the floating window's top edge. Centered when unset.
    pub floating_y: Option<u16>,
    /// Draw file type icons (honoring the global `[editor.icons]` settings).
    /// Defaults to `true`.
    pub icons: bool,
    /// Select the current buffer's file whenever the panel is opened.
    /// Defaults to `false`.
    pub auto_reveal: bool,
    /// Show dotfiles. Defaults to `false`.
    pub show_hidden: bool,
    /// Sort directories before files. Defaults to `true`.
    pub directories_first: bool,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            side: ExplorerSide::Left,
            width: 30,
            floating_width: 40,
            floating_height: 20,
            floating_x: None,
            floating_y: None,
            icons: true,
            auto_reveal: false,
            show_hidden: false,
            directories_first: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// An entry in the explorer clipboard. `cut` items are moved on paste,
/// otherwise copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerClipboard {
    pub path: PathBuf,
    pub cut: bool,
}

/// Runtime explorer state owned by [`crate::Editor`]. The `Option` fields are
/// runtime overrides; they default to the config values and persist until
/// helix exits.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExplorerState {
    /// Whether the panel is currently visible.
    pub visible: bool,
    /// Whether the panel has keyboard focus (tree navigation and file
    /// mutations only respond to keys while this is set).
    pub focused: bool,
    /// Runtime side override (`:explorer-left` / `:explorer-right`). Only ever
    /// holds a pinned side; the floating window is tracked by `floating`.
    pub side: Option<ExplorerSide>,
    /// Whether the panel is currently shown as a floating window. This is a
    /// transient presentation of the one tree, owned by the floating toggle
    /// (`:explorer-float-toggle`); closing the window clears it, so the pinned
    /// panel's toggle never reopens a float.
    pub floating: bool,
    /// Position and size of the floating window as last set with the mouse.
    /// Takes precedence over `floating-*` in the config and is kept across
    /// sessions (see [`load_floating_geometry`]).
    pub floating_geometry: Option<FloatGeometry>,
    /// Runtime width override (`:explorer-width`).
    pub width: Option<u16>,
    /// Runtime show-hidden override (`h`/'H' inside the panel).
    pub show_hidden: Option<bool>,
}

impl ExplorerState {
    /// Effective side of the panel.
    pub fn side(&self, config: &ExplorerConfig) -> ExplorerSide {
        if self.floating {
            ExplorerSide::Floating
        } else {
            self.side.unwrap_or(config.side)
        }
    }

    /// Hides the panel (pinned or floating) and drops its keyboard focus.
    pub fn hide(&mut self) {
        self.visible = false;
        self.focused = false;
        self.floating = false;
    }

    /// Effective width of the pinned panel.
    pub fn width(&self, config: &ExplorerConfig) -> u16 {
        self.width.unwrap_or(config.width)
    }

    /// Effective width of the floating window.
    pub fn floating_width(&self, config: &ExplorerConfig) -> u16 {
        self.floating_geometry
            .map_or(config.floating_width, |g| g.width)
    }

    /// Effective height of the floating window.
    pub fn floating_height(&self, config: &ExplorerConfig) -> u16 {
        self.floating_geometry
            .map_or(config.floating_height, |g| g.height)
    }

    /// The floating window's frame (border included) inside `area`: the mouse
    /// set geometry if there is one, else the `floating-*` config, centered
    /// where no position is given. Always fits inside `area`.
    pub fn floating_frame(&self, config: &ExplorerConfig, area: Rect) -> Rect {
        let width = self
            .floating_width(config)
            .min(area.width.saturating_sub(2));
        let height = self
            .floating_height(config)
            .min(area.height.saturating_sub(2));
        let (x, y) = match self.floating_geometry {
            Some(g) => (Some(g.x), Some(g.y)),
            None => (config.floating_x, config.floating_y),
        };
        let x = x.unwrap_or(area.width.saturating_sub(width) / 2);
        let y = y.unwrap_or(area.height.saturating_sub(height) / 2);
        Rect::new(
            area.x + x.min(area.width.saturating_sub(width)),
            area.y + y.min(area.height.saturating_sub(height)),
            width,
            height,
        )
    }

    /// Whether hidden (dotfile) entries are shown.
    pub fn show_hidden(&self, config: &ExplorerConfig) -> bool {
        self.show_hidden.unwrap_or(config.show_hidden)
    }
}

/// Parameters resolved from config/state that affect tree reads and
/// rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeParams {
    pub show_hidden: bool,
    pub directories_first: bool,
    pub scan: ScanOptions,
}

/// Which ignore files the filter's recursive scan honors. Mirrors the file
/// picker's settings (`[editor.file-picker]`), so a filtered tree hides what
/// the picker hides: `.gitignore`, `.ignore`, `.helix/ignore`, the global
/// git excludes and `.git/info/exclude`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            parents: true,
            ignore: true,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
        }
    }
}

impl From<&crate::editor::FilePickerConfig> for ScanOptions {
    fn from(config: &crate::editor::FilePickerConfig) -> Self {
        Self {
            parents: config.parents,
            ignore: config.ignore,
            git_ignore: config.git_ignore,
            git_global: config.git_global,
            git_exclude: config.git_exclude,
        }
    }
}

impl TreeParams {
    pub fn new(config: &ExplorerConfig, state: &ExplorerState) -> Self {
        Self {
            show_hidden: state.show_hidden(config),
            directories_first: config.directories_first,
            scan: ScanOptions::default(),
        }
    }

    /// Replaces the ignore settings the filter scan uses.
    pub fn with_scan(mut self, scan: ScanOptions) -> Self {
        self.scan = scan;
        self
    }
}

// ---------------------------------------------------------------------------
// Tree model
// ---------------------------------------------------------------------------

/// One visible row of the tree: a flattened, depth-annotated walk over every
/// expanded directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// Display name (file name component). The root row uses its tail.
    pub name: String,
    /// Nesting depth: root is 0, immediate children are 1, ...
    pub depth: u16,
    pub is_dir: bool,
    /// Directories only: whether currently expanded.
    pub expanded: bool,
    /// Positionally the last child of its parent (drives `└─` vs `├─`).
    pub is_last: bool,
    /// The `...` row above the root, which goes up to the parent directory
    /// ([`Tree::set_show_parent`]). Its `path` is that parent directory.
    pub is_parent: bool,
    /// Directories only: nothing (that would be listed) inside.
    pub is_empty: bool,
}

#[derive(Debug)]
struct Child {
    path: PathBuf,
    name: String,
    /// Lowercased name, cached so comparisons never allocate.
    sort_key: String,
    is_dir: bool,
    /// Directories only: whether it has nothing to list, as of the time its
    /// parent was loaded.
    is_empty: bool,
}

#[derive(Debug)]
struct Dir {
    expanded: bool,
    children: Vec<Child>,
}

// ---------------------------------------------------------------------------
// Floating window geometry
// ---------------------------------------------------------------------------

/// Smallest the floating window can be dragged to.
pub const MIN_FLOAT_WIDTH: u16 = 16;
pub const MIN_FLOAT_HEIGHT: u16 = 6;

/// Position and size of the floating window, in cells relative to the area it
/// floats in. Saved between sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatGeometry {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// What a mouse drag on the floating window's border does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatDrag {
    /// Move the whole window (top border).
    Move,
    /// Pull the marked edges (two for a corner).
    Resize {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
}

impl FloatDrag {
    /// The drag started by pressing at (`column`, `row`) on `frame`, if that
    /// cell is on the border. The top border moves the window, except at its
    /// corners; every other border cell resizes.
    pub fn at(frame: Rect, column: u16, row: u16) -> Option<Self> {
        if frame.width < 3 || frame.height < 3 {
            return None;
        }
        if column < frame.x || column >= frame.right() || row < frame.y || row >= frame.bottom() {
            return None;
        }
        let left = column == frame.x;
        let right = column == frame.right() - 1;
        let top = row == frame.y;
        let bottom = row == frame.bottom() - 1;
        if !(left || right || top || bottom) {
            return None;
        }
        if top && !left && !right {
            return Some(Self::Move);
        }
        Some(Self::Resize {
            left,
            right,
            top,
            bottom,
        })
    }

    /// The geometry after dragging from `start` by (`dx`, `dy`) cells, kept
    /// inside a `bounds_width` x `bounds_height` area and above the minimum
    /// size.
    pub fn apply(
        self,
        start: FloatGeometry,
        dx: i32,
        dy: i32,
        bounds_width: u16,
        bounds_height: u16,
    ) -> FloatGeometry {
        let (bw, bh) = (bounds_width as i32, bounds_height as i32);
        let (mut x, mut y) = (start.x as i32, start.y as i32);
        let (mut w, mut h) = (start.width as i32, start.height as i32);
        match self {
            Self::Move => {
                x = (x + dx).clamp(0, (bw - w).max(0));
                y = (y + dy).clamp(0, (bh - h).max(0));
            }
            Self::Resize {
                left,
                right,
                top,
                bottom,
            } => {
                let (min_w, min_h) = (MIN_FLOAT_WIDTH as i32, MIN_FLOAT_HEIGHT as i32);
                if right {
                    w = (w + dx).clamp(min_w, (bw - x).max(min_w));
                }
                if bottom {
                    h = (h + dy).clamp(min_h, (bh - y).max(min_h));
                }
                if left {
                    // The right edge stays put while the left one moves.
                    let right_edge = x + w;
                    x = (x + dx).clamp(0, (right_edge - min_w).max(0));
                    w = right_edge - x;
                }
                if top {
                    let bottom_edge = y + h;
                    y = (y + dy).clamp(0, (bottom_edge - min_h).max(0));
                    h = bottom_edge - y;
                }
            }
        }
        FloatGeometry {
            x: x.max(0) as u16,
            y: y.max(0) as u16,
            width: w.max(1) as u16,
            height: h.max(1) as u16,
        }
    }
}

fn floating_geometry_path() -> PathBuf {
    helix_loader::state_dir().join("explorer-float.toml")
}

/// The floating window geometry saved by an earlier session, if any.
pub fn load_floating_geometry() -> Option<FloatGeometry> {
    let text = fs::read_to_string(floating_geometry_path()).ok()?;
    toml::from_str(&text).ok()
}

/// Saves (or, for `None`, forgets) the floating window geometry. It lives in
/// the state directory rather than in `config.toml`, so the config file is
/// never rewritten behind the user's back; `floating-width` and friends in the
/// config remain the defaults until a window is moved or resized.
pub fn save_floating_geometry(geometry: Option<FloatGeometry>) -> io::Result<()> {
    let path = floating_geometry_path();
    match geometry {
        Some(geometry) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let text = toml::to_string(&geometry).map_err(io::Error::other)?;
            fs::write(path, text)
        }
        None => match fs::remove_file(path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        },
    }
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Upper bound on the entries a filter scan collects. Past this the scan stops
/// and the result is flagged as truncated (see [`Tree::filter_truncated`]).
pub const FILTER_SCAN_LIMIT: usize = 100_000;

/// Upper bound on the entries a filter shows. A broad query ("a") would
/// otherwise turn into tens of thousands of rows that nobody can read and that
/// make every keystroke slow; past this the result is flagged as capped (see
/// [`Tree::filter_capped`]) and the user narrows the query instead.
pub const FILTER_RESULT_LIMIT: usize = 2_000;

/// Entries the scan thread hands over in its first chunk, so results appear
/// almost immediately, and in every following chunk.
const SCAN_FIRST_CHUNK: usize = 256;
const SCAN_CHUNK: usize = 4_096;

/// Case-insensitive glob match: `*` matches any run of characters, `?` one
/// character, and `[abc]` / `[a-z]` / `[!abc]` a character class.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let txt: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    glob_match_chars(&pat, &txt)
}

/// Matches one `[...]` class at the start of `pat` (which begins with `[`)
/// against `c`. Returns `(matched, pattern length consumed)`, or `None` when the
/// class is not closed (the `[` is then an ordinary character).
fn match_class(pat: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(pat.get(i), Some('!') | Some('^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pat.len() {
        // A `]` right after the opening (or negation) is a literal member.
        if pat[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
            if pat[i] <= c && c <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

fn glob_match_chars(pat: &[char], txt: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    // Position to resume from after the most recent `*`: (pattern index after
    // the star, text index the star currently swallows up to).
    let mut star: Option<(usize, usize)> = None;

    while t < txt.len() {
        let step = match pat.get(p) {
            Some('*') => {
                star = Some((p + 1, t));
                p += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => match match_class(&pat[p..], txt[t]) {
                Some((true, len)) => Some(len),
                Some((false, _)) => None,
                // Unclosed class: `[` is a literal.
                None => (txt[t] == '[').then_some(1),
            },
            Some(&c) => (c == txt[t]).then_some(1),
            None => None,
        };
        match step {
            Some(len) => {
                p += len;
                t += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    // Let the star swallow one more character and retry.
                    star = Some((sp, st + 1));
                    p = sp;
                    t = st + 1;
                }
                None => return false,
            },
        }
    }
    while pat.get(p) == Some(&'*') {
        p += 1;
    }
    p == pat.len()
}

/// Expands `{a,b}` alternations (not nested) into plain patterns, so
/// `*.{rs,toml}` becomes `*.rs` and `*.toml`.
fn expand_braces(pattern: &str) -> Vec<String> {
    if let Some(open) = pattern.find('{') {
        if let Some(len) = pattern[open..].find('}') {
            let close = open + len;
            let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
            return pattern[open + 1..close]
                .split(',')
                .flat_map(|alt| expand_braces(&format!("{head}{alt}{tail}")))
                .collect();
        }
    }
    vec![pattern.to_string()]
}

/// One glob alternative of a [`Filter`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// Lowercased pattern characters, for the glob matcher.
    chars: Vec<char>,
    /// Match against the relative path instead of the entry name.
    on_path: bool,
}

impl Pattern {
    fn glob(pattern: &str) -> Self {
        Self {
            chars: pattern.chars().flat_map(char::to_lowercase).collect(),
            on_path: pattern.contains('/'),
        }
    }
}

/// How a [`Filter`] matches.
#[derive(Debug, Clone)]
enum FilterKind {
    /// Wildcard query: whole-name glob match.
    Glob(Vec<Pattern>),
    /// Everything else: fuzzy matching and ranking, like the pickers.
    Fuzzy {
        pattern: Arc<FuzzyPattern>,
        /// Match against the relative path instead of the entry name.
        on_path: bool,
    },
}

/// A matcher configured like the file pickers' (bonuses for path separators).
pub fn new_matcher() -> Matcher {
    Matcher::new(Config::DEFAULT.match_paths())
}

/// A parsed filter query for the explorer's input box.
///
/// A query without wildcard characters (`* ? [ {`) is a fuzzy search, matched
/// and ranked exactly like in the pickers: smart case (an uppercase letter
/// makes it case sensitive), space separated terms that must all match, and
/// the operators `^prefix`, `suffix$`, `'exact` and `!negated`. Typing `mnrs`
/// finds `main.rs`. A query with wildcards is a glob that must match the
/// whole name, e.g. `*.rs` or `test_*`, with `{a,b}` alternatives. A query
/// containing `/` is matched against the path relative to the tree root, any
/// other against the entry's own name.
#[derive(Debug, Clone)]
pub struct Filter {
    raw: String,
    kind: FilterKind,
}

impl PartialEq for Filter {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Eq for Filter {}

impl Filter {
    /// Parses `raw`; blank input means "no filter".
    pub fn new(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let kind = if raw.contains(['*', '?', '[', '{']) {
            FilterKind::Glob(
                expand_braces(raw)
                    .iter()
                    .map(|pattern| Pattern::glob(pattern))
                    .collect(),
            )
        } else {
            FilterKind::Fuzzy {
                pattern: Arc::new(FuzzyPattern::parse(
                    raw,
                    CaseMatching::Smart,
                    Normalization::Smart,
                )),
                on_path: raw.contains('/'),
            }
        };
        Some(Self {
            raw: raw.to_string(),
            kind,
        })
    }

    /// The query text, trimmed.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Whether this is a fuzzy (ranked) search rather than a glob.
    pub fn is_fuzzy(&self) -> bool {
        matches!(self.kind, FilterKind::Fuzzy { .. })
    }

    /// Whether an entry called `name`, at `relative` (a `/`-separated path
    /// below the tree root), passes the filter.
    pub fn matches(&self, name: &str, relative: &str) -> bool {
        let mut buf = Vec::new();
        match &self.kind {
            FilterKind::Glob(patterns) => glob_any(
                patterns,
                &name.to_lowercase(),
                &relative.to_lowercase(),
                &mut buf,
            ),
            FilterKind::Fuzzy { pattern, on_path } => {
                let text = if *on_path { relative } else { name };
                pattern
                    .score(Utf32Str::new(text, &mut buf), &mut new_matcher())
                    .is_some()
            }
        }
    }

    /// The score of a scanned entry, `None` when it does not match. Globs all
    /// score 0 (the tree order then decides). `buf` is a reusable buffer.
    fn score(&self, entry: &ScanEntry, matcher: &mut Matcher, buf: &mut Vec<char>) -> Option<u32> {
        match &self.kind {
            FilterKind::Glob(patterns) => {
                glob_any(patterns, &entry.name_lower, &entry.relative_lower, buf).then_some(0)
            }
            FilterKind::Fuzzy { pattern, on_path } => {
                let text = if *on_path {
                    &entry.relative
                } else {
                    &entry.name
                };
                pattern.score(Utf32Str::new(text, buf), matcher)
            }
        }
    }

    /// The character positions of `name` that the fuzzy query matched, for
    /// highlighting; empty for globs and non-matches. `relative` is the
    /// entry's path below the tree root.
    pub fn match_indices(
        &self,
        name: &str,
        relative: &str,
        matcher: &mut Matcher,
        out: &mut Vec<u32>,
    ) {
        out.clear();
        let FilterKind::Fuzzy { pattern, on_path } = &self.kind else {
            return;
        };
        let mut buf = Vec::new();
        if *on_path {
            if pattern
                .indices(Utf32Str::new(relative, &mut buf), matcher, out)
                .is_none()
            {
                out.clear();
                return;
            }
            // Keep the part of the match that falls inside the name.
            let offset = relative
                .chars()
                .count()
                .saturating_sub(name.chars().count()) as u32;
            out.retain(|&i| i >= offset);
            for i in out.iter_mut() {
                *i -= offset;
            }
        } else if pattern
            .indices(Utf32Str::new(name, &mut buf), matcher, out)
            .is_none()
        {
            out.clear();
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Whether any glob alternative matches the (already lowercased) text.
fn glob_any(patterns: &[Pattern], name: &str, relative: &str, scratch: &mut Vec<char>) -> bool {
    patterns.iter().any(|pattern| {
        let text = if pattern.on_path { relative } else { name };
        scratch.clear();
        scratch.extend(text.chars());
        glob_match_chars(&pattern.chars, scratch)
    })
}

/// One entry of the recursive listing a filter searches.
#[derive(Debug)]
struct ScanEntry {
    path: PathBuf,
    name: String,
    /// Lowercased `name`, so matching never has to lowercase again.
    name_lower: String,
    /// `/`-separated path below the tree root.
    relative: String,
    /// Lowercased `relative`.
    relative_lower: String,
    /// Index of the parent directory's entry, `None` for the root's children.
    /// Parents always precede their children in the scan.
    parent: Option<u32>,
    is_dir: bool,
}

/// What the scan thread sends back.
enum ScanMsg {
    Chunk(Vec<ScanEntry>),
    Done { truncated: bool },
}

/// Walks `root` on its own thread, honoring the ignore files selected in
/// `options`, and streams the entries back in chunks. The thread ends when
/// the walk does, or as soon as the receiver is dropped.
fn spawn_scan(root: PathBuf, show_hidden: bool, options: ScanOptions) -> ScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);
    if let Err(err) = std::thread::Builder::new()
        .name("explorer-scan".into())
        .spawn(move || run_scan(&root, show_hidden, options, &tx, &thread_cancel))
    {
        // The receiver sees the channel close and the filter simply has no
        // results; say why in the log.
        log::error!("failed to start the explorer scan thread: {err}");
    }
    ScanHandle { rx, cancel }
}

/// The receiving end of a running scan. Dropping it (filter cleared, tree
/// refreshed) tells the thread to stop, so abandoned walks of a huge tree do
/// not pile up.
struct ScanHandle {
    rx: mpsc::Receiver<ScanMsg>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ScanHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScanHandle")
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn run_scan(
    root: &Path,
    show_hidden: bool,
    options: ScanOptions,
    tx: &mpsc::Sender<ScanMsg>,
    cancel: &AtomicBool,
) {
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(!show_hidden)
        .parents(options.parents)
        .ignore(options.ignore)
        .git_ignore(options.git_ignore)
        .git_global(options.git_global)
        .git_exclude(options.git_exclude)
        // Never follow symlinks: it keeps the walk finite.
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .add_custom_ignore_filename(helix_loader::config_dir().join("ignore"))
        .add_custom_ignore_filename(".helix/ignore");

    let mut dir_index: HashMap<PathBuf, u32> = HashMap::new();
    let mut chunk: Vec<ScanEntry> = Vec::with_capacity(SCAN_FIRST_CHUNK);
    let mut chunk_size = SCAN_FIRST_CHUNK;
    let mut total = 0usize;
    let mut truncated = false;

    for result in walker.build() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        if total >= FILTER_SCAN_LIMIT {
            truncated = true;
            break;
        }
        let file_type = entry.file_type();
        let real_dir = file_type.is_some_and(|ft| ft.is_dir());
        let path = entry.into_path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let is_dir = real_dir
            || file_type.is_some_and(|ft| ft.is_symlink())
                && fs::metadata(&path).is_ok_and(|m| m.is_dir());
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let parent = path.parent().and_then(|p| dir_index.get(p).copied());
        if real_dir {
            dir_index.insert(path.clone(), total as u32);
        }
        chunk.push(ScanEntry {
            name_lower: name.to_lowercase(),
            relative_lower: relative.to_lowercase(),
            relative,
            path,
            name,
            parent,
            is_dir,
        });
        total += 1;

        if chunk.len() >= chunk_size {
            if tx.send(ScanMsg::Chunk(std::mem::take(&mut chunk))).is_err() {
                return;
            }
            chunk_size = SCAN_CHUNK;
            helix_event::request_redraw();
        }
    }
    if !chunk.is_empty() && tx.send(ScanMsg::Chunk(chunk)).is_err() {
        return;
    }
    if tx.send(ScanMsg::Done { truncated }).is_ok() {
        helix_event::request_redraw();
    }
}

/// Lazily loaded directory tree.
#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    dirs: HashMap<PathBuf, Dir>,
    visible: Vec<Entry>,
    /// Active filter. While set, `visible` is built from `filter_dirs` (only
    /// matching entries and the directories leading to them) instead of `dirs`.
    filter: Option<Filter>,
    filter_dirs: HashMap<PathBuf, Dir>,
    /// Recursive listing of the whole tree, produced by a background thread
    /// when a filter starts and reused while the query is edited (reset when
    /// the filter is cleared or the tree is refreshed).
    scan: Vec<ScanEntry>,
    /// Whether a scan was started for the current filter.
    scan_started: bool,
    /// Channel of the scan still in flight, `None` once it has finished.
    scan_rx: Option<ScanHandle>,
    scan_truncated: bool,
    /// Whether the last filter result was cut at [`FILTER_RESULT_LIMIT`].
    results_capped: bool,
    /// Whether to list a `...` row above the root.
    show_parent: bool,
    /// Scores of the previous fuzzy query, so typing one more character only
    /// re-scores what already matched.
    fuzzy_cache: Option<FuzzyCache>,
    matcher: Matcher,
    /// The best-scoring entry (and the best file) of the active filter.
    best_match: Option<PathBuf>,
    best_file: Option<PathBuf>,
}

/// Matches of a fuzzy query over the first `scanned` scan entries.
#[derive(Debug)]
struct FuzzyCache {
    raw: String,
    /// (scan index, score)
    scored: Vec<(u32, u32)>,
    scanned: usize,
}

impl Tree {
    /// Creates a tree rooted at `root`, loading the root's children.
    pub fn new(root: PathBuf, params: TreeParams) -> io::Result<Self> {
        let mut tree = Self {
            root,
            dirs: HashMap::new(),
            visible: Vec::new(),
            filter: None,
            filter_dirs: HashMap::new(),
            scan: Vec::new(),
            scan_started: false,
            scan_rx: None,
            scan_truncated: false,
            results_capped: false,
            show_parent: false,
            fuzzy_cache: None,
            matcher: new_matcher(),
            best_match: None,
            best_file: None,
        };
        let root = tree.root.clone();
        tree.expand(&root, params)?;
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn visible(&self) -> &[Entry] {
        &self.visible
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// Whether `path` is the tree root.
    pub fn is_root(&self, path: &Path) -> bool {
        path == self.root
    }

    /// The directory map the visible rows are built from: the filtered view
    /// while a filter is active, otherwise the full lazily loaded tree.
    fn active_dirs(&self) -> &HashMap<PathBuf, Dir> {
        if self.filter.is_some() {
            &self.filter_dirs
        } else {
            &self.dirs
        }
    }

    fn active_dirs_mut(&mut self) -> &mut HashMap<PathBuf, Dir> {
        if self.filter.is_some() {
            &mut self.filter_dirs
        } else {
            &mut self.dirs
        }
    }

    /// Whether the directory at `path` is expanded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.active_dirs().get(path).is_some_and(|dir| dir.expanded)
    }

    /// Whether `path` is the trailing entry among its parent directory's
    /// cached children. Drives the `│` vs blank guide lines in the renderer.
    pub fn is_last_child(&self, path: &Path) -> bool {
        path.parent()
            .and_then(|parent| self.active_dirs().get(parent))
            .is_some_and(|dir| dir.children.last().is_some_and(|child| child.path == path))
    }

    /// The active filter, if any.
    pub fn filter(&self) -> Option<&Filter> {
        self.filter.as_ref()
    }

    /// Whether the last filter scan hit [`FILTER_SCAN_LIMIT`] and so may be
    /// missing matches.
    pub fn filter_truncated(&self) -> bool {
        self.filter.is_some() && self.scan_truncated
    }

    /// Whether the filter has more matches than it shows
    /// ([`FILTER_RESULT_LIMIT`]); a narrower query reveals the rest.
    pub fn filter_capped(&self) -> bool {
        self.filter.is_some() && self.results_capped
    }

    /// The entry that matches the filter best (the first in tree order for
    /// globs). The explorer selects it, like a picker's top result.
    pub fn best_match(&self) -> Option<&Path> {
        self.best_match.as_deref().filter(|_| self.filter.is_some())
    }

    /// Like [`Tree::best_match`], restricted to files.
    pub fn best_file(&self) -> Option<&Path> {
        self.best_file.as_deref().filter(|_| self.filter.is_some())
    }

    /// Whether the background scan feeding the filter is still running.
    pub fn scanning(&self) -> bool {
        self.filter.is_some() && self.scan_rx.is_some()
    }

    fn load_children(&self, dir: &Path, params: TreeParams) -> io::Result<Vec<Child>> {
        let mut children = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                // Individual unreadable entries are skipped, not fatal.
                Err(_) => continue,
            };
            let path = entry.path();
            let name = match path.file_name() {
                Some(name) => name.to_string_lossy().into_owned(),
                None => continue,
            };
            if !params.show_hidden && name.starts_with('.') {
                continue;
            }
            // Symlinks are classified by their target: a link pointing at a
            // directory is expanded like a directory, not rendered as a file.
            // `entry.file_type()` reports the link kind, so follow the target
            // only when we must (this runs once per loaded directory).
            let is_dir = match entry.file_type() {
                Ok(ft) if ft.is_dir() => true,
                Ok(ft) if ft.is_symlink() => {
                    fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false)
                }
                _ => false,
            };
            let sort_key = name.to_lowercase();
            let is_empty = is_dir && dir_is_empty(&path, params.show_hidden);
            children.push(Child {
                path,
                name,
                sort_key,
                is_dir,
                is_empty,
            });
        }

        if params.directories_first {
            children.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.sort_key.cmp(&b.sort_key))
            });
        } else {
            children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        }
        Ok(children)
    }

    fn expand(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if self.is_expanded(path) {
            return Ok(());
        }
        if self.filter.is_some() {
            // Inside a filtered view a directory with matches below it just
            // re-opens; a directory that matched by name alone has nothing
            // cached, so its children are listed unfiltered to browse into.
            if let Some(dir) = self.filter_dirs.get_mut(path) {
                dir.expanded = true;
            } else {
                let children = self.load_children(path, params)?;
                self.filter_dirs.insert(
                    path.to_path_buf(),
                    Dir {
                        expanded: true,
                        children,
                    },
                );
            }
            self.rebuild_visible();
            return Ok(());
        }
        let children = self.load_children(path, params)?;
        self.dirs.insert(
            path.to_path_buf(),
            Dir {
                expanded: true,
                children,
            },
        );
        self.rebuild_visible();
        Ok(())
    }

    /// Expands the directory at `path` (no-op when already expanded).
    pub fn expand_at(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if !self.is_root(path) {
            // Only directories are expandable; a missing path errors out so
            // the caller can surface it.
            let meta = fs::metadata(path)?;
            if !meta.is_dir() {
                return Ok(());
            }
        }
        self.expand(path, params)
    }

    /// Collapses the directory at `path` (no-op for files).
    pub fn collapse(&mut self, path: &Path) {
        if let Some(dir) = self.active_dirs_mut().get_mut(path) {
            dir.expanded = false;
        }
        self.rebuild_visible();
    }

    /// Toggles the expansion state of the directory at `path`.
    pub fn toggle(&mut self, path: &Path, params: TreeParams) -> io::Result<()> {
        if self.is_expanded(path) {
            self.collapse(path);
            Ok(())
        } else {
            self.expand_at(path, params)
        }
    }

    /// Reloads every expanded directory from disk: pruning vanished entries
    /// and adding new ones. Collapsed directories are not touched. Directories
    /// that vanished are collapsed; only a vanished root counts as an error
    /// (returned as the error count).
    pub fn refresh(&mut self, params: TreeParams) -> usize {
        let mut errors = 0;
        let paths: Vec<PathBuf> = self
            .dirs
            .iter()
            .filter(|(_, dir)| dir.expanded)
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            match self.load_children(&path, params) {
                Ok(children) => {
                    if let Some(dir) = self.dirs.get_mut(&path) {
                        dir.children = children;
                    }
                }
                Err(_) => {
                    if path == self.root {
                        errors += 1;
                    } else if let Some(dir) = self.dirs.get_mut(&path) {
                        dir.expanded = false;
                    }
                }
            }
        }
        if self.filter.is_some() {
            // The listing the filter searches is stale now; search anew.
            self.reset_scan();
            self.start_scan(params);
            self.apply_filter(params);
        }
        self.rebuild_visible();
        errors
    }

    /// Sets the filter from the input box text (blank clears it). Matching
    /// entries stay visible together with the directories leading to them, so
    /// the result is a pruned tree of the whole workspace, not just of the
    /// directories that happen to be expanded. Clearing the filter restores
    /// the regular tree, with its expansion state, exactly as it was.
    pub fn set_filter(&mut self, raw: &str, params: TreeParams) {
        let filter = Filter::new(raw);
        if filter == self.filter {
            return;
        }
        match filter {
            None => {
                self.filter = None;
                self.filter_dirs.clear();
                self.reset_scan();
            }
            Some(filter) => {
                self.filter = Some(filter);
                self.start_scan(params);
                self.apply_filter(params);
            }
        }
        self.rebuild_visible();
    }

    /// Forgets the recursive listing and stops the thread producing it.
    fn reset_scan(&mut self) {
        self.scan.clear();
        self.scan_started = false;
        self.scan_rx = None;
        self.scan_truncated = false;
        self.results_capped = false;
        self.fuzzy_cache = None;
        self.best_match = None;
        self.best_file = None;
    }

    /// Starts the background scan the filter searches, unless one already ran
    /// for this filter. It skips `.git`, honors the ignore files selected in
    /// `params.scan` and, unless `show_hidden`, skips dotfiles. Symlinked
    /// directories are listed but not descended into.
    fn start_scan(&mut self, params: TreeParams) {
        if self.scan_started {
            return;
        }
        self.scan_started = true;
        self.scan_rx = Some(spawn_scan(
            self.root.clone(),
            params.show_hidden,
            params.scan,
        ));
    }

    fn absorb_scan(&mut self, msg: ScanMsg) {
        match msg {
            ScanMsg::Chunk(entries) => self.scan.extend(entries),
            ScanMsg::Done { truncated } => {
                self.scan_truncated = truncated;
                self.scan_rx = None;
            }
        }
    }

    /// Takes in whatever the scan thread produced since the last call and
    /// refreshes the filtered view. Returns `true` when the rows changed.
    /// Cheap when nothing arrived, so it can run on every frame.
    pub fn poll_scan(&mut self, params: TreeParams) -> bool {
        let mut changed = false;
        while let Some(handle) = &self.scan_rx {
            match handle.rx.try_recv() {
                Ok(msg) => {
                    self.absorb_scan(msg);
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan_rx = None;
                    changed = true;
                }
            }
        }
        if changed && self.filter.is_some() {
            self.apply_filter(params);
            self.rebuild_visible();
        }
        changed
    }

    /// Blocks until the scan has finished. For tests; the editor never waits.
    pub fn wait_for_scan(&mut self, params: TreeParams) {
        while let Some(handle) = &self.scan_rx {
            match handle.rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(msg) => self.absorb_scan(msg),
                Err(_) => self.scan_rx = None,
            }
        }
        if self.filter.is_some() {
            self.apply_filter(params);
            self.rebuild_visible();
        }
    }

    /// Rebuilds `filter_dirs` for the active filter from the entries scanned
    /// so far: every matching entry (up to [`FILTER_RESULT_LIMIT`]) plus all of
    /// its ancestors, grouped under their parents and sorted like a regular
    /// listing. Directories collapsed in the previous filtered view stay
    /// collapsed.
    fn apply_filter(&mut self, params: TreeParams) {
        let Some(filter) = self.filter.clone() else {
            return;
        };
        let scan = &self.scan;
        let fuzzy = filter.is_fuzzy();
        let mut buf = Vec::new();
        let matcher = &mut self.matcher;

        // Typing more of a fuzzy query can only remove matches (and new scan
        // entries get scored fresh), so re-score just what matched before.
        let cache = self
            .fuzzy_cache
            .take()
            .filter(|cache| fuzzy && cache.scanned <= scan.len());
        let (mut scored, start): (Vec<(u32, u32)>, usize) = match cache {
            Some(cache) if cache.raw == filter.raw() => (cache.scored, cache.scanned),
            Some(cache)
                if !cache.raw.is_empty()
                    && filter.raw().starts_with(&cache.raw)
                    && !filter.raw().contains('!') =>
            {
                let rescored = cache
                    .scored
                    .iter()
                    .filter_map(|&(i, _)| {
                        filter
                            .score(&scan[i as usize], matcher, &mut buf)
                            .map(|score| (i, score))
                    })
                    .collect();
                (rescored, cache.scanned)
            }
            _ => (Vec::new(), 0),
        };
        for (i, entry) in scan.iter().enumerate().skip(start) {
            if let Some(score) = filter.score(entry, matcher, &mut buf) {
                scored.push((i as u32, score));
            }
        }
        if fuzzy {
            self.fuzzy_cache = Some(FuzzyCache {
                raw: filter.raw().to_string(),
                scored: scored.clone(),
                scanned: scan.len(),
            });
        }

        // Best matches first (ties keep scan order); only the top results are
        // shown.
        let by_rank = |a: &(u32, u32), b: &(u32, u32)| b.1.cmp(&a.1).then(a.0.cmp(&b.0));
        let capped = scored.len() > FILTER_RESULT_LIMIT;
        if capped {
            scored.select_nth_unstable_by(FILTER_RESULT_LIMIT, by_rank);
            scored.truncate(FILTER_RESULT_LIMIT);
        }
        scored.sort_unstable_by(by_rank);
        self.best_match = scored.first().map(|&(i, _)| scan[i as usize].path.clone());
        self.best_file = scored
            .iter()
            .find(|&&(i, _)| !scan[i as usize].is_dir)
            .map(|&(i, _)| scan[i as usize].path.clone());

        let mut shown = vec![false; scan.len()];
        for &(i, _) in &scored {
            // Mark the entry and every ancestor; stop at the first one that is
            // already marked since its own ancestors are then marked too.
            let mut cur = Some(i as usize);
            while let Some(c) = cur {
                if shown[c] {
                    break;
                }
                shown[c] = true;
                cur = scan[c].parent.map(|p| p as usize);
            }
        }
        self.results_capped = capped;

        let mut grouped: HashMap<PathBuf, Vec<Child>> = HashMap::new();
        for (i, _) in shown.iter().enumerate().filter(|(_, &shown)| shown) {
            let entry = &scan[i];
            let parent = match entry.parent {
                Some(p) => scan[p as usize].path.clone(),
                None => self.root.clone(),
            };
            grouped.entry(parent).or_default().push(Child {
                path: entry.path.clone(),
                name: entry.name.clone(),
                sort_key: entry.name_lower.clone(),
                is_dir: entry.is_dir,
                is_empty: false,
            });
        }
        // The root row exists even when nothing matched.
        grouped.entry(self.root.clone()).or_default();

        let previous = std::mem::take(&mut self.filter_dirs);
        for (path, mut children) in grouped {
            if params.directories_first {
                children.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.sort_key.cmp(&b.sort_key))
                });
            } else {
                children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
            }
            let expanded = previous.get(&path).is_none_or(|dir| dir.expanded);
            self.filter_dirs.insert(path, Dir { expanded, children });
        }
    }

    /// Expands every ancestor of `path` so it becomes visible. Returns `true`
    /// if `path` shows up in the tree afterwards.
    ///
    /// Paths are compared lexically, not canonically: the tree stores entry
    /// paths in the same spelling as the root it was opened with (a root
    /// reached through a symlink must not collide with the canonical
    /// spelling of the same directory).
    pub fn reveal(&mut self, path: &Path, params: TreeParams) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }

        // A filtered view may not contain the target; reveal works on the
        // whole tree.
        if self.filter.is_some() {
            self.set_filter("", params);
        }

        // The root anchors every other expansion; make sure it is expanded.
        if !self.is_expanded(&self.root) {
            let root = self.root.clone();
            if self.expand(&root, params).is_err() {
                return false;
            }
        }

        // Collect the ancestor chain nearest-to-target-first, then expand
        // top-down. The walk always shortens the path, so it terminates on
        // its own.
        let mut ancestors: Vec<PathBuf> = Vec::with_capacity(16);
        let mut cur: PathBuf = path.to_path_buf();
        while cur != self.root {
            let parent = match cur.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return false,
            };
            cur = parent.clone();
            if cur != self.root {
                ancestors.push(parent);
            }
        }
        ancestors.reverse();

        for ancestor in ancestors.iter() {
            if self.is_expanded(ancestor) {
                continue;
            }
            if self.expand(ancestor, params).is_err() {
                self.rebuild_visible();
                return false;
            }
        }
        self.rebuild_visible();
        self.visible.iter().any(|entry| entry.path == path)
    }

    /// Rebuilds `visible` from the expanded directories.
    fn rebuild_visible(&mut self) {
        let dirs = if self.filter.is_some() {
            &self.filter_dirs
        } else {
            &self.dirs
        };
        // While filtering, the rows are search results: no way up.
        let with_parent = self.show_parent && self.filter.is_none();
        self.visible = build_visible(&self.root, dirs, with_parent);
    }

    /// Adds (or removes) a `...` row above the root that stands for its
    /// parent directory, when the root has one. Off by default.
    pub fn set_show_parent(&mut self, show: bool) {
        self.show_parent = show;
        self.rebuild_visible();
    }
}

/// Builds the flattened visible rows: root row first, then a depth-first
/// pre-order walk over expanded directories. Pure over `dirs`, so borrows
/// end when it returns.
fn build_visible(root: &Path, dirs: &HashMap<PathBuf, Dir>, with_parent: bool) -> Vec<Entry> {
    let mut visible = Vec::new();
    let Some(root_dir) = dirs.get(root) else {
        return visible;
    };

    if let Some(parent) = root.parent().filter(|_| with_parent) {
        visible.push(Entry {
            path: parent.to_path_buf(),
            name: "...".to_string(),
            depth: 0,
            is_dir: true,
            expanded: false,
            is_last: false,
            is_parent: true,
            is_empty: false,
        });
    }

    visible.push(Entry {
        path: root.to_path_buf(),
        name: root_display_name(root),
        depth: 0,
        is_dir: true,
        expanded: root_dir.expanded,
        is_last: true,
        is_parent: false,
        is_empty: root_dir.children.is_empty(),
    });
    if !root_dir.expanded {
        return visible;
    }

    // Depth-first pre-order with a stack of child iterators, held as
    // (next_index, slice, depth) frames so children can be pushed without
    // holding a mutable borrow across the iteration. Deep trees cannot blow
    // the stack.
    let mut stack: Vec<(usize, &[Child], u16)> = vec![(0, root_dir.children.as_slice(), 1u16)];
    while let Some((idx, children, depth)) = stack.last().map(|f| (f.0, f.1, f.2)) {
        match children.get(idx) {
            Some(child) => {
                stack.last_mut().expect("frame just peeked").0 += 1;
                visible.push(Entry {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    is_dir: child.is_dir,
                    expanded: dirs.get(&child.path).is_some_and(|d| d.expanded),
                    is_last: false,
                    is_parent: false,
                    // A loaded directory knows its own contents; otherwise
                    // go by what was seen when its parent was listed.
                    is_empty: child.is_dir
                        && dirs
                            .get(&child.path)
                            .map_or(child.is_empty, |d| d.children.is_empty()),
                });
                if child.is_dir {
                    if let Some(dir) = dirs.get(&child.path) {
                        if dir.expanded && !dir.children.is_empty() {
                            stack.push((0, dir.children.as_slice(), depth + 1));
                        }
                    }
                }
            }
            None => {
                stack.pop();
            }
        }
    }

    // Patch `is_last`: a row is last when it is the final entry of its
    // parent directory's cached child list.
    for entry in visible.iter_mut().skip(1) {
        if let Some(parent) = entry.path.parent() {
            if let Some(dir) = dirs.get(parent) {
                entry.is_last = dir
                    .children
                    .last()
                    .is_some_and(|child| child.path == entry.path);
            }
        }
    }
    visible
}

/// Whether `dir` has nothing a listing would show. Stops at the first entry,
/// so it is cheap even for huge directories; an unreadable directory counts as
/// non-empty.
fn dir_is_empty(dir: &Path, show_hidden: bool) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    !entries
        .flatten()
        .any(|entry| show_hidden || !entry.file_name().to_string_lossy().starts_with('.'))
}

fn root_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Recursively copies `from` (file or directory) onto `to` (entries are
/// replaced as they are copied). Used for paste.
pub fn copy_recursively(from: &Path, to: &Path) -> io::Result<()> {
    let file_type = fs::symlink_metadata(from)?.file_type();

    if file_type.is_dir() {
        // Copying a directory into itself would never terminate.
        if to.starts_with(from) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot copy a directory into itself",
            ));
        }
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if file_type.is_symlink() {
        // Links are copied as links, never followed: a link pointing at one
        // of its own ancestors would otherwise recurse forever.
        #[cfg(unix)]
        {
            let target = fs::read_link(from)?;
            if fs::symlink_metadata(to).is_ok_and(|meta| !meta.is_dir()) {
                fs::remove_file(to)?;
            }
            return std::os::unix::fs::symlink(target, to);
        }
        #[cfg(not(unix))]
        if fs::metadata(from).is_ok_and(|meta| meta.is_dir()) {
            return Ok(());
        }
    }
    fs::copy(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn params(show_hidden: bool, directories_first: bool) -> TreeParams {
        TreeParams {
            show_hidden,
            directories_first,
            ..TreeParams::default()
        }
    }

    fn names(tree: &Tree) -> Vec<String> {
        tree.visible().iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn visible_order_and_lazy_expansion() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("b_dir")).unwrap();
        std::fs::create_dir(root.join("a_dir")).unwrap();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::write(root.join("m.txt"), "m").unwrap();
        std::fs::create_dir(root.join("a_dir").join("nested")).unwrap();
        std::fs::write(root.join("a_dir").join("inner.txt"), "i").unwrap();
        std::fs::create_dir(root.join(".hidden_dir")).unwrap();
        std::fs::write(root.join(".dotfile"), "d").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        // Root row, then directories first (sorted), then files (sorted).
        // Hidden entries are excluded.
        assert_eq!(
            names(&tree),
            vec![
                root.file_name().unwrap().to_string_lossy().into_owned(),
                "a_dir".into(),
                "b_dir".into(),
                "m.txt".into(),
                "z.txt".into(),
            ]
        );
        assert!(!tree.visible()[3].is_last); // m.txt
        assert!(tree.visible()[4].is_last); // z.txt is the last row

        // expand: adds "nested" (dir first) then "inner.txt" at depth 2
        tree.expand_at(&root.join("a_dir"), p).unwrap();
        let vis = tree.visible();
        assert_eq!(vis.len(), 7);
        assert_eq!(vis[2].name, "nested");
        assert_eq!(vis[2].depth, 2);
        assert!(!vis[2].is_last);
        assert_eq!(vis[3].name, "inner.txt");
        assert_eq!(vis[3].depth, 2);
        assert!(vis[3].is_last);

        // collapse removes the subtree from view but keeps data.
        tree.collapse(&root.join("a_dir"));
        assert_eq!(tree.visible().len(), 5);

        toggling_hidden_works(&mut tree, root);
    }

    fn toggling_hidden_works(tree: &mut Tree, root: &Path) {
        tree.toggle(root, params(true, true)).unwrap(); // collapse root
        tree.toggle(root, params(true, true)).unwrap(); // re-expand, now showing hidden
        assert!(names(tree).iter().any(|n| n == ".dotfile"));
        assert!(names(tree).iter().any(|n| n == ".hidden_dir"));
        // With mt_sort... directories still first
        let first_dirs: Vec<bool> = tree
            .visible()
            .iter()
            .skip(1)
            .map_while(|e| Some(e.is_dir))
            .take(3)
            .collect();
        assert_eq!(first_dirs, vec![true, true, true]);
    }

    #[test]
    fn reveal_expands_ancestors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        let file = root.join("sub/deep/file.txt");
        std::fs::write(&file, "f").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        assert!(tree.reveal(&file, p));
        assert!(tree.visible().iter().any(|e| e.path == file));
        assert!(tree.is_expanded(&root.join("sub")));
        assert!(tree.is_expanded(&root.join("sub/deep")));

        // Outside the root cannot be revealed.
        assert!(!tree.reveal(tmp.path().parent().unwrap(), p));

        // The root itself reveals fine.
        assert!(tree.reveal(root, p));
        assert!(tree.is_expanded(root));

        // Missing files reveal nothing (target must exist).
        assert!(!tree.reveal(&root.join("nope.txt"), p));
    }

    #[test]
    fn refresh_prunes_and_adds() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::create_dir(root.join("gone")).unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();
        assert!(names(&tree).iter().any(|n| n == "gone"));

        std::fs::write(root.join("new.txt"), "n").unwrap();
        std::fs::remove_file(root.join("z.txt")).unwrap();
        std::fs::remove_dir(root.join("gone")).unwrap();

        assert_eq!(tree.refresh(p), 0);
        tree.wait_for_scan(p);
        let n = names(&tree);
        assert!(n.iter().any(|n| n == "new.txt"));
        assert!(!n.iter().any(|n| n == "z.txt"));
        assert!(!n.iter().any(|n| n == "gone"));
    }

    #[test]
    fn missing_root_errors() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("no_such_dir");
        assert!(Tree::new(missing, params(false, true)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reveal_works_through_symlinked_root() {
        // The root is reached through a symlink (as happens when TMPDIR or
        // /var on macOS resolves through one). Reveal must operate in the
        // root's own spelling, not the canonical one.
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        std::fs::create_dir_all(link.join("sub/deep")).unwrap();
        let file = link.join("sub/deep/file.txt");
        std::fs::write(&file, "f").unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(link.clone(), p).unwrap();
        assert!(tree.reveal(&file, p));
        assert!(tree.visible().iter().any(|e| e.path == file));
        assert!(tree.is_expanded(&link.join("sub")));
        assert!(tree.is_expanded(&link.join("sub/deep")));

        // The root itself still reveals.
        assert!(tree.reveal(&link, p));
    }

    #[test]
    fn recursive_copy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/data")).unwrap();
        std::fs::write(root.join("src/data/a.txt"), "a").unwrap();
        std::fs::write(root.join("src/top.txt"), "t").unwrap();

        copy_recursively(&root.join("src"), &root.join("dst")).unwrap();
        assert!(root.join("dst/data/a.txt").is_file());
        assert!(root.join("dst/top.txt").is_file());

        // Files copy through too.
        copy_recursively(&root.join("src/top.txt"), &root.join("top2.txt")).unwrap();
        assert!(root.join("top2.txt").is_file());
    }

    // Symlink classification uses `fs::metadata`, which is not available on
    // all targets' std APIs in the same shape; keep the test unix-only.
    #[cfg(unix)]
    #[test]
    fn symlinked_dirs_render_as_directories() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("real_dir")).unwrap();
        std::fs::write(root.join("real_dir/file.txt"), "f").unwrap();
        std::fs::write(root.join("loose.txt"), "l").unwrap();
        symlink(root.join("real_dir"), root.join("linked_dir")).unwrap();
        // A link whose target does not exist is kind of a file row: it can
        // not be expanded like a directory.
        symlink(root.join("missing_target"), root.join("dangling")).unwrap();

        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();

        let linked = tree
            .visible()
            .iter()
            .find(|e| e.name == "linked_dir")
            .expect("linked_dir row exists")
            .clone();
        assert!(linked.is_dir, "symlink to a directory is a dir row");
        let dangling = tree
            .visible()
            .iter()
            .find(|e| e.name == "dangling")
            .expect("dangling row exists")
            .clone();
        assert!(!dangling.is_dir, "dangling link is a file-like row");

        // Expanding the linked directory lists the target's children.
        tree.expand_at(&root.join("linked_dir"), p).unwrap();
        assert!(tree
            .visible()
            .iter()
            .any(|e| e.name == "file.txt" && e.depth == 2));
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.RS", "main.rs"), "case-insensitive");
        assert!(!glob_match("*.rs", "main.rs.bak"));
        assert!(glob_match("test_*", "test_foo"));
        assert!(!glob_match("test_*", "a_test_foo"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        // Character classes.
        assert!(glob_match("[abc].txt", "b.txt"));
        assert!(!glob_match("[abc].txt", "d.txt"));
        assert!(glob_match("file[0-9]", "file7"));
        assert!(!glob_match("file[0-9]", "filex"));
        assert!(glob_match("[!a]x", "bx"));
        assert!(!glob_match("[!a]x", "ax"));
        // An unclosed class is literal.
        assert!(glob_match("a[b", "a[b"));
    }

    #[test]
    fn filter_patterns() {
        assert!(Filter::new("").is_none());
        assert!(Filter::new("   ").is_none());

        // No wildcards: substring.
        let f = Filter::new("main").unwrap();
        assert!(f.matches("main.rs", "src/main.rs"));
        assert!(f.matches("domain.txt", "domain.txt"));
        assert!(!f.matches("lib.rs", "src/lib.rs"));

        // Wildcards: whole-name match, with brace alternatives.
        let f = Filter::new("*.{rs,toml}").unwrap();
        assert!(f.matches("a.rs", "a.rs"));
        assert!(f.matches("Cargo.toml", "Cargo.toml"));
        assert!(!f.matches("a.md", "a.md"));

        // A slash matches against the path below the root.
        let f = Filter::new("src/*.rs").unwrap();
        assert!(f.matches("main.rs", "src/main.rs"));
        assert!(!f.matches("main.rs", "tests/main.rs"));
    }

    fn filter_fixture() -> (TempDir, Tree, TreeParams) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/deep/inner.rs"), "").unwrap();
        std::fs::write(root.join("src/notes.md"), "").unwrap();
        std::fs::write(root.join("docs/guide.md"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join(".hidden.rs"), "").unwrap();
        let p = params(false, true);
        let tree = Tree::new(root.to_path_buf(), p).unwrap();
        (tmp, tree, p)
    }

    #[test]
    fn filter_shows_matches_anywhere_with_their_ancestors() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        // Nothing is expanded yet, yet the filter reaches into directories.
        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        let shown: Vec<String> = names(&tree).into_iter().skip(1).collect();
        assert_eq!(shown, vec!["src", "deep", "inner.rs", "main.rs"]);

        let inner = tree
            .visible()
            .iter()
            .find(|e| e.name == "inner.rs")
            .unwrap();
        assert_eq!(inner.depth, 3);
        assert!(inner.is_last, "last among the matches of deep/");
        let main = tree.visible().iter().find(|e| e.name == "main.rs").unwrap();
        assert!(
            main.is_last,
            "notes.md is filtered out, so main.rs is last in src/"
        );
        assert!(tree.is_last_child(&root.join("src/main.rs")));

        // Hidden files stay hidden, like in the regular tree.
        assert!(!names(&tree).iter().any(|n| n == ".hidden.rs"));
    }

    #[test]
    fn filter_with_no_matches_leaves_the_root_row() {
        let (tmp, mut tree, p) = filter_fixture();
        tree.set_filter("*.nothing", p);
        tree.wait_for_scan(p);
        assert_eq!(tree.visible().len(), 1);
        assert_eq!(tree.visible()[0].path, tmp.path());
    }

    #[test]
    fn clearing_the_filter_restores_the_full_tree() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.expand_at(&root.join("docs"), p).unwrap();
        let before = names(&tree);

        tree.set_filter("inner", p);

        tree.wait_for_scan(p);
        assert_ne!(names(&tree), before);
        tree.set_filter("", p);
        assert!(tree.filter().is_none());
        // Same rows, same expansion state as before filtering.
        assert_eq!(names(&tree), before);
        assert!(tree.is_expanded(&root.join("docs")));
        assert!(!tree.is_expanded(&root.join("src")));
    }

    #[test]
    fn filtered_directories_collapse_and_expand() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        assert!(tree.is_expanded(&root.join("src")));

        tree.collapse(&root.join("src"));
        assert_eq!(names(&tree).len(), 2); // root + collapsed src
        tree.toggle(&root.join("src"), p).unwrap();
        assert_eq!(names(&tree).len(), 5);

        // A directory that matched by name only can still be browsed: its
        // children are listed unfiltered.
        tree.set_filter("docs", p);
        tree.wait_for_scan(p);
        assert_eq!(names(&tree).len(), 2);
        tree.toggle(&root.join("docs"), p).unwrap();
        assert!(names(&tree).iter().any(|n| n == "guide.md"));
    }

    #[test]
    fn refresh_keeps_the_filter_and_sees_new_files() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        std::fs::write(root.join("src/extra.rs"), "").unwrap();
        std::fs::remove_file(root.join("src/main.rs")).unwrap();
        assert_eq!(tree.refresh(p), 0);
        tree.wait_for_scan(p);
        let shown = names(&tree);
        assert!(shown.iter().any(|n| n == "extra.rs"));
        assert!(!shown.iter().any(|n| n == "main.rs"));
        assert!(tree.filter().is_some());
    }

    #[test]
    fn reveal_drops_a_filter_that_hides_the_target() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path().to_path_buf();
        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        assert!(tree.reveal(&root.join("docs/guide.md"), p));
        assert!(tree.filter().is_none());
        assert!(tree.visible().iter().any(|e| e.name == "guide.md"));
    }

    #[test]
    fn hidden_files_can_be_filtered_when_shown() {
        let (tmp, mut tree, _) = filter_fixture();
        let p = params(true, true);
        tree.refresh(p);
        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        assert!(names(&tree).iter().any(|n| n == ".hidden.rs"));
        drop(tmp);
    }

    #[test]
    fn filter_honors_ignore_files() {
        let (tmp, mut tree, p) = filter_fixture();
        let root = tmp.path();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/built.rs"), "").unwrap();
        std::fs::write(root.join(".ignore"), "target/\n").unwrap();

        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        let shown = names(&tree);
        assert!(shown.iter().any(|n| n == "main.rs"));
        assert!(!shown.iter().any(|n| n == "built.rs"), "{shown:?}");

        // With the ignore files switched off the entry comes back.
        let mut open = p;
        open.scan = ScanOptions {
            parents: false,
            ignore: false,
            git_ignore: false,
            git_global: false,
            git_exclude: false,
        };
        tree.set_filter("", open);
        tree.set_filter("*.rs", open);
        tree.wait_for_scan(open);
        assert!(names(&tree).iter().any(|n| n == "built.rs"));
    }

    #[test]
    fn broad_filters_are_capped() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..FILTER_RESULT_LIMIT + 50 {
            std::fs::write(tmp.path().join(format!("file{i}.txt")), "").unwrap();
        }
        let p = params(false, true);
        let mut tree = Tree::new(tmp.path().to_path_buf(), p).unwrap();
        tree.set_filter("file", p);
        tree.wait_for_scan(p);
        assert!(tree.filter_capped());
        assert_eq!(tree.visible().len(), FILTER_RESULT_LIMIT + 1);

        // Narrowing the query lifts the cap.
        tree.set_filter("file1", p);
        tree.wait_for_scan(p);
        assert!(!tree.filter_capped());
    }

    #[test]
    fn plain_queries_are_fuzzy_with_smart_case() {
        let filter = Filter::new("mnrs").unwrap();
        assert!(filter.is_fuzzy());
        assert!(filter.matches("main.rs", "src/main.rs"));
        assert!(!filter.matches("lib.rs", "src/lib.rs"));
        // Lowercase queries ignore case, an uppercase letter turns it on.
        assert!(Filter::new("main").unwrap().matches("Main.rs", "Main.rs"));
        assert!(!Filter::new("Main").unwrap().matches("main.rs", "main.rs"));
        // Space separated terms must all match.
        let filter = Filter::new("mn rs").unwrap();
        assert!(filter.matches("main.rs", "main.rs"));
        assert!(!filter.matches("main.py", "main.py"));
        // A query with a slash is matched against the relative path.
        let filter = Filter::new("src/main").unwrap();
        assert!(filter.matches("main.rs", "src/main.rs"));
        assert!(!filter.matches("main.rs", "lib/main.rs"));
    }

    #[test]
    fn wildcard_queries_stay_globs() {
        let filter = Filter::new("*.rs").unwrap();
        assert!(!filter.is_fuzzy());
        assert!(filter.matches("main.rs", "src/main.rs"));
        assert!(!filter.matches("main.rs.bak", "main.rs.bak"));
    }

    #[test]
    fn fuzzy_match_indices_point_at_the_name() {
        let mut matcher = new_matcher();
        let mut out = Vec::new();
        let filter = Filter::new("mnrs").unwrap();
        filter.match_indices("main.rs", "src/main.rs", &mut matcher, &mut out);
        assert_eq!(out, vec![0, 3, 5, 6]);
        // Path queries only report the part inside the name.
        let filter = Filter::new("src/main").unwrap();
        filter.match_indices("main.rs", "src/main.rs", &mut matcher, &mut out);
        assert_eq!(out, vec![0, 1, 2, 3]);
        // Globs highlight nothing.
        let filter = Filter::new("*.rs").unwrap();
        filter.match_indices("main.rs", "src/main.rs", &mut matcher, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn fuzzy_filter_ranks_and_selects_the_best_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("a")).unwrap();
        fs::write(root.join("a/m_a_i_n.txt"), "").unwrap();
        fs::write(root.join("main.rs"), "").unwrap();
        fs::write(root.join("other.rs"), "").unwrap();
        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();
        tree.set_filter("main", p);
        tree.wait_for_scan(p);
        let names: Vec<_> = tree.visible().iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"main.rs"));
        assert!(names.contains(&"m_a_i_n.txt"));
        assert!(!names.contains(&"other.rs"));
        assert_eq!(tree.best_file(), Some(root.join("main.rs").as_path()));

        // Typing more narrows the earlier result set.
        tree.set_filter("main.r", p);
        tree.wait_for_scan(p);
        let names: Vec<_> = tree.visible().iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"main.rs"));
        assert!(!names.contains(&"m_a_i_n.txt"));

        tree.set_filter("", p);
        assert!(tree.best_match().is_none());
    }

    #[test]
    fn floating_frame_uses_geometry_config_or_center() {
        let area = Rect::new(0, 0, 100, 40);
        let mut config = ExplorerConfig::default();
        let mut state = ExplorerState::default();
        // Centered by default.
        assert_eq!(
            state.floating_frame(&config, area),
            Rect::new(30, 10, 40, 20)
        );
        // Config position.
        config.floating_x = Some(5);
        config.floating_y = Some(2);
        assert_eq!(state.floating_frame(&config, area), Rect::new(5, 2, 40, 20));
        // Mouse set geometry wins, and always fits inside the area.
        state.floating_geometry = Some(FloatGeometry {
            x: 90,
            y: 30,
            width: 50,
            height: 12,
        });
        assert_eq!(
            state.floating_frame(&config, area),
            Rect::new(50, 28, 50, 12)
        );
    }

    #[test]
    fn float_drag_hit_testing() {
        let frame = Rect::new(10, 5, 20, 10);
        assert_eq!(FloatDrag::at(frame, 15, 5), Some(FloatDrag::Move));
        assert_eq!(
            FloatDrag::at(frame, 29, 14),
            Some(FloatDrag::Resize {
                left: false,
                right: true,
                top: false,
                bottom: true
            })
        );
        assert_eq!(
            FloatDrag::at(frame, 10, 5),
            Some(FloatDrag::Resize {
                left: true,
                right: false,
                top: true,
                bottom: false
            })
        );
        assert_eq!(FloatDrag::at(frame, 15, 8), None);
        assert_eq!(FloatDrag::at(frame, 40, 8), None);
    }

    #[test]
    fn float_drag_moves_and_resizes_within_bounds() {
        let start = FloatGeometry {
            x: 10,
            y: 5,
            width: 20,
            height: 10,
        };
        let (bw, bh) = (100, 40);
        let moved = FloatDrag::Move.apply(start, 500, -500, bw, bh);
        assert_eq!(
            (moved.x, moved.y, moved.width, moved.height),
            (80, 0, 20, 10)
        );

        let right = FloatDrag::Resize {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        let grown = right.apply(start, 10, 5, bw, bh);
        assert_eq!((grown.width, grown.height), (30, 15));
        let tiny = right.apply(start, -100, -100, bw, bh);
        assert_eq!(
            (tiny.width, tiny.height),
            (MIN_FLOAT_WIDTH, MIN_FLOAT_HEIGHT)
        );

        // Pulling the left edge keeps the right edge in place.
        let left = FloatDrag::Resize {
            left: true,
            right: false,
            top: true,
            bottom: false,
        };
        let pulled = left.apply(start, -4, -2, bw, bh);
        assert_eq!(
            (pulled.x, pulled.y, pulled.width, pulled.height),
            (6, 3, 24, 12)
        );
        let far = left.apply(start, -100, -100, bw, bh);
        assert_eq!((far.x, far.y, far.width, far.height), (0, 0, 30, 15));
    }

    #[test]
    fn copy_refuses_to_copy_a_directory_into_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let err = copy_recursively(&dir, &dir.join("copy")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!dir.join("copy").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_keeps_symlinks_as_links_instead_of_following_them() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        // A link back to the directory that contains it.
        std::os::unix::fs::symlink(&src, src.join("loop")).unwrap();

        let dest = tmp.path().join("dest");
        copy_recursively(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "a");
        assert!(std::fs::symlink_metadata(dest.join("loop"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn empty_directories_are_flagged() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::create_dir(root.join("only_hidden")).unwrap();
        std::fs::write(root.join("only_hidden/.keep"), "").unwrap();
        std::fs::create_dir(root.join("full")).unwrap();
        std::fs::write(root.join("full/a.txt"), "").unwrap();
        std::fs::write(root.join("file.txt"), "").unwrap();

        let flags = |tree: &Tree| -> Vec<(String, bool)> {
            tree.visible()
                .iter()
                .skip(1)
                .map(|e| (e.name.clone(), e.is_empty))
                .collect()
        };
        let p = params(false, true);
        let mut tree = Tree::new(root.to_path_buf(), p).unwrap();
        assert_eq!(
            flags(&tree),
            vec![
                ("empty".into(), true),
                ("full".into(), false),
                ("only_hidden".into(), true),
                ("file.txt".into(), false),
            ]
        );
        // Showing hidden files makes the directory that only held one non-empty.
        let shown = params(true, true);
        tree.refresh(shown);
        assert!(flags(&tree).contains(&("only_hidden".into(), false)));
    }

    #[test]
    fn parent_row_comes_first_and_only_outside_filters() {
        let (tmp, mut tree, p) = filter_fixture();
        assert_eq!(tree.visible()[0].path, tmp.path());
        tree.set_show_parent(true);
        let first = &tree.visible()[0];
        assert!(first.is_parent && first.name == "...");
        assert_eq!(Some(first.path.as_path()), tmp.path().parent());
        assert_eq!(tree.visible()[1].path, tmp.path());

        tree.set_filter("*.rs", p);
        tree.wait_for_scan(p);
        assert!(!tree.visible().iter().any(|e| e.is_parent));
        tree.set_filter("", p);
        assert!(tree.visible()[0].is_parent);
    }

    #[test]
    fn floating_is_independent_of_the_pinned_side() {
        let config = ExplorerConfig::default();
        let mut state = ExplorerState {
            side: Some(ExplorerSide::Right),
            ..ExplorerState::default()
        };
        assert_eq!(state.side(&config), ExplorerSide::Right);

        state.visible = true;
        state.focused = true;
        state.floating = true;
        assert_eq!(state.side(&config), ExplorerSide::Floating);

        // Closing the float leaves the pinned side as it was.
        state.hide();
        assert!(!state.visible && !state.focused && !state.floating);
        assert_eq!(state.side(&config), ExplorerSide::Right);
    }
}
