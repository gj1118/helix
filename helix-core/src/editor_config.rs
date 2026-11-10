//! Support for [EditorConfig](https://EditorConfig.org) configuration loading.
//!
//! EditorConfig is an editor-agnostic format for specifying configuration in an INI-like, human
//! friendly syntax in `.editorconfig` files (which are intended to be checked into VCS). This
//! module provides functions to search for all `.editorconfig` files that apply to a given path
//! and returns an `EditorConfig` type containing any specified configuration options.
//!
//! At time of writing, this module follows the [spec](https://spec.editorconfig.org/) at
//! version 0.17.2.

use std::{
    collections::HashMap,
    fs,
    num::{NonZeroU16, NonZeroU8},
    path::Path,
};

use encoding_rs::Encoding;
use globset::{GlobBuilder, GlobMatcher};

use crate::{
    indent::{IndentStyle, MAX_INDENT},
    LineEnding,
};

/// Configuration declared for a path in `.editorconfig` files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EditorConfig {
    pub indent_style: Option<IndentStyle>,
    /// The columns of a level of indentation with spaces (`indent_size` as a number), also
    /// without `indent_style`: it then sets the width of the spaces the file is indented with.
    pub indent_width: Option<NonZeroU8>,
    pub tab_width: Option<NonZeroU8>,
    pub line_ending: Option<LineEnding>,
    pub encoding: Option<&'static Encoding>,
    /// Whether a new file starts with a byte order mark (`charset = utf-8-bom`).
    pub byte_order_mark: Option<bool>,
    // pub spelling_language: Option<SpellingLanguage>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub max_line_length: Option<NonZeroU16>,
}

impl EditorConfig {
    /// Finds any configuration in `.editorconfig` files which applies to the given path.
    ///
    /// If no configuration applies then `EditorConfig::default()` is returned.
    pub fn find(path: &Path) -> Self {
        let mut configs = Vec::new();
        // <https://spec.editorconfig.org/#file-processing>
        for ancestor in path.ancestors() {
            let editor_config_file = ancestor.join(".editorconfig");
            let Ok(contents) = fs::read_to_string(&editor_config_file) else {
                continue;
            };
            // A section whose glob does not compile is skipped (and logged), not the file.
            let ini = Ini::parse(&contents, &editor_config_file);
            let is_root = ini.pairs.get("root").map(AsRef::as_ref) == Some("true");
            configs.push((ini, ancestor));
            // > The search shall stop if an EditorConfig file is found with the `root` key set to
            // > `true` in the preamble or when reaching the root filesystem directory.
            if is_root {
                break;
            }
        }

        let mut pairs = Pairs::new();
        // Reverse the configuration stack so that the `.editorconfig` files closest to `path`
        // are applied last and overwrite settings in files closer to the search ceiling.
        //
        // > If multiple EditorConfig files have matching sections, the pairs from the closer
        // > EditorConfig file are read last, so pairs in closer files take precedence.
        for (config, dir) in configs.into_iter().rev() {
            let relative_path = path.strip_prefix(dir).expect("dir is an ancestor of path");

            for section in config.sections {
                if section.glob.is_match(relative_path) {
                    log::info!(
                        "applying EditorConfig from section '{}' in file {:?}",
                        section.glob.glob(),
                        dir.join(".editorconfig")
                    );
                    pairs.extend(section.pairs);
                }
            }
        }

        Self::from_pairs(pairs)
    }

    fn from_pairs(pairs: Pairs) -> Self {
        enum IndentSize {
            Tab,
            Spaces(NonZeroU8),
        }

        // <https://spec.editorconfig.org/#supported-pairs>
        let indent_size = pairs.get("indent_size").and_then(|value| {
            if value.as_ref() == "tab" {
                Some(IndentSize::Tab)
            } else if let Ok(spaces) = value.parse::<NonZeroU8>() {
                Some(IndentSize::Spaces(spaces))
            } else {
                None
            }
        });
        let tab_width = pairs
            .get("tab_width")
            .and_then(|value| value.parse::<NonZeroU8>().ok())
            .or(match indent_size {
                Some(IndentSize::Spaces(spaces)) => Some(spaces),
                _ => None,
            });
        let indent_style = pairs
            .get("indent_style")
            .and_then(|value| match value.as_ref() {
                "tab" => Some(IndentStyle::Tabs),
                "space" => {
                    let spaces = match indent_size {
                        Some(IndentSize::Spaces(spaces)) => spaces.get(),
                        Some(IndentSize::Tab) => tab_width.map(|n| n.get()).unwrap_or(4),
                        None => 4,
                    };
                    Some(IndentStyle::Spaces(spaces.clamp(1, MAX_INDENT)))
                }
                _ => None,
            });
        let line_ending = pairs
            .get("end_of_line")
            .and_then(|value| match value.as_ref() {
                "lf" => Some(LineEnding::LF),
                "crlf" => Some(LineEnding::Crlf),
                #[cfg(feature = "unicode-lines")]
                "cr" => Some(LineEnding::CR),
                _ => None,
            });
        let charset = pairs.get("charset").map(AsRef::as_ref);
        let encoding = charset.and_then(|value| match value {
            "latin1" => Some(encoding_rs::WINDOWS_1252),
            // > `utf-8-bom` is discouraged: UTF-8, and a new file starts with the mark.
            "utf-8" | "utf-8-bom" => Some(encoding_rs::UTF_8),
            "utf-16le" => Some(encoding_rs::UTF_16LE),
            "utf-16be" => Some(encoding_rs::UTF_16BE),
            _ => None,
        });
        let byte_order_mark = match charset {
            Some("utf-8-bom") => Some(true),
            Some("utf-8") => Some(false),
            _ => None,
        };
        let indent_width = match indent_size {
            Some(IndentSize::Spaces(spaces)) => NonZeroU8::new(spaces.get().clamp(1, MAX_INDENT)),
            _ => None,
        };
        let trim_trailing_whitespace =
            pairs
                .get("trim_trailing_whitespace")
                .and_then(|value| match value.as_ref() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                });
        let insert_final_newline = pairs
            .get("insert_final_newline")
            .and_then(|value| match value.as_ref() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        // This option is not in the spec but is supported by some editors.
        // <https://github.com/editorconfig/editorconfig/wiki/EditorConfig-Properties#max_line_length>
        let max_line_length = pairs
            .get("max_line_length")
            .and_then(|value| value.parse::<NonZeroU16>().ok());

        Self {
            indent_style,
            indent_width,
            tab_width,
            line_ending,
            encoding,
            byte_order_mark,
            trim_trailing_whitespace,
            insert_final_newline,
            max_line_length,
        }
    }
}

type Pairs = HashMap<Box<str>, Box<str>>;

#[derive(Debug)]
struct Section {
    glob: GlobMatcher,
    pairs: Pairs,
}

#[derive(Debug, Default)]
struct Ini {
    pairs: Pairs,
    sections: Vec<Section>,
}

impl Ini {
    /// The pairs and sections of `source` (the file `file`, for the log). A section whose glob
    /// does not compile is skipped, with its pairs.
    fn parse(source: &str, file: &Path) -> Self {
        // <https://spec.editorconfig.org/#file-format>
        let mut ini = Ini::default();
        // Whether the pairs go to a section that was skipped.
        let mut skipping = false;
        // > EditorConfig files are in an INI-like file format. To read an EditorConfig file, take
        // > one line at a time, from beginning to end. For each line:
        for full_line in source.lines() {
            // > 1. Remove all leading and trailing whitespace.
            let line = full_line.trim();
            // > 2. Process the remaining text as specified for its type below.
            // > The types of lines are:
            // > * Blank: contains nothing. Blank lines are ignored.
            if line.is_empty() {
                continue;
            }
            // > * Comment: starts with a ';' or '#'. Comment lines are ignored.
            if line.starts_with([';', '#']) {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                // > * Section Header: starts with a `[` and ends with a `]`. These lines define
                // >   globs...
                match section_glob(section) {
                    Ok(glob) => {
                        skipping = false;
                        ini.sections.push(Section {
                            glob: glob.compile_matcher(),
                            pairs: Pairs::new(),
                        });
                    }
                    Err(err) => {
                        skipping = true;
                        log::warn!(
                            "Ignoring the section [{section}] of the EditorConfig file {file:?}: {err}"
                        );
                    }
                }
            } else if let Some((key, value)) = line.split_once('=') {
                // > * Key-Value Pair (or Pair): contains a key and a value, separated by an `=`.
                // >     * Key: The part before the first `=` on the line.
                // >     * Value: The part, if any, after the first `=` on the line.
                // >     * Keys and values are trimmed of leading and trailing whitespace, but
                // >       include any whitespace that is between non-whitespace characters.
                // >     * If a value is not provided, then the value is an empty string.
                if skipping {
                    continue;
                }
                let key = key.trim().to_lowercase().into_boxed_str();
                let value = value.trim().to_lowercase().into_boxed_str();
                if let Some(section) = ini.sections.last_mut() {
                    section.pairs.insert(key, value);
                } else {
                    ini.pairs.insert(key, value);
                }
            }
        }
        ini
    }
}

/// The glob of a section header (`section`, without its brackets).
fn section_glob(section: &str) -> Result<globset::Glob, globset::Error> {
    // <https://spec.editorconfig.org/#glob-expressions>
    // We need to modify the glob string slightly since EditorConfig's glob flavor
    // doesn't match `globset`'s exactly. `globset` only allows '**' at the beginning
    // or end of a glob or between two '/'s. (This replacement is not very fancy but
    // should cover most practical cases.)
    let mut glob_str = expand_number_ranges(&section.replace("**.", "**/*."));
    if !is_glob_relative(section) {
        glob_str.insert_str(0, "**/");
    }
    GlobBuilder::new(&glob_str)
        .literal_separator(true)
        .backslash_escape(true)
        .empty_alternates(true)
        .build()
}

/// The most numbers a range (`{1..3}`) is expanded to; a larger one stays as it is (and
/// matches only its literal text).
const MAX_RANGE: i64 = 1024;

/// `glob` with its number ranges (`{num1..num2}`, which `globset` does not know) written as
/// the alternatives they match: `{1..3}` is `{1,2,3}`.
///
/// > `{num1..num2}`: Matches any integer numbers between num1 and num2, where num1 and num2
/// > can be either positive or negative
fn expand_number_ranges(glob: &str) -> String {
    let mut expanded = String::with_capacity(glob.len());
    let mut rest = glob;
    while let Some(open) = rest.find('{') {
        // an escaped brace is not a range
        let escaped = rest[..open].ends_with('\\');
        expanded.push_str(&rest[..open]);
        rest = &rest[open..];
        let range = rest[1..].find('}').and_then(|close| {
            let (from, to) = rest[1..1 + close].split_once("..")?;
            let (from, to) = (from.parse::<i64>().ok()?, to.parse::<i64>().ok()?);
            let (low, high) = (from.min(to), from.max(to));
            (high - low < MAX_RANGE).then_some((low, high, close + 2))
        });
        match range.filter(|_| !escaped) {
            Some((low, high, length)) => {
                let numbers: Vec<String> = (low..=high).map(|n| n.to_string()).collect();
                expanded.push('{');
                expanded.push_str(&numbers.join(","));
                expanded.push('}');
                rest = &rest[length..];
            }
            None => {
                expanded.push('{');
                rest = &rest[1..];
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// Determines whether a glob is relative to the directory of the config file.
fn is_glob_relative(source: &str) -> bool {
    // > If the glob contains a path separator (a `/` not inside square brackets), then the
    // > glob is relative to the directory level of the particular `.editorconfig` file itself.
    let mut idx = 0;
    while let Some(open) = source[idx..].find('[').map(|open| idx + open) {
        if source[..open].contains('/') {
            return true;
        }
        idx = source[open..]
            .find(']')
            .map_or(source.len(), |close| idx + close);
    }
    source[idx..].contains('/')
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn is_glob_relative_test() {
        assert!(is_glob_relative("subdir/*.c"));
        assert!(!is_glob_relative("*.txt"));
        assert!(!is_glob_relative("[a/b].c"));
    }

    fn editor_config(path: impl AsRef<Path>, source: &str) -> EditorConfig {
        let path = path.as_ref();
        let ini = Ini::parse(source, Path::new(".editorconfig"));
        let pairs = ini
            .sections
            .into_iter()
            .filter(|section| section.glob.is_match(path))
            .fold(Pairs::new(), |mut acc, section| {
                acc.extend(section.pairs);
                acc
            });
        EditorConfig::from_pairs(pairs)
    }

    #[test]
    fn parse_test() {
        let source = r#"
        [*]
        indent_style = space

        [Makefile]
        indent_style = tab

        [docs/**.txt]
        insert_final_newline = true
        "#;

        assert_eq!(
            editor_config("a.txt", source),
            EditorConfig {
                indent_style: Some(IndentStyle::Spaces(4)),
                ..Default::default()
            }
        );
        assert_eq!(
            editor_config("pkg/Makefile", source),
            EditorConfig {
                indent_style: Some(IndentStyle::Tabs),
                ..Default::default()
            }
        );
        assert_eq!(
            editor_config("docs/config/editor.txt", source),
            EditorConfig {
                indent_style: Some(IndentStyle::Spaces(4)),
                insert_final_newline: Some(true),
                ..Default::default()
            }
        );
    }

    #[test]
    fn empty_alternate_brace_group_test() {
        // A glob with a brace group followed by a brace group containing an empty
        // alternate (e.g. `{,.foo}`) must apply to names matching the empty branch.
        // <https://github.com/helix-editor/helix/issues/15883>
        let source = r#"
        [*]
        indent_size = 4

        [*.{nix,yml}{,.foo}]
        indent_size = 2
        "#;

        // The empty alternate branch must match (`flake.nix`, `config.yml`).
        assert_eq!(
            editor_config("flake.nix", source),
            EditorConfig {
                indent_style: None,
                indent_width: NonZeroU8::new(2),
                tab_width: NonZeroU8::new(2),
                ..Default::default()
            }
        );
        assert_eq!(
            editor_config("config.yml", source),
            EditorConfig {
                indent_style: None,
                indent_width: NonZeroU8::new(2),
                tab_width: NonZeroU8::new(2),
                ..Default::default()
            }
        );
        // The non-empty alternate branch must also match (`flake.nix.foo`).
        assert_eq!(
            editor_config("flake.nix.foo", source),
            EditorConfig {
                indent_style: None,
                indent_width: NonZeroU8::new(2),
                tab_width: NonZeroU8::new(2),
                ..Default::default()
            }
        );
        // A non-matching extension keeps the catch-all `[*]` value.
        assert_eq!(
            editor_config("main.rs", source),
            EditorConfig {
                indent_style: None,
                indent_width: NonZeroU8::new(4),
                tab_width: NonZeroU8::new(4),
                ..Default::default()
            }
        );
    }

    #[test]
    fn number_ranges_match_the_numbers_between() {
        let expanded: String = expand_number_ranges("file{1..3}.txt");
        assert_eq!(expanded, "file{1,2,3}.txt");
        assert_eq!(expand_number_ranges("x{-1..1}"), "x{-1,0,1}");
        assert_eq!(expand_number_ranges("x{3..1}"), "x{1,2,3}");
        // not a range, an escaped brace, a huge range: as they are
        assert_eq!(expand_number_ranges("*.{rs,toml}"), "*.{rs,toml}");
        assert_eq!(expand_number_ranges("a\\{1..2}"), "a\\{1..2}");
        assert_eq!(expand_number_ranges("a{0..99999}"), "a{0..99999}");

        let source = r#"
        [test{1..10}.rs]
        indent_style = tab
        "#;
        let tabs = EditorConfig {
            indent_style: Some(IndentStyle::Tabs),
            ..Default::default()
        };
        assert_eq!(editor_config("test7.rs", source), tabs);
        assert_eq!(editor_config("src/test10.rs", source), tabs);
        assert_eq!(editor_config("test11.rs", source), EditorConfig::default());
        assert_eq!(editor_config("test0.rs", source), EditorConfig::default());
    }

    #[test]
    fn a_section_that_does_not_compile_is_skipped_alone() {
        // `[a{b]` has an unclosed brace: its pairs go with it, the others stay
        let source = r#"
        root = true
        [*]
        insert_final_newline = true
        [a{b]
        indent_style = tab
        [*.rs]
        trim_trailing_whitespace = true
        "#;
        let ini = Ini::parse(source, Path::new(".editorconfig"));
        let root: Option<&str> = ini.pairs.get("root").map(AsRef::as_ref);
        assert_eq!(root, Some("true"));
        assert_eq!(ini.sections.len(), 2);
        assert_eq!(
            editor_config("main.rs", source),
            EditorConfig {
                insert_final_newline: Some(true),
                trim_trailing_whitespace: Some(true),
                ..Default::default()
            }
        );
    }

    #[test]
    fn indent_size_charset_and_unset() {
        let config = editor_config("a.py", "[*]\nindent_size = 2\n");
        let width: Option<NonZeroU8> = config.indent_width;
        assert_eq!(width, NonZeroU8::new(2));
        assert_eq!(config.indent_style, None);
        // `indent_size = tab`: the width of a tab, no width of spaces
        let config = editor_config("a.py", "[*]\nindent_size = tab\ntab_width = 8\n");
        assert_eq!(config.indent_width, None);
        assert_eq!(config.tab_width, NonZeroU8::new(8));

        let config = editor_config("a.txt", "[*]\ncharset = utf-8-bom\n");
        assert_eq!(config.encoding, Some(encoding_rs::UTF_8));
        let bom: Option<bool> = config.byte_order_mark;
        assert_eq!(bom, Some(true));
        let config = editor_config("a.txt", "[*]\ncharset = UTF-8\n");
        assert_eq!(
            (config.encoding, config.byte_order_mark),
            (Some(encoding_rs::UTF_8), Some(false))
        );

        // `unset` (in any case) undoes what a section before set
        let source = "[*]\nindent_style = tab\nend_of_line = crlf\n[*.md]\nindent_style = UNSET\n";
        let config = editor_config("a.md", source);
        assert_eq!(config.indent_style, None);
        assert_eq!(config.line_ending, Some(LineEnding::Crlf));
    }

    #[test]
    fn the_closest_file_wins_and_root_stops_the_search() {
        let dir = std::env::temp_dir().join(format!("helix-editorconfig-{}", std::process::id()));
        let project = dir.join("project");
        let sub = project.join("sub");
        fs::create_dir_all(&sub).unwrap();
        // above the root: never read
        fs::write(dir.join(".editorconfig"), "[*]\nmax_line_length = 10\n").unwrap();
        fs::write(
            project.join(".editorconfig"),
            "root = true\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\n",
        )
        .unwrap();
        fs::write(sub.join(".editorconfig"), "[*.go]\nindent_style = tab\n").unwrap();

        let go = EditorConfig::find(&sub.join("main.go"));
        assert_eq!(go.indent_style, Some(IndentStyle::Tabs));
        assert_eq!(go.line_ending, Some(LineEnding::LF));
        assert_eq!(go.max_line_length, None);
        let rs = EditorConfig::find(&sub.join("main.rs"));
        assert_eq!(rs.indent_style, Some(IndentStyle::Spaces(4)));
        fs::remove_dir_all(&dir).unwrap();
    }
}
