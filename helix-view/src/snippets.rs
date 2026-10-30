//! User-defined snippets.
//!
//! Snippets are written in TOML as `[[snippet]]` tables and are loaded from
//!
//! * `snippets.toml` and every `.toml` file below the `snippets/` directory of the user config
//!   directory, and
//! * the same layout inside `.helix/` of a *trusted* workspace. A workspace snippet replaces a
//!   user snippet with the same name.
//!
//! The body uses the LSP/TextMate snippet syntax that Helix already understands for language
//! server completions, which includes the variables resolved by [`variables`].

pub mod variables;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use serde::Deserialize;

use helix_core::snippets::Snippet;

/// A snippet as defined by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetDef {
    /// Unique name of the snippet.
    pub name: String,
    /// The snippet body in LSP snippet syntax.
    pub body: String,
    /// Words that trigger the snippet in completion.
    pub prefixes: Vec<String>,
    /// Languages the snippet is offered for. Empty means every language.
    pub scopes: Vec<String>,
    pub description: Option<String>,
    /// The file the snippet was defined in.
    pub source: PathBuf,
    /// Whether the snippet comes from a workspace (`.helix/`) instead of the user config.
    pub workspace: bool,
}

impl SnippetDef {
    /// Whether the snippet should be offered in a document of the given language.
    pub fn applies_to(&self, language: Option<&str>) -> bool {
        self.scopes.is_empty()
            || language.is_some_and(|language| {
                self.scopes
                    .iter()
                    .any(|scope| scope.eq_ignore_ascii_case(language))
            })
    }

    /// Parse the body. Bodies are validated when loaded so this only fails for hand built
    /// definitions.
    pub fn parse(&self) -> Result<Snippet> {
        Snippet::parse(&self.body)
            .map_err(|err| anyhow::anyhow!("invalid snippet \"{}\": {err}", self.name))
    }
}

/// All snippets that are currently loaded.
#[derive(Debug, Default, Clone)]
pub struct Snippets {
    by_name: BTreeMap<String, Arc<SnippetDef>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            StringOrList::One(one) => vec![one],
            StringOrList::Many(many) => many,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSnippet {
    name: String,
    body: StringOrList,
    prefix: Option<StringOrList>,
    scope: Option<StringOrList>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    snippet: Vec<RawSnippet>,
}

impl RawSnippet {
    fn validate(self, source: &Path, workspace: bool) -> Result<SnippetDef> {
        let name = self.name;
        if name.trim().is_empty() {
            bail!("a snippet has an empty name");
        }
        let body = match self.body {
            StringOrList::One(body) => body,
            StringOrList::Many(lines) => lines.join("\n"),
        };
        if body.is_empty() {
            bail!("snippet \"{name}\" has an empty body");
        }
        Snippet::parse(&body).map_err(|err| anyhow::anyhow!("snippet \"{name}\": {err}"))?;

        let prefixes = self.prefix.map(StringOrList::into_vec).unwrap_or_default();
        if let Some(bad) = prefixes
            .iter()
            .find(|prefix| prefix.is_empty() || prefix.chars().any(char::is_whitespace))
        {
            bail!("snippet \"{name}\" has the prefix {bad:?}, prefixes must not be empty or contain whitespace");
        }
        let scopes = self.scope.map(StringOrList::into_vec).unwrap_or_default();
        if scopes.iter().any(|scope| scope.trim().is_empty()) {
            bail!("snippet \"{name}\" has an empty scope");
        }

        Ok(SnippetDef {
            name,
            body,
            prefixes,
            scopes,
            description: self.description,
            source: source.to_owned(),
            workspace,
        })
    }
}

/// Parse the contents of one snippet file.
pub fn parse_file(source: &Path, contents: &str, workspace: bool) -> Result<Vec<SnippetDef>> {
    let raw: RawFile = toml::from_str(contents).map_err(|err| {
        // toml's error already contains the offending line, keep it as is.
        anyhow::anyhow!("{}: {err}", source.display())
    })?;
    raw.snippet
        .into_iter()
        .map(|snippet| {
            snippet
                .validate(source, workspace)
                .with_context(|| source.display().to_string())
        })
        .collect()
}

/// The `.toml` files below `dir`, sorted. Symlinks to files are followed, symlinked
/// directories are not so that a loop can't hang the editor.
fn toml_files_in(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            toml_files_in(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "toml")
            && std::fs::metadata(&path).is_ok_and(|meta| meta.is_file())
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Load the snippets under `base`: `base/snippets.toml` and `base/snippets/**/*.toml`.
/// Names must be unique across all of the files.
fn load_dir(base: &Path, workspace: bool) -> Result<BTreeMap<String, SnippetDef>> {
    let mut files = Vec::new();
    let single = base.join("snippets.toml");
    if single.is_file() {
        files.push(single);
    }
    let dir = base.join("snippets");
    if dir.is_dir() {
        toml_files_in(&dir, &mut files)?;
    }

    let mut snippets: BTreeMap<String, SnippetDef> = BTreeMap::new();
    for file in files {
        let contents = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        for snippet in parse_file(&file, &contents, workspace)? {
            if let Some(existing) = snippets.get(&snippet.name) {
                let mut message = format!("snippet \"{}\" is defined twice", snippet.name);
                if existing.source == snippet.source {
                    write!(message, " in {}", snippet.source.display()).unwrap();
                } else {
                    write!(
                        message,
                        ": in {} and in {}",
                        existing.source.display(),
                        snippet.source.display()
                    )
                    .unwrap();
                }
                bail!(message);
            }
            snippets.insert(snippet.name.clone(), snippet);
        }
    }
    Ok(snippets)
}

impl Snippets {
    /// Load the user snippets from `user_dir` and, if given, the snippets of a trusted
    /// workspace from `<workspace>/.helix`.
    ///
    /// Any error (unreadable file, invalid TOML, bad snippet body, duplicate name) fails the whole
    /// load so callers can keep using the previous set instead of half of a new one.
    pub fn load(user_dir: &Path, workspace: Option<&Path>) -> Result<Self> {
        let mut snippets = load_dir(user_dir, false)?;
        if let Some(workspace) = workspace {
            // Workspace snippets win over user snippets with the same name.
            snippets.extend(load_dir(&workspace.join(".helix"), true)?);
        }
        Ok(Self::from_defs(snippets.into_values()))
    }

    pub fn from_defs(defs: impl IntoIterator<Item = SnippetDef>) -> Self {
        Snippets {
            by_name: defs
                .into_iter()
                .map(|def| (def.name.clone(), Arc::new(def)))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<SnippetDef>> {
        self.by_name.get(name)
    }

    /// All snippets, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<SnippetDef>> {
        self.by_name.values()
    }

    /// The snippets offered for a document of the given language.
    pub fn for_language<'a>(
        &'a self,
        language: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Arc<SnippetDef>> {
        self.iter().filter(move |def| def.applies_to(language))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> Result<Vec<SnippetDef>> {
        parse_file(Path::new("test.toml"), toml, false)
    }

    #[test]
    fn parses_all_forms() {
        let defs = parse(
            r#"
            [[snippet]]
            name = "fn"
            body = ["fn ${1:name}() {", "\t$0", "}"]
            prefix = ["fn", "func"]
            scope = "rust"
            description = "A function"

            [[snippet]]
            name = "date"
            body = "$CURRENT_YEAR-$CURRENT_MONTH-$CURRENT_DATE"
            prefix = "date"
            "#,
        )
        .unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].body, "fn ${1:name}() {\n\t$0\n}");
        assert_eq!(defs[0].prefixes, ["fn", "func"]);
        assert_eq!(defs[0].scopes, ["rust"]);
        assert_eq!(defs[0].description.as_deref(), Some("A function"));
        assert_eq!(defs[1].prefixes, ["date"]);
        assert!(defs[1].scopes.is_empty());
        assert!(defs[1].applies_to(Some("anything")));
        assert!(defs[0].applies_to(Some("Rust")));
        assert!(!defs[0].applies_to(Some("python")));
        assert!(!defs[0].applies_to(None));
    }

    #[test]
    fn rejects_bad_definitions() {
        let err = |toml: &str| format!("{:#}", parse(toml).unwrap_err());
        assert!(err("[[snippet]]\nname = \"a\"\n").contains("body"));
        assert!(err("[[snippet]]\nname = \"\"\nbody = \"x\"\n").contains("empty name"));
        assert!(err("[[snippet]]\nname = \"a\"\nbody = \"\"\n").contains("empty body"));
        assert!(
            err("[[snippet]]\nname = \"a\"\nbody = \"x\"\nprefix = \"a b\"\n")
                .contains("whitespace")
        );
        assert!(err("[[snippet]]\nname = \"a\"\nbody = \"x\"\nbogus = 1\n").contains("bogus"));
        assert!(err("snippet = 3").contains("test.toml"));
    }

    #[test]
    fn loads_directories_with_overrides_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(user.join("snippets/nested")).unwrap();
        std::fs::create_dir_all(workspace.join(".helix/snippets")).unwrap();
        let write = |path: PathBuf, text: &str| std::fs::write(path, text).unwrap();
        write(
            user.join("snippets.toml"),
            "[[snippet]]\nname = \"a\"\nbody = \"user a\"\n",
        );
        write(
            user.join("snippets/nested/b.toml"),
            "[[snippet]]\nname = \"b\"\nbody = \"user b\"\n",
        );
        write(user.join("snippets/ignored.txt"), "not toml {{{");
        write(
            workspace.join(".helix/snippets/x.toml"),
            "[[snippet]]\nname = \"a\"\nbody = \"project a\"\n",
        );

        let user_only = Snippets::load(&user, None).unwrap();
        assert_eq!(user_only.len(), 2);
        assert_eq!(user_only.get("a").unwrap().body, "user a");

        let both = Snippets::load(&user, Some(&workspace)).unwrap();
        assert_eq!(both.len(), 2);
        let a = both.get("a").unwrap();
        assert_eq!(a.body, "project a");
        assert!(a.workspace);
        assert!(!both.get("b").unwrap().workspace);
        assert_eq!(
            both.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );

        // The same name in two user files is an error naming both files.
        write(
            user.join("snippets/nested/c.toml"),
            "[[snippet]]\nname = \"b\"\nbody = \"again\"\n",
        );
        let err = format!("{:#}", Snippets::load(&user, None).unwrap_err());
        assert!(
            err.contains("\"b\"") && err.contains("b.toml") && err.contains("c.toml"),
            "{err}"
        );

        // And so is a file that doesn't parse.
        std::fs::remove_file(user.join("snippets/nested/c.toml")).unwrap();
        write(user.join("snippets/bad.toml"), "[[snippet]\n");
        let err = format!("{:#}", Snippets::load(&user, None).unwrap_err());
        assert!(err.contains("bad.toml"), "{err}");
    }

    #[test]
    fn missing_directories_are_fine() {
        let dir = tempfile::tempdir().unwrap();
        let snippets = Snippets::load(&dir.path().join("nope"), Some(dir.path())).unwrap();
        assert!(snippets.is_empty());
    }
}
