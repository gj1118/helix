//! Task-tag pickers: `todos` (the current file) and `workspace_todos`
//! (the workspace). They list the task tags — `TODO:`, `FIXME(user):`, bare
//! keyword mentions without a colon, ... — found in comments via the tree-sitter
//! tags query of the `comment` grammar, like the highlighting does: they are
//! syntax-aware, not a text search.
//!
//! Like the diagnostics pickers (see [`super::lsp::diagnostics_picker`]) the
//! pickers are live: the tags of open documents are re-collected when their
//! documents change, and the workspace picker keeps the tags it scanned from
//! disk between refreshes.

use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use helix_core::{
    syntax::{Loader, QueryMatchIterEvent, Syntax},
    RopeSlice, Selection,
};
use helix_stdx::path;
use helix_view::{
    align_view,
    document::SCRATCH_BUFFER_NAME,
    editor::{Action, FilePickerConfig},
    icons::ICONS,
    theme::Style,
    Align, Document, DocumentId, Editor, Theme,
};
use tui::text::Span;

use crate::{
    filter_picker_entry,
    job::Callback,
    ui::{overlay::overlaid, picker::PathOrId, Picker, PickerColumn},
};

use super::{
    lsp::{MAX_WORKSPACE_FILES, MAX_WORKSPACE_FILE_BYTES},
    push_jump,
    syntax::syntax_for_path,
    Context,
};

/// The language that parses comments (its injected layers in a document are
/// what gets scanned).
const TAG_LANGUAGE: &str = "comment";

/// At most this many characters of a tag's line are kept for the message
/// column. A minified file (whose one "line" is megabytes) would otherwise
/// freeze the picker's matcher.
const MAX_TAG_TEXT: usize = 200;

/// The comment tag keywords, in the same order and with the same aliases as in
/// `runtime/queries/comment/highlights.scm`: per canonical key its severity
/// and the tags that resolve to it (the aliases and the key). The comment
/// grammar only accepts uppercase tags.
const TAG_KEYWORDS: &[(TagSeverity, &str, &[&str])] = &[
    (TagSeverity::Error, "broken", &["BROKEN"]),
    (TagSeverity::Error, "bug", &["BUG"]),
    (TagSeverity::Error, "compliance", &["COMPLIANCE"]),
    (TagSeverity::Error, "debug", &["DEBUG"]),
    (TagSeverity::Error, "fail", &["FAIL", "FAILED"]),
    (TagSeverity::Error, "fix", &["FIX", "FIXIT"]),
    (TagSeverity::Error, "fixme", &["FIXME"]),
    (TagSeverity::Error, "invariant", &["INVARIANT"]),
    (TagSeverity::Error, "issue", &["ISSUE"]),
    (TagSeverity::Error, "panic", &["PANIC"]),
    (TagSeverity::Error, "safety", &["SAFETY"]),
    (TagSeverity::Error, "security", &["SECURITY"]),
    (TagSeverity::Error, "xxx", &["XXX"]),
    (TagSeverity::Warning, "hack", &["HACK", "WORKAROUND"]),
    (TagSeverity::Warning, "temp", &["TEMP"]),
    (TagSeverity::Warning, "test", &["TEST"]),
    (TagSeverity::Warning, "untested", &["UNTESTED"]),
    (TagSeverity::Warning, "warn", &["WARN", "WARNING"]),
    (TagSeverity::Info, "info", &["INFO"]),
    (TagSeverity::Info, "note", &["NOTE", "NOTES"]),
    (TagSeverity::Info, "optimize", &["OPTIMISE", "OPTIMIZE"]),
    (TagSeverity::Info, "perf", &["PERF", "PERFORMANCE"]),
    (TagSeverity::Info, "question", &["ASK", "QUESTION"]),
    (TagSeverity::Info, "refactor", &["REFACTOR"]),
    (TagSeverity::Info, "review", &["CR", "PR", "REVIEW"]),
    (TagSeverity::Info, "todo", &["TO-DO", "TODO"]),
    (TagSeverity::Info, "undone", &["UNDONE"]),
    (TagSeverity::Hint, "done", &["DONE"]),
    (TagSeverity::Hint, "hint", &["HINT"]),
    (TagSeverity::Hint, "mark", &["BOOKMARK", "MARK", "MARKER"]),
    (TagSeverity::Hint, "passed", &["PASSED"]),
    (TagSeverity::Hint, "stub", &["MOCK", "STUB"]),
    (TagSeverity::Hint, "tip", &["TIP"]),
];

/// The severity of a task tag: it orders the list and colors the keywords,
/// through the theme's `<severity>.<keyword>` scopes (with fallback to the
/// severity scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl TagSeverity {
    /// The severity scope the keyword styles fall back to. These are the same
    /// scopes the severity capture names use.
    fn key(self) -> &'static str {
        match self {
            TagSeverity::Error => "error",
            TagSeverity::Warning => "warning",
            TagSeverity::Info => "info",
            TagSeverity::Hint => "hint",
        }
    }
}

/// How the tag and user columns are styled. The styles resolve at picker
/// construction, like the diagnostics picker's.
struct TagStyles {
    /// Per `TAG_KEYWORDS` entry: the theme's `<severity>.<keyword>` style
    /// (which falls back to the theme's severity style via `Theme::get`).
    keywords: Box<[Style]>,
    user: Style,
}

impl TagStyles {
    fn load(theme: &Theme) -> Self {
        Self {
            keywords: TAG_KEYWORDS
                .iter()
                .map(|(severity, keyword, _)| theme.get(&format!("{}.{}", severity.key(), keyword)))
                .collect(),
            // The in-buffer highlighting styles the user mention as @constant.
            user: theme.get("constant"),
        }
    }
}

/// Where a tag was found, for previewing and jumping.
#[derive(Clone, Debug)]
enum TagSource {
    /// An open document. `path` is none for scratch buffers.
    Doc {
        id: DocumentId,
        path: Option<Arc<Path>>,
    },
    /// A file on disk that is not open (at the time of the scan or refresh).
    Path(Arc<Path>),
}

impl TagSource {
    /// The path when there is one. Scratch buffers have none.
    fn path(&self) -> Option<&Path> {
        let path: Option<&Arc<Path>> = match self {
            TagSource::Doc { path, .. } => path.as_ref(),
            TagSource::Path(path) => Some(path),
        };
        path.map(Arc::as_ref)
    }

    /// The `path` column of the workspace picker: the truncated path, or what
    /// a scratch buffer is called.
    fn display(&self) -> String {
        match self.path() {
            Some(path) => path::get_truncated_path(path).to_string_lossy().to_string(),
            None => SCRATCH_BUFFER_NAME.to_owned(),
        }
    }

    fn path_or_id(&self) -> Option<PathOrId<'_>> {
        match self {
            TagSource::Doc { id, .. } => Some(PathOrId::Id(*id)),
            TagSource::Path(path) => Some(PathOrId::Path(path)),
        }
    }
}

/// One task tag found in a comment.
#[derive(Clone, Debug)]
struct CommentTag {
    source: TagSource,
    /// The keyword's index in [`TAG_KEYWORDS`].
    keyword: u8,
    /// The `(user)` of `TODO(user):`, if any.
    user: Option<String>,
    /// Character index range of the keyword in the document's text.
    start: usize,
    end: usize,
    /// The lines of the keyword, for the preview.
    start_line: usize,
    end_line: usize,
    /// The line the tag is on, trimmed (for the `message` column).
    text: String,
}

impl CommentTag {
    fn severity(&self) -> TagSeverity {
        TAG_KEYWORDS[self.keyword as usize].0
    }

    /// The canonical keyword of the tag, for display.
    fn keyword(&self) -> &'static str {
        TAG_KEYWORDS[self.keyword as usize].1
    }

    /// The identity of the tag as used to re-seat the selection across
    /// refreshes.
    fn key(&self) -> String {
        format!(
            "{:?}:{}:{}:{:?}",
            self.source, self.start, self.end, self.user
        )
    }
}

/// Looks up the index of a spelled tag (one of the grammar's keywords) in
/// [`TAG_KEYWORDS`].
fn spelled_keyword(tag: &str) -> Option<u8> {
    TAG_KEYWORDS
        .iter()
        .position(|(_, _, tags)| tags.contains(&tag))
        .map(|index| index as u8)
}

/// Orders a list of tags: by severity (like the diagnostics pickers), then
/// file, then position.
fn sort_tags(tags: &mut [CommentTag]) {
    tags.sort_by(|a, b| {
        a.severity()
            .cmp(&b.severity())
            .then_with(|| a.source.path().cmp(&b.source.path()))
            .then_with(|| a.start.cmp(&b.start))
            .then_with(|| a.end.cmp(&b.end))
            .then_with(|| a.user.cmp(&b.user))
    });
}

/// Builds the picker over a batch of tags. The workspace variant adds the file
/// column (`show_source_path`); the current file's tags all live in the same
/// file.
fn tag_picker(
    editor: &Editor,
    tags: Vec<CommentTag>,
    show_source_path: bool,
) -> Picker<CommentTag, TagStyles> {
    let styles = TagStyles::load(&editor.theme);

    let mut columns = vec![
        PickerColumn::new("tag", |tag: &CommentTag, styles: &TagStyles| {
            let icons = ICONS.load();
            let icon = match tag.severity() {
                TagSeverity::Error => icons.diagnostic().error(),
                TagSeverity::Warning => icons.diagnostic().warning(),
                TagSeverity::Info => icons.diagnostic().info(),
                TagSeverity::Hint => icons.diagnostic().hint(),
            };
            Span::styled(
                format!("{} {}", icon, tag.keyword().to_uppercase()),
                styles.keywords[tag.keyword as usize],
            )
            .into()
        }),
        PickerColumn::new("message", |tag: &CommentTag, _| tag.text.as_str().into()),
        PickerColumn::new("user", |tag: &CommentTag, styles: &TagStyles| {
            Span::styled(tag.user.as_deref().unwrap_or(""), styles.user).into()
        }),
    ];
    let primary_column = 1; // message

    if show_source_path {
        columns.push(PickerColumn::new("path", |tag: &CommentTag, _| {
            tag.source.display().into()
        }));
    }

    Picker::new(
        columns,
        primary_column,
        tags,
        styles,
        move |cx, tag, action| jump_to_tag(cx.editor, tag, action),
    )
    .with_preview(|_editor, tag| {
        Some((
            tag.source.path_or_id()?,
            Some((tag.start_line, tag.end_line)),
        ))
    })
    .truncate_start(false)
}

/// Jumps to the tag's keyword: the document it is open in (also split views)
/// or the file it lives in.
fn jump_to_tag(editor: &mut Editor, tag: &CommentTag, action: Action) {
    let (view, doc) = current!(editor);
    push_jump(view, doc);

    let doc_id = match &tag.source {
        TagSource::Doc { id, .. } => *id,
        TagSource::Path(path) => match editor.open(path.as_ref(), action) {
            Ok(id) => id,
            Err(err) => {
                editor.set_error(format!("Failed to open file '{path:?}': {err}"));
                return;
            }
        },
    };
    let doc = doc_mut!(editor, &doc_id);
    let view = view_mut!(editor);
    let len_chars = doc.text().len_chars();
    if tag.start >= len_chars || tag.end > len_chars {
        editor.set_error(
            "The location you jumped to does not exist anymore because the file has changed.",
        );
        return;
    }
    doc.set_selection(view.id, Selection::single(tag.start, tag.end));
    if action.align_view(view, doc.id()) {
        align_view(doc, view, Align::Center)
    }
}

/// Collects the task tags of open documents: `only` (or every open document
/// when it is none). Live: the document's current text and syntax tree.
fn scan_open_docs_tags(editor: &Editor, only: Option<DocumentId>) -> Vec<CommentTag> {
    let loader = editor.syn_loader.load();
    let mut tags = Vec::new();
    for doc in editor.documents() {
        if only.is_some_and(|only| doc.id() != only) {
            continue;
        }
        let Some(syntax) = doc.syntax() else {
            continue;
        };
        let source = TagSource::Doc {
            id: doc.id(),
            path: doc.path().map(|path| path.into()),
        };
        tags.extend(scan_syntax_tags(
            syntax,
            doc.text().slice(..),
            &loader,
            source,
        ));
    }
    tags
}

/// Collects the task tags of a parsed document, from its (injected) comment
/// layers, using the comment grammar's tags query.
fn scan_syntax_tags<'a>(
    syntax: &'a Syntax,
    text: RopeSlice<'a>,
    loader: &'a Loader,
    source: TagSource,
) -> Vec<CommentTag> {
    let Some(comment) = loader.language_for_name(TAG_LANGUAGE) else {
        return Vec::new();
    };
    let Some(comment_query) = loader.tag_query(comment) else {
        return Vec::new();
    };
    let comment_query = &comment_query.query;
    let tag_capture = comment_query.get_capture("comment.tag");
    let user_capture = comment_query.get_capture("comment.user");

    let mut tags = Vec::new();
    // `Syntax::tags` walks every language's tags query over the document (its
    // injected comment layers included); only matches of the comment language
    // are ours.
    let mut tags_iter = syntax.tags(text, loader, ..);

    while let Some(event) = tags_iter.next() {
        let QueryMatchIterEvent::Match(matched) = event else {
            continue;
        };
        if tags_iter.current_language() != comment {
            continue;
        }
        let mut name_range = None::<std::ops::Range<u32>>;
        let mut user_range = None::<std::ops::Range<u32>>;
        for matched_node in matched.nodes.iter() {
            if Some(matched_node.capture) == tag_capture {
                name_range = Some(matched_node.node.byte_range());
            } else if Some(matched_node.capture) == user_capture {
                user_range = Some(matched_node.node.byte_range());
            }
        }
        let Some(name_range) = name_range else {
            continue;
        };

        let (start, end) = (
            text.byte_to_char(name_range.start as usize),
            text.byte_to_char(name_range.end as usize),
        );
        let name = text.slice(start..end).to_string();
        // The query only matches spelled keywords, but they still have to be
        // looked up for the canonical key (and its severity).
        let Some(keyword) = spelled_keyword(&name) else {
            continue;
        };

        let user = user_range.map(|user| {
            let (start, end) = (
                text.byte_to_char(user.start as usize),
                text.byte_to_char(user.end as usize),
            );
            text.slice(start..end).to_string()
        });
        let start_line = text.char_to_line(start);
        let end_line = text.char_to_line(end);
        // The line the tag is on, for the `message` column.
        let line = text
            .line(start_line)
            .to_string()
            .trim()
            .chars()
            .take(MAX_TAG_TEXT)
            .collect();

        tags.push(CommentTag {
            source: source.clone(),
            keyword,
            user,
            start,
            end,
            start_line,
            end_line,
            text: line,
        });
    }
    tags
}

/// Hashes what the open documents' task tags depend on: the path, version and
/// whether the syntax tree exists yet, of the open documents (`only` or all of
/// them). This runs on every redraw of a live tag picker, so it hashes no
/// strings. The workspace pickers hash all documents as they also fill in the
/// files of the workspace.
fn open_docs_fingerprint(editor: &Editor, only: Option<DocumentId>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut docs = 0usize;
    for doc in editor.documents() {
        if only.is_some_and(|only| doc.id() != only) {
            continue;
        }
        docs += 1;
        doc.path().is_some().hash(&mut hasher);
        if let Some(path) = doc.path() {
            path.hash(&mut hasher);
        }
        doc.version().hash(&mut hasher);
        doc.syntax().is_some().hash(&mut hasher);
    }
    docs.hash(&mut hasher);
    hasher.finish()
}

/// `todos`: the task tags of the current file.
pub fn todos_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);
    if doc.syntax().is_none() {
        cx.editor
            .set_error("Syntax tree is not available on this buffer");
        return;
    }
    let doc_id = doc.id();
    let tags = scan_open_docs_tags(cx.editor, Some(doc_id));
    let picker = tag_picker(cx.editor, tags, false).with_live_refresh(
        move |editor| open_docs_fingerprint(editor, Some(doc_id)),
        Some(Box::new(move |editor, injector| {
            for tag in scan_open_docs_tags(editor, Some(doc_id)) {
                if injector.push(tag).is_err() {
                    break;
                }
            }
        })),
        CommentTag::key,
    );
    cx.push_layer(Box::new(overlaid(
        picker,
        cx.editor.config().fullscreen_overlay,
    )));
}

/// Set while the workspace is being scanned for task tags.
static WORKSPACE_SCANNING: AtomicBool = AtomicBool::new(false);

/// `workspace-todos`: the task tags across the workspace.
pub fn workspace_todos_picker(cx: &mut Context) {
    if WORKSPACE_SCANNING.swap(true, Ordering::SeqCst) {
        // Another scan is under way: open the picker for what the open
        // documents have (its live part) while it runs.
        let picker = make_workspace_todos_picker(cx.editor, Arc::new(Vec::new()));
        cx.push_layer(Box::new(overlaid(
            picker,
            cx.editor.config().fullscreen_overlay,
        )));
        return;
    }

    let root = doc!(cx.editor).workspace_root().to_path_buf();
    let file_picker_config = cx.editor.config().file_picker.clone();
    let loader = cx.editor.syn_loader.load_full();

    cx.editor.set_status("Scanning workspace for task tags...");

    cx.jobs.callback(async move {
        // Walking the workspace, and reading and parsing its files, is slow:
        // run it on the blocking thread pool.
        let scanned = tokio::task::spawn_blocking(move || {
            scan_workspace_files(root, file_picker_config, &loader)
        })
        .await
        .unwrap_or_else(|_| Arc::new(Vec::new()));

        crate::job::dispatch_blocking(move |editor, compositor| {
            WORKSPACE_SCANNING.store(false, Ordering::SeqCst);
            let picker = make_workspace_todos_picker(editor, scanned);
            compositor.push(Box::new(overlaid(
                picker,
                editor.config().fullscreen_overlay,
            )));
        });
        Ok(Callback::Editor(Box::new(|_| {})))
    });
}

/// Builds the workspace picker: the tags of the files scanned from disk (kept
/// between refreshes) minus those whose file has an open document — covered by
/// the live tags of the open documents.
fn make_workspace_todos_picker(
    editor: &Editor,
    scanned: Arc<Vec<CommentTag>>,
) -> Picker<CommentTag, TagStyles> {
    let tags = merged_workspace_tags(editor, &scanned);
    tag_picker(editor, tags, true)
        .with_live_refresh(
            move |editor| open_docs_fingerprint(editor, None),
            Some(Box::new(move |editor, injector| {
                for tag in merged_workspace_tags(editor, &scanned) {
                    if injector.push(tag).is_err() {
                        break;
                    }
                }
            })),
            CommentTag::key,
        )
        .with_title("Workspace Todos")
}

/// The tags the workspace picker shows: the tags scanned from disk, except
/// where the file has an open document (whose tags, possibly after edits, are
/// collected live instead), plus the tags of every open document.
fn merged_workspace_tags(editor: &Editor, scanned: &[CommentTag]) -> Vec<CommentTag> {
    let open_paths: HashSet<&Path> = editor.documents().filter_map(Document::path).collect();
    let mut tags: Vec<CommentTag> = scanned
        .iter()
        // The document paths are canonical, and so are the scanned ones.
        .filter(|tag| {
            tag.source
                .path()
                .is_none_or(|path| !open_paths.contains(path))
        })
        .cloned()
        .collect();
    tags.extend(scan_open_docs_tags(editor, None));
    sort_tags(&mut tags);
    tags
}

/// Scans the files below `root` for task tags. Slow work: it runs on a
/// blocking thread. Files that are open are scanned too; [`merged_workspace_tags`]
/// replaces their disk tags with the live ones their open document has.
fn scan_workspace_files(
    root: PathBuf,
    config: FilePickerConfig,
    loader: &Loader,
) -> Arc<Vec<CommentTag>> {
    let absolute_root = root.canonicalize().unwrap_or_else(|_| root.clone());

    // The same files the file picker would show.
    let mut walk_builder = ignore::WalkBuilder::new(&root);
    walk_builder
        .hidden(config.hidden)
        .parents(config.parents)
        .ignore(config.ignore)
        .follow_links(config.follow_symlinks)
        .git_ignore(config.git_ignore)
        .git_global(config.git_global)
        .git_exclude(config.git_exclude)
        .max_depth(config.max_depth)
        .filter_entry(move |entry| {
            filter_picker_entry(entry, &absolute_root, config.deduplicate_links)
        });
    walk_builder.add_custom_ignore_filename(helix_loader::config_dir().join("ignore"));
    walk_builder.add_custom_ignore_filename(".helix/ignore");

    let mut files = Vec::new();
    for entry in walk_builder.build() {
        if files.len() >= MAX_WORKSPACE_FILES {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
            continue;
        }
        // Document paths are canonical; the merge compares against them.
        files.push(path::canonicalize(&entry.into_path()));
    }

    let mut tags = Vec::new();
    for path in files {
        let Some((rope, syntax)) = syntax_for_path(&path, loader) else {
            continue;
        };
        let source = TagSource::Path(Arc::from(path.as_path()));
        tags.extend(scan_syntax_tags(&syntax, rope.slice(..), loader, source));
    }
    sort_tags(&mut tags);
    Arc::new(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyword table agrees with the highlighting: one row per canonical
    /// key (the 33 keys of `runtime/queries/comment/highlights.scm`), alias
    /// spellings appearing exactly once across the table, and all-CAPS tags as
    /// the comment grammar produces them.
    #[test]
    fn tag_keywords_cover_the_highlighting() {
        assert_eq!(TAG_KEYWORDS.len(), 33);
        let mut tags = Vec::new();
        for row in TAG_KEYWORDS.iter() {
            let (severity, _, spellings) = row;
            assert!(matches!(
                severity,
                TagSeverity::Error | TagSeverity::Warning | TagSeverity::Info | TagSeverity::Hint
            ));
            assert!(!spellings.is_empty());
            for spelling in spellings.iter() {
                assert!(
                    spelling
                        .chars()
                        .all(|ch| ch.is_ascii_alphabetic() || ch == '-'),
                    "tag {spelling} has characters the grammar's name tokens do not"
                );
                tags.push(*spelling);
            }
        }
        tags.sort_unstable();
        let total = tags.len();
        tags.dedup();
        assert_eq!(total, tags.len(), "a spelling maps to two keywords");
        assert_eq!(total, 47);
    }

    /// The walk-up the pickers do: a spelled tag resolves to its canonical
    /// key (aliases included).
    #[test]
    fn spelled_keyword_resolves_aliases() {
        let expect = |spelling| spelled_keyword(spelling).unwrap();
        assert_eq!(TAG_KEYWORDS[expect("TODO") as usize].1, "todo");
        assert_eq!(TAG_KEYWORDS[expect("TO-DO") as usize].1, "todo");
        assert_eq!(expect("TODO"), expect("TO-DO"));
        assert_eq!(TAG_KEYWORDS[expect("FIXIT") as usize].1, "fix");
        assert_eq!(TAG_KEYWORDS[expect("WORKAROUND") as usize].1, "hack");
        assert!(spelled_keyword("XYZZY").is_none());
        assert!(spelled_keyword("todo").is_none());
    }
}
