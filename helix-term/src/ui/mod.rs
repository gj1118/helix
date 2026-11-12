mod cmdline_popup;
mod completion;
mod confirm_box;
pub mod diff_view;
mod document;
pub(crate) mod editor;
pub mod explorer;
mod file_explorer;
pub mod gradient_border;
mod image_preview;
mod info;
pub mod logo;
pub mod lsp;
mod markdown;
pub mod menu;
mod notification_popup;
pub mod overlay;
pub mod picker;
pub mod popup;
pub mod prompt;
pub mod quickfix;
mod select;
mod spinner;
mod statusline;
mod text;
mod text_decorations;

use crate::compositor::Compositor;
use crate::filter_picker_entry;
use crate::job::{self, Callback};
use crate::ui::completers::CompletionResult;
pub use cmdline_popup::CmdlinePopup;
pub use completion::Completion;
pub use confirm_box::ConfirmBox;
pub use editor::EditorView;
pub use file_explorer::file_explorer;
use helix_stdx::rope;
use helix_view::theme::Style;
pub use markdown::Markdown;
pub use menu::Menu;
pub use notification_popup::NotificationPopup;
pub use picker::{Column as PickerColumn, FileLocation, Picker};
pub use popup::Popup;
pub use prompt::{Prompt, PromptEvent};
pub use select::Select;
pub use spinner::{ProgressSpinners, Spinner};
pub use text::Text;

use helix_view::editor::CmdlineStyle;
use helix_view::Editor;
use tui::text::{Span, Spans};

use std::path::Path;
use std::{error::Error, path::PathBuf};

struct Utf8PathBuf {
    path: String,
    is_dir: bool,
    is_symlink: bool,
}

impl AsRef<str> for Utf8PathBuf {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

/// The border type for panels, popups and menus, from `border-style`.
pub fn border_type(config: &helix_view::editor::Config) -> tui::widgets::BorderType {
    use helix_view::editor::BorderStyle;
    use tui::widgets::BorderType;
    match config.effective_border_style() {
        BorderStyle::None => BorderType::Hidden,
        // Plain borders hug the outer edge of their cells (see `border_cells_take_background`).
        BorderStyle::Plain => BorderType::Edge,
        BorderStyle::Rounded => BorderType::Rounded,
        BorderStyle::Double => BorderType::Double,
        BorderStyle::Thick => BorderType::Thick,
        BorderStyle::QuadrantInside => BorderType::QuadrantInside,
        BorderStyle::QuadrantOutside => BorderType::QuadrantOutside,
    }
}

/// The color of the border of a picker, panel or popup when `gradient-borders` is off: the same
/// for all of them. It is the foreground of `ui.popup.border`, else of `ui.window`, else of
/// `ui.text`, so that a theme can color it, and it is never the terminal default.
pub fn border_style(theme: &helix_view::Theme) -> helix_view::graphics::Style {
    ["ui.popup.border", "ui.window", "ui.text"]
        .into_iter()
        .find_map(|scope| theme.try_get(scope).and_then(|style| style.fg))
        .map(|fg| helix_view::graphics::Style::default().fg(fg))
        .unwrap_or_default()
}

/// Whether the border cells of a popup are to take the popup's background. A terminal cell
/// has one background, and a border's glyph decides which side of it the popup is on:
/// - plain borders are drawn along the outer edge of their cells (`▏ ▔ ▕ ▁`), and the
///   `quadrant-outside` blocks fill their outer half: the rest of the cell is inside the
///   popup, so it takes the popup's background (no gap inside, nothing beyond the line);
/// - the other styles (rounded, double, thick, `quadrant-inside`, and gradient borders
///   thicker than one cell) run through the middle of the cell or fill its inner half: the
///   cell keeps the background behind the popup, so nothing shows beyond the line.
///
/// Without border glyphs the popup simply covers its whole area.
pub fn border_cells_take_background(config: &helix_view::editor::Config) -> bool {
    use helix_view::editor::BorderStyle;
    if config.borders_hidden() {
        return true;
    }
    if config.gradient_borders.enable {
        // Gradient borders are rounded or square; square and thin is the plain edge.
        return gradient_border::GradientBorder::hugs_the_edge(
            config.gradient_borders.thickness,
            config.border_rounded(),
        );
    }
    matches!(
        config.effective_border_style(),
        BorderStyle::Plain | BorderStyle::QuadrantOutside
    )
}

/// Paints the background of a popup inside its border. With `keep_border_background` the
/// border cells keep the background already behind the popup (for border glyphs through the
/// middle of their cells, see [`border_cells_take_background`]); otherwise the whole area,
/// border cells included, takes `background`.
pub fn clear_bordered(
    surface: &mut tui::buffer::Buffer,
    area: helix_view::graphics::Rect,
    background: Style,
    keep_border_background: bool,
) {
    let area = area.intersection(surface.area);
    if !keep_border_background || area.width < 3 || area.height < 3 {
        surface.clear_with(area, background);
        return;
    }
    let ring = (area.left()..area.right())
        .flat_map(|x| [(x, area.top()), (x, area.bottom() - 1)])
        .chain(
            (area.top() + 1..area.bottom() - 1)
                .flat_map(|y| [(area.left(), y), (area.right() - 1, y)]),
        );
    for (x, y) in ring {
        let cell = &mut surface[(x, y)];
        let behind = cell.bg;
        cell.reset();
        cell.bg = behind;
    }
    surface.clear_with(area.inner(helix_view::graphics::Margin::all(1)), background);
}

/// The border type of the separator lines inside a panel. Hidden borders keep
/// their separators, which would otherwise merge the sections they divide.
pub fn separator_type(config: &helix_view::editor::Config) -> tui::widgets::BorderType {
    match border_type(config) {
        // A separator runs through the content, so it stays a line through the middle of its
        // cells.
        tui::widgets::BorderType::Hidden | tui::widgets::BorderType::Edge => {
            tui::widgets::BorderType::Plain
        }
        other => other,
    }
}

pub fn prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    callback_fn: impl FnMut(&mut crate::compositor::Context, &str, PromptEvent) + 'static,
) {
    let mut prompt = Prompt::new(prompt, history_register, completion_fn, callback_fn);
    // Calculate the initial completion
    prompt.recalculate_completion(cx.editor);
    cx.push_layer(Box::new(prompt));
}

pub fn regex_prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, rope::Regex, PromptEvent) + 'static,
) {
    raw_regex_prompt(
        cx,
        prompt,
        history_register,
        completion_fn,
        move |cx, regex, _, event| fun(cx, regex, event),
    );
}
pub fn raw_regex_prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, rope::Regex, &str, PromptEvent) + 'static,
) {
    let (view, doc) = current!(cx.editor);
    let doc_id = view.doc;
    let view_id = view.id;
    let snapshot = doc.selection(view.id).clone();
    let offset_snapshot = doc.view_offset(view.id);
    let config = cx.editor.config();

    match config.cmdline.style {
        CmdlineStyle::Popup => {
            let cmdline = CmdlinePopup::new(
                prompt,
                history_register,
                completion_fn,
                move |cx: &mut crate::compositor::Context, input: &str, event: PromptEvent| {
                    match event {
                        PromptEvent::Abort => {
                            let doc = doc_mut!(cx.editor, &doc_id);
                            let view = view_mut!(cx.editor, view_id);
                            doc.set_selection(view.id, snapshot.clone());
                            doc.set_view_offset(view.id, offset_snapshot);
                        }
                        PromptEvent::Update | PromptEvent::Validate => {
                            if input.is_empty() {
                                return;
                            }

                            let case_insensitive = if config.search.smart_case {
                                !input.chars().any(char::is_uppercase)
                            } else {
                                false
                            };

                            let is_crlf = doc!(cx.editor).line_ending == helix_core::LineEnding::Crlf;
                            match rope::RegexBuilder::new()
                                .syntax(
                                    rope::Config::new()
                                        .case_insensitive(case_insensitive)
                                        .multi_line(true)
                                        .crlf(is_crlf),
                                )
                                .build(input)
                            {
                                Ok(regex) => {
                                    let doc = doc_mut!(cx.editor, &doc_id);
                                    let view = view_mut!(cx.editor, view_id);

                                    doc.set_selection(view.id, snapshot.clone());

                                    if event == PromptEvent::Validate {
                                        view.push_jump(doc, (doc_id, snapshot.clone()));
                                    }

                                    fun(cx, regex, input, event);

                                    let (view, doc) = current!(cx.editor);
                                    view.ensure_cursor_in_view(doc, config.scrolloff);
                                }
                                Err(err) => {
                                    let doc = doc_mut!(cx.editor, &doc_id);
                                    let view = view_mut!(cx.editor, view_id);
                                    doc.set_selection(view.id, snapshot.clone());
                                    doc.set_view_offset(view.id, offset_snapshot);

                                    if event == PromptEvent::Validate {
                                        let callback = async move {
                                            let call: job::Callback = Callback::EditorCompositor(Box::new(
                                                move |_editor: &mut Editor, compositor: &mut Compositor| {
                                                    let contents = Text::new(format!("{}", err));
                                                    let size = compositor.size();
                                                    let popup = Popup::new("invalid-regex", contents)
                                                        .position(Some(helix_core::Position::new(
                                                            (size.height as usize).saturating_sub(2),
                                                            0,
                                                        )))
                                                        .auto_close(true);
                                                    compositor.replace_or_push("invalid-regex", popup);
                                                },
                                            ));
                                            Ok(call)
                                        };

                                        cx.jobs.callback(callback);
                                    }
                                }
                            }
                        }
                    }
                },
                CmdlineStyle::Popup,
            )
            .with_language("regex", std::sync::Arc::clone(&cx.editor.syn_loader));

            cx.push_layer(Box::new(cmdline));
        }
        CmdlineStyle::Bottom => {
            let mut prompt = Prompt::new(
                prompt,
                history_register,
                completion_fn,
                move |cx: &mut crate::compositor::Context, input: &str, event: PromptEvent| {
                    match event {
                        PromptEvent::Abort => {
                            let doc = doc_mut!(cx.editor, &doc_id);
                            let view = view_mut!(cx.editor, view_id);
                            doc.set_selection(view.id, snapshot.clone());
                            doc.set_view_offset(view.id, offset_snapshot);
                        }
                        PromptEvent::Update | PromptEvent::Validate => {
                            if input.is_empty() {
                                return;
                            }

                            let case_insensitive = if config.search.smart_case {
                                !input.chars().any(char::is_uppercase)
                            } else {
                                false
                            };

                            let is_crlf = doc!(cx.editor).line_ending == helix_core::LineEnding::Crlf;
                            match rope::RegexBuilder::new()
                                .syntax(
                                    rope::Config::new()
                                        .case_insensitive(case_insensitive)
                                        .multi_line(true)
                                        .crlf(is_crlf),
                                )
                                .build(input)
                            {
                                Ok(regex) => {
                                    let doc = doc_mut!(cx.editor, &doc_id);
                                    let view = view_mut!(cx.editor, view_id);

                                    doc.set_selection(view.id, snapshot.clone());

                                    if event == PromptEvent::Validate {
                                        view.push_jump(doc, (doc_id, snapshot.clone()));
                                    }

                                    fun(cx, regex, input, event);

                                    let (view, doc) = current!(cx.editor);
                                    view.ensure_cursor_in_view(doc, config.scrolloff);
                                }
                                Err(err) => {
                                    let doc = doc_mut!(cx.editor, &doc_id);
                                    let view = view_mut!(cx.editor, view_id);
                                    doc.set_selection(view.id, snapshot.clone());
                                    doc.set_view_offset(view.id, offset_snapshot);

                                    if event == PromptEvent::Validate {
                                        let callback = async move {
                                            let call: job::Callback = Callback::EditorCompositor(Box::new(
                                                move |_editor: &mut Editor, compositor: &mut Compositor| {
                                                    let contents = Text::new(format!("{}", err));
                                                    let size = compositor.size();
                                                    let popup = Popup::new("invalid-regex", contents)
                                                        .position(Some(helix_core::Position::new(
                                                            (size.height as usize).saturating_sub(2),
                                                            0,
                                                        )))
                                                        .auto_close(true);
                                                    compositor.replace_or_push("invalid-regex", popup);
                                                },
                                            ));
                                            Ok(call)
                                        };

                                        cx.jobs.callback(callback);
                                    }
                                }
                            }
                        }
                    }
                },
            )
            .with_language("regex", std::sync::Arc::clone(&cx.editor.syn_loader));
            prompt.recalculate_completion(cx.editor);
            cx.push_layer(Box::new(prompt));
        }
    }
}

#[derive(Debug)]
pub struct FilePickerData {
    root: PathBuf,
    directory_style: Style,
}
type FilePicker = Picker<PathBuf, FilePickerData>;

/// Lists the files below `root` that the file picker shows.
fn picker_files(editor: &Editor, root: &Path) -> impl Iterator<Item = PathBuf> + Send + 'static {
    use ignore::{types::TypesBuilder, WalkBuilder};

    let config = editor.config();
    let root = root.to_path_buf();

    let dedup_symlinks = config.file_picker.deduplicate_links;
    let absolute_root = root.canonicalize().unwrap_or_else(|_| root.clone());

    let mut walk_builder = WalkBuilder::new(&root);
    walk_builder
        .hidden(config.file_picker.hidden)
        .parents(config.file_picker.parents)
        .ignore(config.file_picker.ignore)
        .follow_links(config.file_picker.follow_symlinks)
        .git_ignore(config.file_picker.git_ignore)
        .git_global(config.file_picker.git_global)
        .git_exclude(config.file_picker.git_exclude)
        .sort_by_file_name(|name1, name2| name1.cmp(name2))
        .max_depth(config.file_picker.max_depth)
        .filter_entry(move |entry| filter_picker_entry(entry, &absolute_root, dedup_symlinks));

    walk_builder.add_custom_ignore_filename(helix_loader::config_dir().join("ignore"));
    walk_builder.add_custom_ignore_filename(".helix/ignore");

    // We want to exclude files that the editor can't handle yet
    let mut type_builder = TypesBuilder::new();
    type_builder
        .add(
            "compressed",
            "*.{zip,gz,bz2,zst,lzo,sz,tgz,tbz2,lz,lz4,lzma,lzo,z,Z,xz,7z,rar,cab}",
        )
        .expect("Invalid type definition");
    type_builder.negate("all");
    let excluded_types = type_builder
        .build()
        .expect("failed to build excluded_types");
    walk_builder.types(excluded_types);
    walk_builder.build().filter_map(|entry| {
        let entry = entry.ok()?;
        if !entry.path().is_file() {
            return None;
        }
        Some(entry.into_path())
    })
}

/// The recently used files (opened or saved, in this editor or another one; see
/// `[editor.recent-picker]`), most recent first, without the current buffer's file nor files
/// that no longer exist. Listed in the background: opening it never waits for the disk.
pub fn recent_picker(editor: &Editor, root: PathBuf) -> FilePicker {
    let config = editor.config();
    let data = FilePickerData {
        root,
        directory_style: editor.theme.get("ui.text.directory"),
    };
    let columns = [PickerColumn::new(
        "path",
        |item: &PathBuf, data: &FilePickerData| {
            // In the workspace: relative to it; elsewhere: the full path, `~` for the home.
            let path = match item.strip_prefix(&data.root) {
                Ok(relative) => std::borrow::Cow::Borrowed(relative),
                Err(_) => helix_stdx::path::fold_home_dir(item.as_path()),
            };
            let mut spans = Vec::with_capacity(3);
            if let Some(dirs) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let separator = if dirs.parent().is_none() {
                    // the root directory already ends with a separator
                    ""
                } else {
                    std::path::MAIN_SEPARATOR_STR
                };
                spans.extend([
                    Span::styled(dirs.to_string_lossy().into_owned(), data.directory_style),
                    Span::styled(separator, data.directory_style),
                ]);
            }
            let filename = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            spans.push(Span::raw(filename));
            Spans::from(spans).into()
        },
    )];
    let picker = Picker::new(columns, 0, [], data, move |cx, path: &PathBuf, action| {
        let old_id = cx.editor.document_id_by_path(path);
        match cx.editor.open(path, action) {
            Ok(doc_id) => {
                if old_id != Some(doc_id) {
                    default_folding(cx.editor);
                }
            }
            Err(e) => {
                let err = if let Some(err) = e.source() {
                    format!("{}", err)
                } else {
                    format!("unable to open \"{}\"", path.display())
                };
                cx.editor.set_error(err);
            }
        }
    })
    .with_preview(|_editor, path| Some((path.as_path().into(), None)))
    .show_preview(!config.file_picker.hide_preview)
    .with_title("Recent Files");

    let injector = picker.injector();
    let recent_files = editor.recent_files.clone();
    let recents_path = config.recent_picker.recents_path.clone();
    let max = config.recent_picker.max_show_entries;
    let current = doc!(editor).path().map(|path| path.to_path_buf());
    std::thread::spawn(move || {
        // A file left (or the current one) is skipped, not counted: up to `max` shown.
        let files = recent_files.list(&recents_path, usize::MAX);
        let shown = files
            .into_iter()
            .filter(|file| Some(file) != current.as_ref() && file.is_file())
            .take(max);
        for file in shown {
            if injector.push(file).is_err() {
                break;
            }
        }
    });
    picker
}

pub fn file_picker(editor: &Editor, root: PathBuf) -> FilePicker {
    use std::time::Instant;

    let config = editor.config();
    let data = FilePickerData {
        root: root.clone(),
        directory_style: editor.theme.get("ui.text.directory"),
    };

    let now = Instant::now();
    let mut files = picker_files(editor, &root);
    log::debug!("file_picker init {:?}", Instant::now().duration_since(now));

    let columns = [PickerColumn::new(
        "path",
        |item: &PathBuf, data: &FilePickerData| {
            let path = item.strip_prefix(&data.root).unwrap_or(item);
            let mut spans = Vec::with_capacity(3);
            if let Some(dirs) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                spans.extend([
                    Span::styled(dirs.to_string_lossy(), data.directory_style),
                    Span::styled(std::path::MAIN_SEPARATOR_STR, data.directory_style),
                ]);
            }
            let filename = path
                .file_name()
                .expect("normalized paths can't end in `..`")
                .to_string_lossy();
            spans.push(Span::raw(filename));
            Spans::from(spans).into()
        },
    )];
    let picker = Picker::new(columns, 0, [], data, move |cx, path: &PathBuf, action| {
        let path = helix_stdx::path::canonicalize(path);
        let old_id = cx.editor.document_id_by_path(&path);

        match cx.editor.open(&path, action) {
            Ok(doc_id) => {
                if old_id != Some(doc_id) {
                    default_folding(cx.editor);
                }
            }
            Err(e) => {
                let err = if let Some(err) = e.source() {
                    format!("{}", err)
                } else {
                    format!("unable to open \"{}\"", path.display())
                };
                cx.editor.set_error(err);
            }
        }
    })
    .with_preview(|_editor, path| Some((path.as_path().into(), None)))
    .show_preview(!config.file_picker.hide_preview)
    .with_title("Files");
    let injector = picker.injector();
    let timeout = std::time::Instant::now() + std::time::Duration::from_millis(30);

    let mut hit_timeout = false;
    for file in &mut files {
        if injector.push(file).is_err() {
            break;
        }
        if std::time::Instant::now() >= timeout {
            hit_timeout = true;
            break;
        }
    }
    if hit_timeout {
        std::thread::spawn(move || {
            for file in files {
                if injector.push(file).is_err() {
                    break;
                }
            }
        });
    }

    // Keep the list in sync with the file system.
    let watch = crate::fs_watch::FsWatch::workspace(&root);
    let walk_root = root;
    picker.with_live_refresh(
        move |_editor| watch.as_ref().map_or(0, |watch| watch.generation()),
        Some(Box::new(move |editor, injector| {
            let files = picker_files(editor, &walk_root);
            let injector = injector.clone();
            std::thread::spawn(move || {
                for file in files {
                    if injector.push(file).is_err() {
                        break;
                    }
                }
            });
        })),
        |path: &PathBuf| path.to_string_lossy().into_owned(),
    )
}

fn get_excluded_types() -> ignore::types::Types {
    use ignore::types::TypesBuilder;
    let mut type_builder = TypesBuilder::new();
    type_builder
        .add(
            "compressed",
            "*.{zip,gz,bz2,zst,lzo,sz,tgz,tbz2,lz,lz4,lzma,lzo,z,Z,xz,7z,rar,cab}",
        )
        .expect("Invalid type definition");
    type_builder.negate("all");
    type_builder
        .build()
        .expect("failed to build excluded_types")
}

pub fn directory_content(
    root: &Path,
    editor: &Editor,
) -> Result<Vec<(PathBuf, bool)>, std::io::Error> {
    directory_content_with(root, &editor.config().file_explorer)
}

/// [`directory_content`] with the `[editor.file-explorer]` settings it lists by, for use off
/// the UI thread.
pub fn directory_content_with(
    root: &Path,
    explorer: &helix_view::editor::FileExplorerConfig,
) -> Result<Vec<(PathBuf, bool)>, std::io::Error> {
    use ignore::WalkBuilder;

    struct Config<'a> {
        file_explorer: &'a helix_view::editor::FileExplorerConfig,
    }
    let config = Config {
        file_explorer: explorer,
    };

    let mut walk_builder = WalkBuilder::new(root);

    let mut content: Vec<(PathBuf, bool)> = walk_builder
        .hidden(config.file_explorer.hidden)
        .parents(config.file_explorer.parents)
        .ignore(config.file_explorer.ignore)
        .follow_links(config.file_explorer.follow_symlinks)
        .git_ignore(config.file_explorer.git_ignore)
        .git_global(config.file_explorer.git_global)
        .git_exclude(config.file_explorer.git_exclude)
        .max_depth(Some(1))
        .add_custom_ignore_filename(helix_loader::config_dir().join("ignore"))
        .add_custom_ignore_filename(".helix/ignore")
        .types(get_excluded_types())
        .build()
        .filter_map(|entry| {
            entry
                .map(|entry| {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    let mut path = path.to_path_buf();
                    if is_dir && path != root && config.file_explorer.flatten_dirs {
                        while let Some(single_child_directory) = get_child_if_single_dir(&path) {
                            path = single_child_directory;
                        }
                    }
                    (path, is_dir)
                })
                .ok()
                .filter(|entry| entry.0 != root)
        })
        .collect();

    content.sort_by(|(path1, is_dir1), (path2, is_dir2)| (!is_dir1, path1).cmp(&(!is_dir2, path2)));
    if root.parent().is_some() {
        content.insert(0, (root.join(".."), true));
    }
    Ok(content)
}

fn get_child_if_single_dir(path: &Path) -> Option<PathBuf> {
    let mut entries = path.read_dir().ok()?;
    let entry = entries.next()?.ok()?;
    let entry_path = entry.path();
    if entries.next().is_none() && entry_path.is_dir() {
        Some(entry_path)
    } else {
        None
    }
}

pub fn default_folding(editor: &mut Editor) {
    use crate::commands::typed::{fold_textobjects, FOLD_SIGNATURE};
    use helix_core::command_line::Args;

    let textobjects = editor.config.load().fold_textobjects.join(" ");
    if textobjects.is_empty() {
        return;
    }

    let loader = editor.syn_loader.load();

    let str = format!("--document {textobjects}");
    let args = Args::parse(&str, FOLD_SIGNATURE, true, |token| Ok(token.content)).unwrap();

    let (view, doc) = current!(editor);
    _ = fold_textobjects(doc, view, &loader, args);
}
pub mod completers {
    use super::Utf8PathBuf;
    use crate::ui::prompt::Completion;
    use helix_core::command_line::{self, Tokenizer};
    use helix_core::fuzzy::fuzzy_match;
    use helix_core::syntax::config::LanguageServerFeature;
    use helix_event::TaskHandle;
    use helix_view::document::SCRATCH_BUFFER_NAME;
    use helix_view::graphics::Style;
    use helix_view::theme;
    use helix_view::{editor::Config, Editor};
    use std::borrow::Cow;
    use std::collections::BTreeSet;
    use std::sync::LazyLock;
    use tui::text::Span;

    /// A completion computation to be run on a background thread.
    ///
    /// The closure is handed a [TaskHandle] and should check [TaskHandle::is_canceled] at
    /// convenient intervals, bailing out early when the results are no longer needed (i.e.
    /// when the prompt line has changed or the prompt was closed).
    pub type DeferredCompletion = Box<dyn FnOnce(&TaskHandle) -> Vec<Completion> + Send>;

    /// The result of invoking a [Completer].
    ///
    /// Completers which read only in-memory editor state complete immediately. Completers which need
    /// blocking I/O (i.e. :cd, :open, etc...) return a `Deferred` closure instead.
    pub enum CompletionResult {
        Immediate(Vec<Completion>),
        Deferred(DeferredCompletion),
    }

    fn deferred(
        f: impl FnOnce(&TaskHandle) -> Vec<Completion> + Send + 'static,
    ) -> CompletionResult {
        CompletionResult::Deferred(Box::new(f))
    }

    impl CompletionResult {
        /// Applies a transformation to the completions, either immediately or, for deferred
        /// completions, on the background thread once they have been computed.
        pub fn map(
            self,
            f: impl FnOnce(Vec<Completion>) -> Vec<Completion> + Send + 'static,
        ) -> CompletionResult {
            match self {
                CompletionResult::Immediate(completions) => {
                    CompletionResult::Immediate(f(completions))
                }
                CompletionResult::Deferred(g) => deferred(move |handle| f(g(handle))),
            }
        }
    }

    impl FromIterator<Completion> for CompletionResult {
        fn from_iter<T: IntoIterator<Item = Completion>>(items: T) -> Self {
            Self::Immediate(items.into_iter().collect())
        }
    }

    pub type Completer = fn(&Editor, &str) -> CompletionResult;

    pub fn none(_editor: &Editor, _input: &str) -> CompletionResult {
        CompletionResult::Immediate(Vec::new())
    }

    pub fn buffer(editor: &Editor, input: &str) -> CompletionResult {
        let names = editor.documents.values().map(|doc| {
            doc.relative_path()
                .map(|p| p.display().to_string().into())
                .unwrap_or_else(|| Cow::from(SCRATCH_BUFFER_NAME))
        });

        fuzzy_match(input, names, true)
            .into_iter()
            .map(|(name, _)| ((0..), name.into()))
            .collect()
    }

    pub fn snippet(editor: &Editor, input: &str) -> CompletionResult {
        fuzzy_match(
            input,
            editor.snippets.iter().map(|def| def.name.as_str()),
            false,
        )
        .into_iter()
        .map(|(name, _)| ((0..), name.to_owned().into()))
        .collect()
    }

    pub fn theme(_editor: &Editor, input: &str) -> CompletionResult {
        let input = String::from(input);
        deferred(move |_| {
            let mut names = theme::Loader::read_names(&helix_loader::config_dir().join("themes"));
            for rt_dir in helix_loader::runtime_dirs() {
                names.extend(theme::Loader::read_names(&rt_dir.join("themes")));
            }
            names.push("default".into());
            names.push("base16_default".into());
            names.sort();
            names.dedup();

            fuzzy_match(&input, names, false)
                .into_iter()
                .map(|(name, _)| ((0..), name.into()))
                .collect()
        })
    }

    /// Recursive function to get all keys from this value and add them to vec
    fn get_keys(value: &serde_json::Value, vec: &mut Vec<String>, scope: Option<&str>) {
        if let Some(map) = value.as_object() {
            for (key, value) in map.iter() {
                let key = match scope {
                    Some(scope) => format!("{}.{}", scope, key),
                    None => key.clone(),
                };
                get_keys(value, vec, Some(&key));
                if !value.is_object() {
                    vec.push(key);
                }
            }
        }
    }

    /// Completes names of language servers which are running for the current document.
    pub fn active_language_servers(editor: &Editor, input: &str) -> CompletionResult {
        let language_servers = doc!(editor).language_servers().map(|ls| ls.name());

        fuzzy_match(input, language_servers, false)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name.to_string())))
            .collect()
    }

    /// Completes names of language servers which are configured for the language of the current
    /// document.
    pub fn configured_language_servers(editor: &Editor, input: &str) -> CompletionResult {
        let language_servers = doc!(editor)
            .language_config()
            .into_iter()
            .flat_map(|config| &config.language_servers)
            .map(|ls| ls.name.as_str());

        fuzzy_match(input, language_servers, false)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name.to_string())))
            .collect()
    }

    pub fn setting(_editor: &Editor, input: &str) -> CompletionResult {
        static KEYS: LazyLock<Vec<String>> = LazyLock::new(|| {
            let mut keys = Vec::new();
            let json = serde_json::json!(Config::default());
            get_keys(&json, &mut keys, None);
            keys
        });

        fuzzy_match(input, &*KEYS, false)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name)))
            .collect()
    }

    pub fn filename(editor: &Editor, input: &str) -> CompletionResult {
        filename_with_git_ignore(editor, input, true)
    }

    pub fn filename_with_git_ignore(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
    ) -> CompletionResult {
        filename_impl(editor, input, git_ignore, |entry| {
            if entry.path().is_dir() {
                FileMatch::AcceptIncomplete
            } else {
                FileMatch::Accept
            }
        })
    }

    pub fn language(editor: &Editor, input: &str) -> CompletionResult {
        let text: String = "text".into();

        let loader = editor.syn_loader.load();
        let language_ids = loader
            .language_configs()
            .map(|config| &config.language_id)
            .chain(std::iter::once(&text));

        fuzzy_match(input, language_ids, false)
            .into_iter()
            .map(|(name, _)| ((0..), name.to_owned().into()))
            .collect()
    }

    pub fn lsp_workspace_command(editor: &Editor, input: &str) -> CompletionResult {
        let commands = doc!(editor)
            .language_servers_with_feature(LanguageServerFeature::WorkspaceCommand)
            .flat_map(|ls| {
                ls.capabilities()
                    .execute_command_provider
                    .iter()
                    .flat_map(|options| options.commands.iter())
            });

        fuzzy_match(input, commands, false)
            .into_iter()
            .map(|(name, _)| ((0..), name.to_owned().into()))
            .collect()
    }

    pub fn directory(editor: &Editor, input: &str) -> CompletionResult {
        directory_with_git_ignore(editor, input, true)
    }

    pub fn directory_with_git_ignore(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
    ) -> CompletionResult {
        filename_impl(editor, input, git_ignore, |entry| {
            if entry.path().is_dir() {
                FileMatch::Accept
            } else {
                FileMatch::Reject
            }
        })
    }

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum FileMatch {
        /// Entry should be ignored
        Reject,
        /// Entry is usable but can't be the end (for instance if the entry is a directory and we
        /// try to match a file)
        AcceptIncomplete,
        /// Entry is usable and can be the end of the match
        Accept,
    }

    fn filename_impl<F>(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
        filter_fn: F,
    ) -> CompletionResult
    where
        F: Fn(&ignore::DirEntry) -> FileMatch + Send + 'static,
    {
        // Walking the directory performs blocking I/O, which can be slow on network
        // filesystems. Defer it so that the prompt stays responsive. Only the styles below are
        // needed from the editor: everything else can move to the background thread.
        let input = String::from(input);
        let directory_style = editor.theme.get("ui.text.directory");
        let symlink_style = editor.theme.get("ui.text.symlink");
        deferred(move |handle| {
            filename_completions(
                handle,
                &input,
                git_ignore,
                directory_style,
                symlink_style,
                filter_fn,
            )
        })
    }

    // TODO: we could return an iter/lazy thing so it can fetch as many as it needs.
    fn filename_completions<F>(
        handle: &TaskHandle,
        input: &str,
        git_ignore: bool,
        directory_style: Style,
        symlink_style: Style,
        filter_fn: F,
    ) -> Vec<Completion>
    where
        F: Fn(&ignore::DirEntry) -> FileMatch,
    {
        // Rust's filename handling is really annoying.

        use ignore::WalkBuilder;
        use std::path::Path;

        let is_tilde = input == "~";
        let path = helix_stdx::path::expand_tilde(Path::new(input));

        let (dir, file_name) = if input.ends_with(std::path::MAIN_SEPARATOR) {
            (path, None)
        } else {
            let is_period = (input.ends_with((format!("{}.", std::path::MAIN_SEPARATOR)).as_str())
                && input.len() > 2)
                || input == ".";
            let file_name = if is_period {
                Some(String::from("."))
            } else {
                path.file_name()
                    .and_then(|file| file.to_str().map(|path| path.to_owned()))
            };

            let path = if is_period {
                path
            } else {
                match path.parent() {
                    Some(path) if !path.as_os_str().is_empty() => Cow::Borrowed(path),
                    // Path::new("h")'s parent is Some("")...
                    _ => Cow::Owned(helix_stdx::env::current_working_dir()),
                }
            };

            (path, file_name)
        };

        let end = input.len()..;

        let files = WalkBuilder::new(&dir)
            .hidden(false)
            .follow_links(false) // We're scanning over depth 1
            .git_ignore(git_ignore)
            .max_depth(Some(1))
            .build()
            // Stop walking early if these completions are no longer needed. Partial results
            // are fine: the canceling task has already invalidated the receiving end.
            .take_while(|_| !handle.is_canceled())
            .filter_map(|file| {
                file.ok().and_then(|entry| {
                    let fmatch = filter_fn(&entry);

                    if fmatch == FileMatch::Reject {
                        return None;
                    }

                    let path = entry.path();
                    let is_dir = path.is_dir();
                    let file_type = entry.file_type();
                    let is_symlink = file_type.is_some_and(|ft| ft.is_symlink());
                    let mut path = if is_tilde {
                        // if it's a single tilde an absolute path is displayed so that when `TAB` is pressed on
                        // one of the directories the tilde will be replaced with a valid path not with a relative
                        // home directory name.
                        // ~ -> <TAB> -> /home/user
                        // ~/ -> <TAB> -> ~/first_entry
                        path.to_path_buf()
                    } else {
                        path.strip_prefix(&dir).unwrap_or(path).to_path_buf()
                    };

                    if fmatch == FileMatch::AcceptIncomplete {
                        path.push("");
                    }

                    let path = path.into_os_string().into_string().ok()?;
                    Some(Utf8PathBuf {
                        path,
                        is_dir,
                        is_symlink,
                    })
                })
            }) // TODO: unwrap or skip
            .filter(|path| !path.path.is_empty());

        let style_from_file = |file: Utf8PathBuf| {
            if file.is_symlink {
                Span::styled(file.path, symlink_style)
            } else if file.is_dir {
                Span::styled(file.path, directory_style)
            } else {
                Span::raw(file.path)
            }
        };

        // if empty, return a list of dirs and files in current dir
        if let Some(file_name) = file_name {
            let range = (input.len().saturating_sub(file_name.len()))..;
            fuzzy_match(&file_name, files, true)
                .into_iter()
                .map(|(name, _)| (range.clone(), style_from_file(name)))
                .collect()

            // TODO: complete to longest common match
        } else {
            let mut files: Vec<_> = files
                .map(|file| (end.clone(), style_from_file(file)))
                .collect();
            files.sort_unstable_by(|(_, path1), (_, path2)| path1.content.cmp(&path2.content));
            files
        }
    }

    pub fn register(editor: &Editor, input: &str) -> CompletionResult {
        let iter = editor
            .registers
            .iter_preview()
            // Exclude special registers that shouldn't be written to
            .filter(|(ch, _)| !matches!(ch, '%' | '#' | '.'))
            .map(|(ch, _)| ch.to_string());

        fuzzy_match(input, iter, false)
            .into_iter()
            .map(|(name, _)| ((0..), name.into()))
            .collect()
    }

    pub fn program(_editor: &Editor, input: &str) -> CompletionResult {
        static PROGRAMS_IN_PATH: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
            // Go through the entire PATH and read all files into a set.
            let Some(path) = std::env::var_os("PATH") else {
                return Default::default();
            };

            std::env::split_paths(&path)
                .filter_map(|path| std::fs::read_dir(path).ok())
                .flatten()
                .filter_map(|res| {
                    let entry = res.ok()?;
                    let file_type = entry.file_type().ok()?;
                    if file_type.is_file() || file_type.is_symlink() {
                        entry.file_name().into_string().ok()
                    } else {
                        None
                    }
                })
                .collect()
        });

        // Deferred because the first access to `PROGRAMS_IN_PATH` scans every directory in `$PATH`.
        let input = String::from(input);
        deferred(move |_| {
            fuzzy_match(&input, PROGRAMS_IN_PATH.iter(), false)
                .into_iter()
                .map(|(name, _)| ((0..), name.clone().into()))
                .collect()
        })
    }

    /// This expects input to be a raw string of arguments, because this is what Signature's raw_after does.
    pub fn repeating_filenames(editor: &Editor, input: &str) -> CompletionResult {
        let token = match Tokenizer::new(input, false).last() {
            Some(token) => token.unwrap(),
            None => return filename(editor, input),
        };

        let offset = token.content_start;

        filename(editor, &input[offset..]).map(move |mut completions| {
            for completion in completions.iter_mut() {
                completion.0.start += offset;
            }
            completions
        })
    }

    pub fn shell(editor: &Editor, input: &str) -> CompletionResult {
        let (command, args, complete_command) = command_line::split(input);

        if complete_command {
            return program(editor, command);
        }

        let offset = command.len() + 1; // + 1 for separator between `command` and `args`
        repeating_filenames(editor, args).map(move |mut completions| {
            for completion in completions.iter_mut() {
                completion.0.start += offset;
            }
            completions
        })
    }

    pub fn foldable_textobjects(_editor: &Editor, input: &str) -> CompletionResult {
        let textobjects = ["class", "function", "comment"];
        fuzzy_match(input, textobjects.iter(), false)
            .into_iter()
            .map(|(name, _)| ((0..), (*name).into()))
            .collect()
    }
}

/// Applies the user's `[keys.<component>]` table to a key: the key the component handles, or
/// `None` when the key was bound to `no_op`.
pub(crate) fn component_key(
    editor: &helix_view::Editor,
    component: helix_view::component_keys::Component,
    key: helix_view::input::KeyEvent,
) -> Option<helix_view::input::KeyEvent> {
    use helix_view::component_keys::Translated;
    match editor.config().component_keys.translate(component, key) {
        Translated::Unchanged => Some(key),
        Translated::Key(key) => Some(key),
        Translated::Disabled => None,
    }
}

#[cfg(test)]
mod clear_bordered_tests {
    use super::*;
    use helix_view::graphics::{Color, Rect};

    fn surface(bg: Color) -> tui::buffer::Buffer {
        let area = Rect::new(0, 0, 10, 6);
        let mut surface = tui::buffer::Buffer::empty(area);
        surface.set_stringn(0, 0, "editortext", 10, Style::default().fg(Color::Red));
        surface.set_style(area, Style::default().bg(bg));
        surface
    }

    #[test]
    fn border_cells_keep_the_background_behind_the_popup() {
        let (editor, popup) = (Color::Rgb(255, 255, 255), Color::Rgb(240, 230, 210));
        let mut surface = surface(editor);
        let area = Rect::new(1, 0, 6, 4);
        clear_bordered(&mut surface, area, Style::default().bg(popup), true);
        for y in 0..4 {
            for x in 1..7 {
                let cell = &surface[(x, y)];
                let on_border = x == 1 || x == 6 || y == 0 || y == 3;
                let want = if on_border { editor } else { popup };
                assert_eq!(cell.bg, want, "({x}, {y})");
                if on_border {
                    // the text behind is gone, only its background is kept
                    assert_eq!(cell.symbol.as_str(), " ", "({x}, {y})");
                    assert_eq!(cell.fg, Color::Reset, "({x}, {y})");
                }
            }
        }
        // outside the popup nothing changed
        assert_eq!(surface[(0, 0)].symbol.as_str(), "e");
        assert_eq!(surface[(7, 0)].bg, editor);
    }

    #[test]
    fn which_borders_take_the_popup_background() {
        use helix_view::editor::{BorderStyle, Config};
        let with = |style: BorderStyle, gradient: Option<u8>| {
            let mut config = Config {
                border_style: Some(style),
                ..Config::default()
            };
            if let Some(thickness) = gradient {
                config.gradient_borders.enable = true;
                config.gradient_borders.thickness = thickness;
            }
            border_cells_take_background(&config)
        };
        // drawn along the outer edge of the cell, or filling its outer half
        assert!(with(BorderStyle::Plain, None));
        assert!(with(BorderStyle::QuadrantOutside, None));
        assert!(with(BorderStyle::Plain, Some(1)));
        // through the middle of the cell, or filling its inner half
        for style in [
            BorderStyle::Rounded,
            BorderStyle::Double,
            BorderStyle::Thick,
            BorderStyle::QuadrantInside,
        ] {
            assert!(!with(style, None), "{style:?}");
        }
        assert!(!with(BorderStyle::Rounded, Some(1)));
        assert!(!with(BorderStyle::Plain, Some(2)));
        // no glyphs: the popup covers its area
        assert!(with(BorderStyle::None, None));
    }

    #[test]
    fn plain_borders_hug_the_edge_and_separators_stay_lines() {
        use helix_view::editor::{BorderStyle, Config};
        let config = Config {
            border_style: Some(BorderStyle::Plain),
            ..Config::default()
        };
        let border = tui::widgets::BorderType::line_symbols(border_type(&config));
        assert_eq!(
            [
                border.vertical,
                border.horizontal,
                border.right,
                border.bottom
            ],
            ["▏", "▔", "▕", "▁"]
        );
        let separator = tui::widgets::BorderType::line_symbols(separator_type(&config));
        assert_eq!(separator.horizontal, "─");
    }

    #[test]
    fn without_a_border_the_whole_area_is_painted() {
        let (editor, popup) = (Color::Rgb(255, 255, 255), Color::Rgb(240, 230, 210));
        let mut surface = surface(editor);
        let area = Rect::new(1, 0, 6, 4);
        clear_bordered(&mut surface, area, Style::default().bg(popup), false);
        for y in 0..4 {
            for x in 1..7 {
                assert_eq!(surface[(x, y)].bg, popup, "({x}, {y})");
            }
        }
    }
}
