use crate::error::Result;
use crate::lua::api::treesitter as ts;
use crate::permissions::{require, Permission};
use helix_core::Position;
use helix_view::DocumentId;
use mlua::prelude::*;

/// Lua wrapper for a Helix buffer/document
#[derive(Clone)]
pub struct LuaBuffer {
    pub document_id: DocumentId,
}

impl LuaBuffer {
    pub fn new(document_id: DocumentId) -> Self {
        Self { document_id }
    }
}

/// Run `f` with the document of `id`, or error if it no longer exists.
fn with_doc<F, R>(id: DocumentId, f: F) -> LuaResult<R>
where
    F: FnOnce(&helix_view::Document) -> LuaResult<R>,
{
    crate::lua::with_editor(|editor| {
        let doc = editor
            .document(id)
            .ok_or_else(|| LuaError::RuntimeError(format!("Buffer {:?} no longer exists", id)))?;
        f(doc)
    })
}

/// Run `f` with the focused view and its document, erroring unless the focused
/// view shows the document `id`.
fn with_focused_doc<F, R>(id: DocumentId, what: &str, f: F) -> LuaResult<R>
where
    F: FnOnce(&mut helix_view::View, &mut helix_view::Document) -> LuaResult<R>,
{
    crate::lua::with_editor(|editor| {
        let (view, doc) = crate::lua::focused_mut(editor)?;
        // For now, only support current doc
        if doc.id() != id {
            return Err(LuaError::RuntimeError(format!(
                "{} currently only supported for the active buffer.",
                what
            )));
        }
        f(view, doc)
    })
}

/// Run `f` with the document `id` and a view to apply its changes with: any open buffer can
/// be edited, not only the focused one (like a workspace edit does). A change of a buffer
/// that is not focused is committed to its history at once.
pub(crate) fn with_doc_mut<F, R>(id: DocumentId, f: F) -> LuaResult<R>
where
    F: FnOnce(&mut helix_view::View, &mut helix_view::Document) -> LuaResult<R>,
{
    crate::lua::with_editor(|editor| {
        if editor.document(id).is_none() {
            return Err(LuaError::RuntimeError(format!(
                "Buffer {:?} no longer exists",
                id
            )));
        }
        // `get_synced_view_id` needs a focused window.
        crate::lua::focused_ref(editor)?;
        let focused = editor.tree.focus;
        let view_id = editor.get_synced_view_id(id);
        let view = editor.tree.get_mut(view_id);
        let doc = editor.documents.get_mut(&id).expect("checked above");
        let result = f(view, doc)?;
        if view_id != focused || view.doc != id {
            doc.append_changes_to_history(view);
        }
        Ok(result)
    })
}

/// Run `f` with the document `id` (mutable, without a view), or error once it is closed.
fn with_doc_mut_ref<F, R>(id: DocumentId, f: F) -> LuaResult<R>
where
    F: FnOnce(&mut helix_view::Document) -> LuaResult<R>,
{
    crate::lua::with_editor(|editor| {
        let doc = editor
            .document_mut(id)
            .ok_or_else(|| LuaError::RuntimeError(format!("Buffer {:?} no longer exists", id)))?;
        f(doc)
    })
}

/// Applies the change of the chars `from..to` to `text` (`None`: a deletion).
fn change(
    view: &mut helix_view::View,
    doc: &mut helix_view::Document,
    from: usize,
    to: usize,
    text: Option<&str>,
) {
    let rope = doc.text();
    let to = to.min(rope.len_chars());
    let from = from.min(to);
    if from == to && text.is_none_or(str::is_empty) {
        return;
    }
    let transaction =
        helix_core::Transaction::change(rope, std::iter::once((from, to, text.map(Into::into))));
    doc.apply(&transaction, view.id);
}

/// Clamp a 1-based line and 0-based column to a char offset inside `text`. A column past
/// the end of the line is the end of that line (before its line ending), not a position
/// on the next line.
fn line_col_to_char(text: &helix_core::Rope, line: usize, col: usize) -> usize {
    let text = text.slice(..);
    let row = (line.saturating_sub(1)).min(text.len_lines().saturating_sub(1));
    let start = text.line_to_char(row);
    let end = helix_core::line_ending::line_end_char_index(&text, row);
    start + col.min(end - start)
}

/// Where a plugin's diagnostic is: a whole line, a range of lines and columns, or char
/// indices.
enum DiagnosticAt {
    Line(usize),
    Range(usize, usize, usize, usize),
    Chars(usize, usize),
}

/// A diagnostic as a plugin gives it to `buffer:set_diagnostics`.
struct PluginDiagnostic {
    at: DiagnosticAt,
    message: String,
    severity: helix_core::diagnostic::Severity,
    source: Option<String>,
    code: Option<helix_core::diagnostic::NumberOrString>,
}

impl PluginDiagnostic {
    fn read(item: &LuaTable) -> LuaResult<Self> {
        use helix_core::diagnostic::{NumberOrString, Severity};
        let get = |key: &str| item.raw_get::<Option<usize>>(key);
        let at = if let (Some(start_line), Some(end_line)) = (get("start_line")?, get("end_line")?)
        {
            DiagnosticAt::Range(
                start_line,
                get("start_col")?.unwrap_or(0),
                end_line,
                get("end_col")?.unwrap_or(usize::MAX),
            )
        } else if let (Some(from), Some(to)) = (get("from")?, get("to")?) {
            DiagnosticAt::Chars(from, to)
        } else if let Some(line) = get("line")? {
            DiagnosticAt::Line(line)
        } else {
            return Err(LuaError::RuntimeError(
                "A diagnostic needs a `line` (1-based), `start_line`/`end_line` or `from`/`to`"
                    .into(),
            ));
        };
        let message: String = item
            .raw_get::<Option<String>>("message")?
            .ok_or_else(|| LuaError::RuntimeError("A diagnostic needs a `message`".into()))?;
        let severity = match item.raw_get::<Option<String>>("severity")?.as_deref() {
            Some("error") => Severity::Error,
            None | Some("warning") => Severity::Warning,
            Some("info") => Severity::Info,
            Some("hint") => Severity::Hint,
            Some(other) => {
                return Err(LuaError::RuntimeError(format!(
                    "Unknown severity '{other}': \"error\", \"warning\", \"info\" or \"hint\""
                )))
            }
        };
        let code = match item.raw_get::<LuaValue>("code")? {
            LuaValue::Nil => None,
            LuaValue::Integer(n) => Some(NumberOrString::Number(n as i32)),
            LuaValue::Number(n) => Some(NumberOrString::Number(n as i32)),
            LuaValue::String(s) => Some(NumberOrString::String(s.to_str()?.to_string())),
            _ => {
                return Err(LuaError::RuntimeError(
                    "A diagnostic's `code` is a number or a string".into(),
                ))
            }
        };
        Ok(Self {
            at,
            message,
            severity,
            source: item.raw_get::<Option<String>>("source")?,
            code,
        })
    }
}

/// Replaces `plugin`'s diagnostics of buffer `id` (in the buffer, and in the editor's list the
/// diagnostics pickers show, for a buffer with a file).
fn set_plugin_diagnostics(
    editor: &mut helix_view::Editor,
    id: DocumentId,
    plugin: &str,
    items: Vec<PluginDiagnostic>,
) -> LuaResult<()> {
    use helix_core::chars::char_is_word;
    use helix_core::diagnostic::{Diagnostic, DiagnosticProvider, Range, Severity};
    use helix_lsp::lsp;
    let provider = DiagnosticProvider::Plugin {
        name: plugin.into(),
    };
    let doc = editor
        .document_mut(id)
        .ok_or_else(|| LuaError::RuntimeError(format!("Buffer {id:?} no longer exists")))?;
    let text = doc.text().clone();
    let slice = text.slice(..);
    let len = text.len_chars();
    let diagnostics: Vec<Diagnostic> = items
        .into_iter()
        .map(|item| {
            let (start, end) = match item.at {
                DiagnosticAt::Line(line) => {
                    let row = line
                        .saturating_sub(1)
                        .min(text.len_lines().saturating_sub(1));
                    let line_start = text.line_to_char(row);
                    let line_end = helix_core::line_ending::line_end_char_index(&slice, row);
                    let first = slice
                        .line(row)
                        .chars()
                        .position(|c| !c.is_whitespace())
                        .map_or(line_start, |offset| line_start + offset)
                        .min(line_end);
                    (first, line_end)
                }
                DiagnosticAt::Range(start_line, start_col, end_line, end_col) => {
                    let a = line_col_to_char(&text, start_line, start_col);
                    let b = line_col_to_char(&text, end_line, end_col);
                    (a.min(b), a.max(b))
                }
                DiagnosticAt::Chars(from, to) => {
                    let (a, b) = (from.min(len), to.min(len));
                    (a.min(b), a.max(b))
                }
            };
            Diagnostic {
                range: Range { start, end },
                ends_at_word: start != end
                    && end != 0
                    && slice.get_char(end - 1).is_some_and(char_is_word),
                starts_at_word: start != end && slice.get_char(start).is_some_and(char_is_word),
                zero_width: start == end,
                line: text.char_to_line(start),
                message: item.message,
                severity: Some(item.severity),
                code: item.code,
                provider: provider.clone(),
                tags: Vec::new(),
                source: item.source.or_else(|| Some(plugin.to_string())),
                data: None,
            }
        })
        .collect();

    // the diagnostics pickers' list: positions as lines and chars
    let to_lsp = |diagnostic: &Diagnostic| -> lsp::Diagnostic {
        let position = |char: usize| {
            let line = text.char_to_line(char);
            lsp::Position::new(line as u32, (char - text.line_to_char(line)) as u32)
        };
        lsp::Diagnostic {
            range: lsp::Range::new(
                position(diagnostic.range.start),
                position(diagnostic.range.end),
            ),
            severity: diagnostic.severity.map(|severity| match severity {
                Severity::Error => lsp::DiagnosticSeverity::ERROR,
                Severity::Warning => lsp::DiagnosticSeverity::WARNING,
                Severity::Info => lsp::DiagnosticSeverity::INFORMATION,
                Severity::Hint => lsp::DiagnosticSeverity::HINT,
            }),
            code: diagnostic.code.clone().map(|code| match code {
                helix_core::diagnostic::NumberOrString::Number(n) => lsp::NumberOrString::Number(n),
                helix_core::diagnostic::NumberOrString::String(s) => lsp::NumberOrString::String(s),
            }),
            source: diagnostic.source.clone(),
            message: diagnostic.message.clone(),
            ..Default::default()
        }
    };
    let listed: Vec<(lsp::Diagnostic, DiagnosticProvider)> = diagnostics
        .iter()
        .map(|diagnostic| (to_lsp(diagnostic), provider.clone()))
        .collect();
    let uri = doc.uri();
    doc.replace_diagnostics(diagnostics, &[], Some(&provider));
    if let Some(uri) = uri {
        let entry = editor.diagnostics.entry(uri).or_default();
        entry.retain(|(_, from)| from != &provider);
        entry.extend(listed);
    }
    helix_event::request_redraw();
    Ok(())
}

/// Build the `{ {anchor=, head=}, ... }` table returned by the selection getters.
/// Must be called after the editor borrow has ended (table creation can run Lua finalizers).
pub(crate) fn selections_to_lua(lua: &Lua, ranges: &[(usize, usize)]) -> LuaResult<LuaTable> {
    let selections = lua.create_table_with_capacity(ranges.len(), 0)?;
    for (i, &(anchor, head)) in ranges.iter().enumerate() {
        let s = lua.create_table_with_capacity(0, 2)?;
        s.raw_set("anchor", anchor)?;
        s.raw_set("head", head)?;
        selections.raw_set(i + 1, s)?;
    }
    Ok(selections)
}

impl LuaUserData for LuaBuffer {
    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // Get buffer text
        methods.add_method("get_text", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_text")?;
            with_doc(this.document_id, |doc| Ok(doc.text().to_string()))
        });

        // Get buffer length
        methods.add_method("len", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:len")?;
            with_doc(this.document_id, |doc| Ok(doc.text().len_chars()))
        });

        // Get buffer line count
        methods.add_method("line_count", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:line_count")?;
            with_doc(this.document_id, |doc| Ok(doc.text().len_lines()))
        });

        // Get line at index (1-based)
        methods.add_method("get_line", |lua, this, line_num: usize| {
            require(lua, Permission::Read, "buffer:get_line")?;
            if line_num == 0 {
                return Err(LuaError::RuntimeError(
                    "Line numbers are 1-based (must be >= 1)".to_string(),
                ));
            }
            with_doc(this.document_id, |doc| {
                let line_idx = line_num - 1;
                if line_idx >= doc.text().len_lines() {
                    return Err(LuaError::RuntimeError(format!(
                        "Line number {} out of bounds (max {})",
                        line_num,
                        doc.text().len_lines()
                    )));
                }
                Ok(doc.text().line(line_idx).to_string())
            })
        });

        // Get buffer path
        methods.add_method("get_path", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_path")?;
            with_doc(this.document_id, |doc| {
                Ok(doc.path().map(|p| p.to_string_lossy().to_string()))
            })
        });

        // Get document ID
        methods.add_method("id", |_lua, this, ()| Ok(format!("{:?}", this.document_id)));

        // Check if buffer is modified
        methods.add_method("is_modified", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:is_modified")?;
            with_doc(this.document_id, |doc| Ok(doc.is_modified()))
        });

        // Get buffer language
        methods.add_method("get_language", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_language")?;
            with_doc(this.document_id, |doc| {
                Ok(doc.language_name().map(|s| s.to_string()))
            })
        });

        // Insert text at position (any open buffer)
        methods.add_method(
            "insert",
            |lua, this, (line, col, text): (usize, usize, String)| {
                require(lua, Permission::Edit, "buffer:insert")?;
                with_doc_mut(this.document_id, |view, doc| {
                    let offset = line_col_to_char(doc.text(), line, col);
                    change(view, doc, offset, offset, Some(&text));
                    Ok(())
                })
            },
        );

        // Delete range (any open buffer; a reversed range is accepted)
        methods.add_method(
            "delete",
            |lua,
             this,
             (start_line, start_col, end_line, end_col): (usize, usize, usize, usize)| {
                require(lua, Permission::Edit, "buffer:delete")?;
                with_doc_mut(this.document_id, |view, doc| {
                    let a = line_col_to_char(doc.text(), start_line, start_col);
                    let b = line_col_to_char(doc.text(), end_line, end_col);
                    change(view, doc, a.min(b), a.max(b), None);
                    Ok(())
                })
            },
        );

        // Replace a range with text (any open buffer)
        methods.add_method(
            "replace",
            |lua,
             this,
             (start_line, start_col, end_line, end_col, text): (
                usize,
                usize,
                usize,
                usize,
                String,
            )| {
                require(lua, Permission::Edit, "buffer:replace")?;
                with_doc_mut(this.document_id, |view, doc| {
                    let a = line_col_to_char(doc.text(), start_line, start_col);
                    let b = line_col_to_char(doc.text(), end_line, end_col);
                    change(view, doc, a.min(b), a.max(b), Some(&text));
                    Ok(())
                })
            },
        );

        // Replace the whole text (one change: one undo step)
        methods.add_method("set_text", |lua, this, text: String| {
            require(lua, Permission::Edit, "buffer:set_text")?;
            with_doc_mut(this.document_id, |view, doc| {
                let len = doc.text().len_chars();
                change(view, doc, 0, len, Some(&text));
                Ok(())
            })
        });

        // The lines `first..=last` (1-based, inclusive, clamped), without line endings; the
        // empty line after a final line ending is not one (unless asked for with `last`)
        methods.add_method(
            "get_lines",
            |lua, this, (first, last): (usize, Option<usize>)| {
                require(lua, Permission::Read, "buffer:get_lines")?;
                let lines: Vec<String> = with_doc(this.document_id, |doc| {
                    let text = doc.text().slice(..);
                    let count = text.len_lines();
                    let ends_with_newline = count > 1 && text.line(count - 1).len_chars() == 0;
                    let default_last = if ends_with_newline { count - 1 } else { count };
                    let first = first.max(1);
                    let last = last.unwrap_or(default_last).min(count);
                    Ok((first..=last)
                        .map(|line| {
                            let line = text.line(line - 1).to_string();
                            line.trim_end_matches(['\n', '\r']).to_string()
                        })
                        .collect())
                })?;
                lua.create_sequence_from(lines)
            },
        );

        // The text of a range (positions like `delete`)
        methods.add_method(
            "get_range",
            |lua,
             this,
             (start_line, start_col, end_line, end_col): (usize, usize, usize, usize)| {
                require(lua, Permission::Read, "buffer:get_range")?;
                with_doc(this.document_id, |doc| {
                    let a = line_col_to_char(doc.text(), start_line, start_col);
                    let b = line_col_to_char(doc.text(), end_line, end_col);
                    Ok(doc.text().slice(a.min(b)..a.max(b)).to_string())
                })
            },
        );

        // Write the buffer (`force`: like `:write!`)
        methods.add_method("save", |lua, this, force: Option<bool>| {
            require(lua, Permission::Files, "buffer:save")?;
            crate::lua::with_editor(|editor| {
                editor
                    .save::<std::path::PathBuf>(this.document_id, None, force.unwrap_or(false))
                    .map_err(|e| LuaError::RuntimeError(format!("Failed to save: {e}")))
            })
        });

        // Close the buffer (`force`: even with unsaved changes)
        methods.add_method("close", |lua, this, force: Option<bool>| {
            require(lua, Permission::Windows, "buffer:close")?;
            crate::lua::with_editor(|editor| {
                editor
                    .close_document(this.document_id, force.unwrap_or(false))
                    .map_err(|e| {
                        use helix_view::editor::CloseError;
                        LuaError::RuntimeError(match e {
                            CloseError::DoesNotExist => "Failed to close: no such buffer".into(),
                            CloseError::BufferModified(name) => format!(
                                "Failed to close: {name} has unsaved changes (use close(true))"
                            ),
                            CloseError::SaveError(e) => format!("Failed to close: {e}"),
                        })
                    })
            })
        });

        // Set the language (`"text"` for none), like `:lang`
        methods.add_method("set_language", |lua, this, name: String| {
            require(lua, Permission::Config, "buffer:set_language")?;
            crate::lua::with_editor(|editor| {
                let loader = editor.syn_loader.load();
                let doc = editor.documents.get_mut(&this.document_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    ))
                })?;
                if name == "text" {
                    doc.set_language(None, &loader);
                } else {
                    doc.set_language_by_language_id(&name, &loader)
                        .map_err(|e| LuaError::RuntimeError(format!("{e}")))?;
                }
                doc.detect_indent_and_line_ending();
                drop(loader);
                editor.refresh_language_servers(this.document_id);
                let doc = editor
                    .documents
                    .get_mut(&this.document_id)
                    .expect("checked above");
                let diagnostics = helix_view::Editor::doc_diagnostics(
                    &editor.language_servers,
                    &editor.diagnostics,
                    doc,
                );
                doc.replace_diagnostics(diagnostics, &[], None);
                Ok(())
            })
        });

        // Diagnostics of the plugin: { {line | start_line,start_col,end_line,end_col | from,to,
        // message, severity, source, code}, ... }; replaces the plugin's diagnostics of this
        // buffer (`{}` clears them). Shown like a language server's: underlines, gutter,
        // inline diagnostics, the diagnostics pickers.
        methods.add_method("set_diagnostics", |lua, this, list: LuaTable| {
            require(lua, Permission::Ui, "buffer:set_diagnostics")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mut items = Vec::new();
            for i in 1..=list.raw_len() {
                let item: LuaTable = list.raw_get(i)?;
                items.push(PluginDiagnostic::read(&item)?);
            }
            let count = items.len();
            crate::lua::with_editor(|editor| {
                set_plugin_diagnostics(editor, this.document_id, &plugin, items)
            })?;
            Ok(count)
        });

        // Removes the plugin's diagnostics of this buffer.
        methods.add_method("clear_diagnostics", |lua, this, ()| {
            require(lua, Permission::Ui, "buffer:clear_diagnostics")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            crate::lua::with_editor(|editor| {
                set_plugin_diagnostics(editor, this.document_id, &plugin, Vec::new())
            })
        });

        // Gutter signs of the plugin: { {line (1-based), text, style, fg, ...}, ... } (replaces
        // the plugin's signs of this buffer; `{}` clears them)
        methods.add_method("set_signs", |lua, this, list: LuaTable| {
            require(lua, Permission::Ui, "buffer:set_signs")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mut signs = Vec::new();
            for i in 1..=list.raw_len() {
                let item: LuaTable = list.raw_get(i)?;
                let line: usize = item.raw_get::<Option<usize>>("line")?.ok_or_else(|| {
                    LuaError::RuntimeError("A sign needs a `line` (1-based)".into())
                })?;
                let text: String = item.raw_get::<Option<String>>("text")?.unwrap_or_default();
                // The column is one cell wide.
                let text: String =
                    helix_core::unicode::segmentation::UnicodeSegmentation::graphemes(
                        text.as_str(),
                        true,
                    )
                    .next()
                    .filter(|g| helix_core::unicode::width::UnicodeWidthStr::width(*g) == 1)
                    .unwrap_or(" ")
                    .to_string();
                let (scope, style) = super::style::read_style(lua, &item)?;
                signs.push(helix_view::document::PluginSign {
                    plugin: plugin.clone(),
                    line: line.saturating_sub(1),
                    text,
                    scope,
                    style,
                });
            }
            crate::lua::with_editor(|editor| {
                let doc = editor.document_mut(this.document_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    ))
                })?;
                doc.plugin_signs.retain(|s| s.plugin != plugin);
                doc.plugin_signs.extend(signs);
                // Sorted by line for the gutter (a plugin's earlier sign of a line wins).
                doc.plugin_signs.sort_by_key(|s| s.line);
                helix_event::request_redraw();
                Ok(())
            })
        });

        // A gutter column of the plugin: buffer:set_gutter(id, { width, position, priority,
        // cells = { { line, text, style... }, ... } }), or nil to remove it
        methods.add_method(
            "set_gutter",
            |lua, this, (id, spec): (String, Option<LuaTable>)| {
                require(lua, Permission::Ui, "buffer:set_gutter")?;
                let plugin = crate::lua::current_plugin_name().to_string();
                let gutter = spec
                    .map(|spec| gutter_from_lua(lua, &plugin, &id, &spec))
                    .transpose()?;
                crate::lua::with_editor(|editor| {
                    let doc = editor.document_mut(this.document_id).ok_or_else(|| {
                        LuaError::RuntimeError(format!(
                            "Buffer {:?} no longer exists",
                            this.document_id
                        ))
                    })?;
                    let index = doc
                        .plugin_gutters
                        .iter()
                        .position(|g| g.plugin == plugin && g.id == id);
                    match (index, gutter) {
                        (Some(index), Some(gutter)) => doc.plugin_gutters[index] = gutter,
                        (None, Some(gutter)) => doc.plugin_gutters.push(gutter),
                        (Some(index), None) => {
                            doc.plugin_gutters.remove(index);
                        }
                        (None, None) => {}
                    }
                    helix_event::request_redraw();
                    Ok(())
                })
            },
        );

        // The gutter columns of all plugins on this buffer: { { plugin, id, width, position,
        // priority, cells = { { line, text } } } }
        methods.add_method("get_gutters", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_gutters")?;
            let gutters = with_doc(this.document_id, |doc| Ok(doc.plugin_gutters.clone()))?;
            let list = lua.create_table_with_capacity(gutters.len(), 0)?;
            for gutter in gutters {
                let entry = lua.create_table()?;
                entry.set("plugin", gutter.plugin)?;
                entry.set("id", gutter.id)?;
                entry.set("width", gutter.width)?;
                entry.set("position", if gutter.left { "left" } else { "right" })?;
                entry.set("priority", gutter.priority)?;
                let cells = lua.create_table_with_capacity(gutter.cells.len(), 0)?;
                for cell in gutter.cells {
                    let item = lua.create_table_with_capacity(0, 2)?;
                    item.set("line", cell.line + 1)?;
                    item.set("text", cell.text)?;
                    cells.push(item)?;
                }
                entry.set("cells", cells)?;
                list.push(entry)?;
            }
            Ok(list)
        });

        // Highlights of the plugin: { {start_line, start_col, end_line, end_col, style}, ...}
        // (positions like `delete`) or { {from, to, style}, ... } (char indices); `style` is a
        // theme key. Replaces the plugin's highlights of this buffer; `{}` clears them.
        methods.add_method("set_highlights", |lua, this, list: LuaTable| {
            require(lua, Permission::Ui, "buffer:set_highlights")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mut items = Vec::new();
            for i in 1..=list.raw_len() {
                let item: LuaTable = list.raw_get(i)?;
                let scope: String = item.raw_get::<Option<String>>("style")?.ok_or_else(|| {
                    LuaError::RuntimeError("A highlight needs a `style` (a theme key)".into())
                })?;
                let chars: (Option<usize>, Option<usize>) =
                    (item.raw_get("from")?, item.raw_get("to")?);
                let lines: [Option<usize>; 4] = [
                    item.raw_get("start_line")?,
                    item.raw_get("start_col")?,
                    item.raw_get("end_line")?,
                    item.raw_get("end_col")?,
                ];
                items.push((scope, chars, lines));
            }
            with_doc_mut_ref(this.document_id, |doc| {
                let text = doc.text();
                let mut highlights = Vec::with_capacity(items.len());
                for (scope, chars, lines) in items {
                    let (from, to) = match (chars, lines) {
                        ((Some(from), Some(to)), _) => (from, to),
                        (_, [Some(sl), sc, Some(el), ec]) => (
                            line_col_to_char(text, sl, sc.unwrap_or(0)),
                            line_col_to_char(text, el, ec.unwrap_or(usize::MAX)),
                        ),
                        _ => {
                            return Err(LuaError::RuntimeError(
                                "A highlight needs `from`/`to` or `start_line`/`end_line`".into(),
                            ))
                        }
                    };
                    highlights.push(helix_view::document::PluginHighlight {
                        plugin: plugin.clone(),
                        from: from.min(to),
                        to: from.max(to),
                        scope,
                    });
                }
                doc.plugin_highlights.retain(|h| h.plugin != plugin);
                doc.plugin_highlights.extend(highlights);
                helix_event::request_redraw();
                Ok(())
            })
        });

        // Information about the text: indent, line ending, encoding...
        methods.add_method("get_info", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_info")?;
            let (indent, width, tab_width, line_ending, encoding, readonly, text_width) =
                with_doc(this.document_id, |doc| {
                    let (indent, width) = match doc.indent_style {
                        helix_core::indent::IndentStyle::Tabs => ("tabs", doc.tab_width()),
                        helix_core::indent::IndentStyle::Spaces(n) => ("spaces", n as usize),
                    };
                    let line_ending = match doc.line_ending {
                        helix_core::LineEnding::Crlf => "crlf",
                        helix_core::LineEnding::LF => "lf",
                        #[allow(unreachable_patterns)]
                        _ => "other",
                    };
                    Ok((
                        indent,
                        width,
                        doc.tab_width(),
                        line_ending,
                        doc.encoding().name(),
                        doc.readonly,
                        doc.text_width(),
                    ))
                })?;
            let info = lua.create_table()?;
            info.set("indent_style", indent)?;
            info.set("indent_width", width)?;
            info.set("tab_width", tab_width)?;
            info.set("line_ending", line_ending)?;
            info.set("encoding", encoding)?;
            info.set("readonly", readonly)?;
            info.set("text_width", text_width)?;
            Ok(info)
        });

        // Get selections
        methods.add_method("get_selections", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_selections")?;
            let ranges = crate::lua::with_editor(|editor| {
                if editor.document(this.document_id).is_none() {
                    return Err(LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    )));
                }
                let (view, doc) = crate::lua::focused_ref(editor)?;
                if doc.id() != this.document_id {
                    return Ok(Vec::new());
                }
                Ok(doc
                    .selection(view.id)
                    .iter()
                    .map(|range| (range.anchor, range.head))
                    .collect::<Vec<_>>())
            })?;
            selections_to_lua(lua, &ranges)
        });

        // Get diagnostics
        methods.add_method("get_diagnostics", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_diagnostics")?;
            let diagnostics: Vec<LuaDiagnostic> = with_doc(this.document_id, |doc| {
                Ok(doc
                    .diagnostics()
                    .iter()
                    .map(|diag| LuaDiagnostic::from(diag.clone()))
                    .collect())
            })?;
            lua.create_sequence_from(diagnostics)
        });

        // Set annotations
        methods.add_method(
            "set_annotations",
            |lua, this, annotations: Vec<LuaPluginAnnotation>| {
                require(lua, Permission::Ui, "buffer:set_annotations")?;
                let plugin = crate::lua::current_plugin_name().to_string();
                let mine: Vec<helix_view::document::PluginAnnotation> = annotations
                    .into_iter()
                    .map(|a| helix_view::document::PluginAnnotation {
                        plugin: plugin.clone(),
                        char_idx: a.char_idx,
                        text: a.text,
                        style: a.style,
                        fg: a.fg,
                        bg: a.bg,
                        offset: a.offset,
                        is_line: a.is_line,
                        virt_line_idx: a.virt_line_idx,
                        dropped_text: a.dropped_text,
                        align: a.align,
                    })
                    .collect();

                with_focused_doc(this.document_id, "Annotations", |view, doc| {
                    // Only the caller's annotations are replaced: the other plugins' stay.
                    let annots = doc.plugin_annotations.entry(view.id).or_default();
                    annots.retain(|a| a.plugin != plugin);
                    annots.extend(mine);
                    // The renderers binary-search annotations by char_idx; the sort is
                    // stable so annotations at the same position keep their order.
                    annots.sort_by_key(|a| a.char_idx);
                    if annots.is_empty() {
                        doc.plugin_annotations.remove(&view.id);
                    }
                    Ok(())
                })
            },
        );

        // Remove the caller's annotations of this buffer, in every window (any buffer)
        methods.add_method("clear_annotations", |lua, this, ()| {
            require(lua, Permission::Ui, "buffer:clear_annotations")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            crate::lua::with_editor(|editor| {
                let doc = editor.document_mut(this.document_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    ))
                })?;
                doc.plugin_annotations.retain(|_, annots| {
                    annots.retain(|a| a.plugin != plugin);
                    !annots.is_empty()
                });
                Ok(())
            })
        });

        // Text drawn over the blank cells of lines in the focused window: { { line (1-based),
        // col (0-based), text, style... }, ... } (replaces the caller's; `{}` clears them)
        methods.add_method("set_overlays", |lua, this, list: LuaTable| {
            require(lua, Permission::Ui, "buffer:set_overlays")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mut mine = Vec::with_capacity(list.raw_len());
            for i in 1..=list.raw_len() {
                let item: LuaTable = list.raw_get(i)?;
                let line: usize = item
                    .raw_get::<Option<usize>>("line")?
                    .filter(|line| *line > 0)
                    .ok_or_else(|| {
                        LuaError::RuntimeError("An overlay needs a `line` (1-based)".into())
                    })?;
                let col: usize = item.raw_get::<Option<usize>>("col")?.unwrap_or(0);
                let text: String = item.raw_get::<Option<String>>("text")?.unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                use helix_view::document::OverlayRows;
                let rows = match item.raw_get::<Option<String>>("rows")?.as_deref() {
                    None | Some("first") => OverlayRows::First,
                    Some("wrapped") => OverlayRows::Wrapped,
                    Some("all") => OverlayRows::All,
                    Some(other) => {
                        return Err(LuaError::RuntimeError(format!(
                        "An overlay's `rows` is \"first\", \"wrapped\" or \"all\", not \"{other}\""
                    )))
                    }
                };
                let (scope, style) = super::style::read_style(lua, &item)?;
                mine.push(helix_view::document::PluginOverlay {
                    plugin: plugin.clone(),
                    line: line - 1,
                    col,
                    text,
                    rows,
                    scope,
                    style,
                });
            }
            with_focused_doc(this.document_id, "Overlays", |view, doc| {
                let overlays = doc.plugin_overlays.entry(view.id).or_default();
                overlays.retain(|o| o.plugin != plugin);
                overlays.extend(mine);
                // By line for the renderer; stable: a plugin's earlier overlay of a cell is
                // drawn first (the later one on top).
                overlays.sort_by_key(|o| o.line);
                if overlays.is_empty() {
                    doc.plugin_overlays.remove(&view.id);
                }
                helix_event::request_redraw();
                Ok(())
            })
        });

        // The overlays of every plugin in the focused window: { { plugin, line, col, text } }
        methods.add_method("get_overlays", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_overlays")?;
            let overlays = with_focused_doc(this.document_id, "Overlays", |view, doc| {
                Ok(doc
                    .plugin_overlays
                    .get(&view.id)
                    .cloned()
                    .unwrap_or_default())
            })?;
            let list = lua.create_table_with_capacity(overlays.len(), 0)?;
            for overlay in overlays {
                let item = lua.create_table_with_capacity(0, 5)?;
                item.raw_set("plugin", overlay.plugin)?;
                item.raw_set("line", overlay.line + 1)?;
                item.raw_set("col", overlay.col)?;
                item.raw_set("text", overlay.text)?;
                use helix_view::document::OverlayRows;
                let rows = match overlay.rows {
                    OverlayRows::First => "first",
                    OverlayRows::Wrapped => "wrapped",
                    OverlayRows::All => "all",
                };
                item.raw_set("rows", rows)?;
                list.raw_push(item)?;
            }
            Ok(list)
        });

        // Remove the caller's overlays of this buffer, in every window
        methods.add_method("clear_overlays", |lua, this, ()| {
            require(lua, Permission::Ui, "buffer:clear_overlays")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            crate::lua::with_editor(|editor| {
                let doc = editor.document_mut(this.document_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    ))
                })?;
                doc.plugin_overlays.retain(|_, overlays| {
                    overlays.retain(|o| o.plugin != plugin);
                    !overlays.is_empty()
                });
                helix_event::request_redraw();
                Ok(())
            })
        });

        // Styles of the line numbers of lines in the focused window: { { line | start_line,
        // end_line (1-based), style... }, ... } (replaces the caller's; `{}` clears them)
        methods.add_method("set_line_number_styles", |lua, this, list: LuaTable| {
            require(lua, Permission::Ui, "buffer:set_line_number_styles")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            let mut mine = Vec::with_capacity(list.raw_len());
            for i in 1..=list.raw_len() {
                let item: LuaTable = list.raw_get(i)?;
                let line: Option<usize> = item.raw_get("line")?;
                let start: Option<usize> = item.raw_get("start_line")?;
                let end: Option<usize> = item.raw_get("end_line")?;
                let (first, last) = match (line, start, end) {
                    (Some(line), None, None) => (line, line),
                    (None, Some(start), end) => (start, end.unwrap_or(start)),
                    _ => {
                        return Err(LuaError::RuntimeError(
                            "A line number style needs a `line`, or a `start_line` and an \
                             `end_line` (1-based)"
                                .into(),
                        ))
                    }
                };
                if first == 0 || last < first {
                    return Err(LuaError::RuntimeError(format!(
                        "Line number style: lines {first} to {last} (1-based, the first \
                         first)"
                    )));
                }
                let (scope, style) = super::style::read_style(lua, &item)?;
                mine.push(helix_view::document::PluginLineNumberStyle {
                    plugin: plugin.clone(),
                    first: first - 1,
                    last: last - 1,
                    scope,
                    style,
                });
            }
            with_focused_doc(this.document_id, "Line number styles", |view, doc| {
                let styles = doc.plugin_line_numbers.entry(view.id).or_default();
                styles.retain(|s| s.plugin != plugin);
                styles.extend(mine);
                if styles.is_empty() {
                    doc.plugin_line_numbers.remove(&view.id);
                }
                helix_event::request_redraw();
                Ok(())
            })
        });

        // The line number styles of every plugin in the focused window:
        // { { plugin, start_line, end_line } }
        methods.add_method("get_line_number_styles", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_line_number_styles")?;
            let styles = with_focused_doc(this.document_id, "Line number styles", |view, doc| {
                Ok(doc
                    .plugin_line_numbers
                    .get(&view.id)
                    .cloned()
                    .unwrap_or_default())
            })?;
            let list = lua.create_table_with_capacity(styles.len(), 0)?;
            for style in styles {
                let item = lua.create_table_with_capacity(0, 3)?;
                item.raw_set("plugin", style.plugin)?;
                item.raw_set("start_line", style.first + 1)?;
                item.raw_set("end_line", style.last + 1)?;
                list.raw_push(item)?;
            }
            Ok(list)
        });

        // Remove the caller's line number styles of this buffer, in every window
        methods.add_method("clear_line_number_styles", |lua, this, ()| {
            require(lua, Permission::Ui, "buffer:clear_line_number_styles")?;
            let plugin = crate::lua::current_plugin_name().to_string();
            crate::lua::with_editor(|editor| {
                let doc = editor.document_mut(this.document_id).ok_or_else(|| {
                    LuaError::RuntimeError(format!(
                        "Buffer {:?} no longer exists",
                        this.document_id
                    ))
                })?;
                doc.plugin_line_numbers.retain(|_, styles| {
                    styles.retain(|s| s.plugin != plugin);
                    !styles.is_empty()
                });
                helix_event::request_redraw();
                Ok(())
            })
        });

        // The matches of a regex in the buffer, like Helix's search finds them
        methods.add_method(
            "find_all",
            |lua, this, (pattern, options): (String, Option<LuaTable>)| {
                require(lua, Permission::Read, "buffer:find_all")?;
                let max: usize = match &options {
                    Some(options) => options.get::<Option<usize>>("max")?,
                    None => None,
                }
                .unwrap_or(FIND_ALL_DEFAULT_MAX)
                .min(FIND_ALL_MAX);
                let case_insensitive: Option<bool> = match &options {
                    Some(options) => options.get("case_insensitive")?,
                    None => None,
                };
                let matches = crate::lua::with_editor(|editor| {
                    let smart_case = editor.config().search.smart_case;
                    let doc = editor.document(this.document_id).ok_or_else(|| {
                        LuaError::RuntimeError(format!(
                            "Buffer {:?} no longer exists",
                            this.document_id
                        ))
                    })?;
                    find_all(doc, &pattern, case_insensitive, smart_case, max)
                })?;
                let list = lua.create_table_with_capacity(matches.len(), 0)?;
                for (from, to) in matches {
                    let item = lua.create_table_with_capacity(0, 2)?;
                    item.set("from", from)?;
                    item.set("to", to)?;
                    list.push(item)?;
                }
                Ok(list)
            },
        );

        // Show a panel next to a line, drawn over the text (`nil` removes it)
        methods.add_method("set_panel", |lua, this, panel: Option<LuaTable>| {
            require(lua, Permission::Ui, "buffer:set_panel")?;
            let panel = panel.as_ref().map(panel_from_table).transpose()?;
            with_focused_doc(this.document_id, "Panels", |view, doc| {
                match panel {
                    Some(panel) => doc.plugin_panels.insert(view.id, panel),
                    None => doc.plugin_panels.remove(&view.id),
                };
                Ok(())
            })
        });

        // The syntax tree (see `treesitter.rs`): nodes are snapshot tables.
        methods.add_method("has_syntax", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:has_syntax")?;
            with_doc(this.document_id, |doc| Ok(doc.syntax().is_some()))
        });
        methods.add_method("syntax_root", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:syntax_root")?;
            let node = with_doc(this.document_id, ts::root)?;
            node.into_lua(lua)
        });
        methods.add_method(
            "syntax_node",
            |lua, this, (from, options): (usize, Option<LuaTable>)| {
                require(lua, Permission::Read, "buffer:syntax_node")?;
                let (to, named) = match &options {
                    Some(options) => (
                        options.get::<Option<usize>>("to")?,
                        options.get::<Option<bool>>("named")?,
                    ),
                    None => (None, None),
                };
                let node = with_doc(this.document_id, |doc| {
                    ts::node_at(doc, from, to.unwrap_or(from), named.unwrap_or(true))
                })?;
                node.into_lua(lua)
            },
        );
        methods.add_method("syntax_parent", |lua, this, node: LuaTable| {
            require(lua, Permission::Read, "buffer:syntax_parent")?;
            let node = ts::NodeRef::from_lua(&node)?;
            let parent = with_doc(this.document_id, |doc| ts::parent(doc, &node))?;
            parent.map(|parent| parent.into_lua(lua)).transpose()
        });
        methods.add_method(
            "syntax_children",
            |lua, this, (node, options): (LuaTable, Option<LuaTable>)| {
                require(lua, Permission::Read, "buffer:syntax_children")?;
                let node = ts::NodeRef::from_lua(&node)?;
                let named = match &options {
                    Some(options) => options.get::<Option<bool>>("named")?,
                    None => None,
                }
                .unwrap_or(false);
                let children = with_doc(this.document_id, |doc| ts::children(doc, &node, named))?;
                ts::nodes_into_lua(lua, children)
            },
        );
        methods.add_method("syntax_text", |lua, this, node: LuaTable| {
            require(lua, Permission::Read, "buffer:syntax_text")?;
            let node = ts::NodeRef::from_lua(&node)?;
            with_doc(this.document_id, |doc| ts::node_text(doc, &node))
        });
        methods.add_method(
            "syntax_query",
            |lua, this, (source, options): (String, Option<LuaTable>)| {
                require(lua, Permission::Read, "buffer:syntax_query")?;
                let (from, to, max) = match &options {
                    Some(options) => (
                        options.get::<Option<usize>>("from")?,
                        options.get::<Option<usize>>("to")?,
                        options.get::<Option<usize>>("max")?,
                    ),
                    None => (None, None, None),
                };
                let max = max.unwrap_or(ts::QUERY_DEFAULT_MAX).min(ts::QUERY_MAX);
                let matches = with_doc(this.document_id, |doc| {
                    let range = match (from, to) {
                        (None, None) => None,
                        (from, to) => Some((from.unwrap_or(0), to.unwrap_or(usize::MAX))),
                    };
                    ts::query(doc, &source, range, max)
                })?;
                ts::matches_into_lua(lua, matches)
            },
        );

        // Get cursor position (char index)
        methods.add_method("get_cursor", |lua, this, ()| {
            require(lua, Permission::Read, "buffer:get_cursor")?;
            crate::lua::with_editor(|editor| {
                let (view, doc) = crate::lua::focused_ref(editor)?;
                if doc.id() != this.document_id {
                    return Err(LuaError::RuntimeError(
                        "Current view is not showing this buffer".into(),
                    ));
                }
                Ok(doc
                    .selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)))
            })
        });

        // Convert char index to line index (0-based)
        methods.add_method("char_to_line", |lua, this, char_idx: usize| {
            require(lua, Permission::Read, "buffer:char_to_line")?;
            with_doc(this.document_id, |doc| {
                Ok(doc
                    .text()
                    .char_to_line(char_idx.min(doc.text().len_chars())))
            })
        });

        // Convert line index to char index (0-based)
        methods.add_method("line_to_char", |lua, this, line_idx: usize| {
            require(lua, Permission::Read, "buffer:line_to_char")?;
            with_doc(this.document_id, |doc| {
                Ok(doc
                    .text()
                    .line_to_char(line_idx.min(doc.text().len_lines())))
            })
        });

        // Get visual column for char index
        methods.add_method("get_visual_column", |lua, this, char_idx: usize| {
            require(lua, Permission::Read, "buffer:get_visual_column")?;
            with_doc(this.document_id, |doc| {
                let text = doc.text();
                let char_idx = char_idx.min(text.len_chars());
                let line_idx = text.char_to_line(char_idx);
                let line_start = text.line_to_char(line_idx);
                let tab_width = doc.tab_width().max(1);

                let mut column = 0;
                for ch in text.slice(line_start..char_idx).chars() {
                    if ch == '\t' {
                        column += tab_width - (column % tab_width);
                    } else {
                        column +=
                            helix_core::unicode::width::UnicodeWidthChar::width(ch).unwrap_or(1);
                    }
                }
                Ok(column)
            })
        });
    }

    fn add_fields<'lua, F: LuaUserDataFields<Self>>(fields: &mut F) {
        // Add read-only fields
        fields.add_field_method_get("document_id", |_lua, this| {
            Ok(format!("{:?}", this.document_id))
        });
    }
}

/// The widest gutter column of a plugin.
const MAX_GUTTER_WIDTH: usize = 20;

/// The text cut to `width` cells (whole graphemes).
fn fit_width(text: &str, width: usize) -> String {
    use helix_core::unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr};
    let mut used = 0;
    let mut fitted = String::new();
    for grapheme in text.graphemes(true) {
        let cells = grapheme.width();
        if used + cells > width {
            break;
        }
        used += cells;
        fitted.push_str(grapheme);
    }
    fitted
}

/// A gutter column from its Lua spec (`buffer:set_gutter`).
fn gutter_from_lua(
    lua: &Lua,
    plugin: &str,
    id: &str,
    spec: &LuaTable,
) -> LuaResult<helix_view::document::PluginGutter> {
    let width: usize = spec.raw_get::<Option<usize>>("width")?.unwrap_or(1);
    if !(1..=MAX_GUTTER_WIDTH).contains(&width) {
        return Err(LuaError::RuntimeError(format!(
            "A gutter column is 1 to {MAX_GUTTER_WIDTH} cells wide (got {width})"
        )));
    }
    let left = match spec.raw_get::<Option<String>>("position")?.as_deref() {
        None | Some("left") => true,
        Some("right") => false,
        Some(other) => {
            return Err(LuaError::RuntimeError(format!(
                "Invalid position '{other}' (expected left or right)"
            )))
        }
    };
    let priority: i64 = spec.raw_get::<Option<i64>>("priority")?.unwrap_or(0);
    let mut cells = Vec::new();
    if let Some(list) = spec.raw_get::<Option<LuaTable>>("cells")? {
        for i in 1..=list.raw_len() {
            let item: LuaTable = list.raw_get(i)?;
            let line: usize = item.raw_get::<Option<usize>>("line")?.ok_or_else(|| {
                LuaError::RuntimeError("A gutter cell needs a `line` (1-based)".into())
            })?;
            let text: String = item.raw_get::<Option<String>>("text")?.unwrap_or_default();
            if text.contains(['\n', '\r', '\t']) {
                return Err(LuaError::RuntimeError(
                    "A gutter cell is text of one line, without tabs".into(),
                ));
            }
            let (scope, style) = super::style::read_style(lua, &item)?;
            cells.push(helix_view::document::PluginGutterCell {
                line: line.saturating_sub(1),
                text: fit_width(&text, width),
                scope,
                style,
            });
        }
    }
    // by line (the first cell of a line wins)
    cells.sort_by_key(|cell| cell.line);
    cells.dedup_by_key(|cell| cell.line);
    Ok(helix_view::document::PluginGutter {
        plugin: plugin.to_string(),
        id: id.to_string(),
        width,
        left,
        priority,
        cells,
    })
}

/// How many matches `find_all` returns without a `max`, and at most.
const FIND_ALL_DEFAULT_MAX: usize = 10_000;
const FIND_ALL_MAX: usize = 1_000_000;

/// The matches of `pattern` in `doc` as `(from, to)` char indices (`to` exclusive), at most
/// `max`: the regex is built like Helix's search builds it (multi-line, the buffer's line
/// ending), case-insensitive as asked, or with smart case (no uppercase letter in the
/// pattern) when `smart_case` is on. An invalid pattern is an error.
fn find_all(
    doc: &helix_view::Document,
    pattern: &str,
    case_insensitive: Option<bool>,
    smart_case: bool,
    max: usize,
) -> LuaResult<Vec<(usize, usize)>> {
    use helix_stdx::rope::{self, RopeSliceExt};
    let case_insensitive =
        case_insensitive.unwrap_or_else(|| smart_case && !pattern.chars().any(char::is_uppercase));
    let regex = rope::RegexBuilder::new()
        .syntax(
            rope::Config::new()
                .case_insensitive(case_insensitive)
                .multi_line(true)
                .crlf(doc.line_ending == helix_core::LineEnding::Crlf),
        )
        .build(pattern)
        .map_err(|err| LuaError::RuntimeError(format!("Invalid pattern {pattern:?}: {err}")))?;
    let text = doc.text().slice(..);
    Ok(regex
        .find_iter(text.regex_input())
        .take(max)
        .map(|found| {
            (
                text.byte_to_char(found.start()),
                text.byte_to_char(found.end()),
            )
        })
        .collect())
}

/// Lua wrapper for a plugin annotation
#[derive(Clone)]
pub struct LuaPluginAnnotation {
    pub char_idx: usize,
    pub text: String,
    pub style: Option<String>,
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub offset: u16,
    pub is_line: bool,
    pub virt_line_idx: Option<u16>,
    pub dropped_text: Option<String>,
    pub align: helix_view::document::AnnotationAlign,
}

impl LuaUserData for LuaPluginAnnotation {
    fn add_fields<'lua, F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("char_idx", |_lua, this| Ok(this.char_idx));
        fields.add_field_method_get("text", |_lua, this| Ok(this.text.clone()));
        fields.add_field_method_get("style", |_lua, this| Ok(this.style.clone()));
        fields.add_field_method_get("fg", |_lua, this| Ok(this.fg.clone()));
        fields.add_field_method_get("bg", |_lua, this| Ok(this.bg.clone()));
        fields.add_field_method_get("offset", |_lua, this| Ok(this.offset));
        fields.add_field_method_get("is_line", |_lua, this| Ok(this.is_line));
        fields.add_field_method_get("virt_line_idx", |_lua, this| Ok(this.virt_line_idx));
        fields.add_field_method_get("dropped_text", |_lua, this| Ok(this.dropped_text.clone()));
        fields.add_field_method_get("align", |_lua, this| {
            use helix_view::document::AnnotationAlign;
            Ok(match this.align {
                AnnotationAlign::Eol => "eol",
                AnnotationAlign::Right => "right",
                AnnotationAlign::EolOrRight => "eol_or_right",
            })
        });
    }

    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(_methods: &mut M) {}
}

impl LuaPluginAnnotation {
    /// Read an annotation from a table. A field of the wrong type (or out of range, like
    /// a negative `offset`) is an error rather than silently ignored, and so is an error
    /// raised by a metamethod (which may be the time limit aborting it).
    fn from_table(table: &LuaTable) -> LuaResult<Self> {
        Ok(LuaPluginAnnotation {
            char_idx: table.get("char_idx")?,
            text: table.get("text")?,
            style: table.get("style")?,
            fg: table.get("fg")?,
            bg: table.get("bg")?,
            offset: Self::offset_from_table(table)?,
            is_line: table.get::<Option<bool>>("is_line")?.unwrap_or(false),
            virt_line_idx: table.get("virt_line_idx")?,
            dropped_text: table.get("dropped_text")?,
            align: Self::align_from_table(table)?,
        })
    }

    /// The `align` field: `"eol"` (the default), `"right"` or `"eol_or_right"`.
    fn align_from_table(table: &LuaTable) -> LuaResult<helix_view::document::AnnotationAlign> {
        use helix_view::document::AnnotationAlign;
        match table.get::<Option<String>>("align")?.as_deref() {
            None | Some("eol") => Ok(AnnotationAlign::Eol),
            Some("right") => Ok(AnnotationAlign::Right),
            Some("eol_or_right") => Ok(AnnotationAlign::EolOrRight),
            Some(other) => Err(LuaError::RuntimeError(format!(
                "invalid annotation align {other:?} (expected \"eol\", \"right\" or \"eol_or_right\")"
            ))),
        }
    }
}

impl LuaPluginAnnotation {
    /// The `offset` field (0 when absent). A column past `u16::MAX` (a very long line)
    /// saturates to `u16::MAX`; a negative, fractional or NaN number, or a value that isn't a
    /// number, is an error.
    fn offset_from_table(table: &LuaTable) -> LuaResult<u16> {
        let invalid = |message: String| LuaError::FromLuaConversionError {
            from: "number",
            to: "u16".to_string(),
            message: Some(format!("invalid annotation offset: {message}")),
        };
        let number = match table.get::<Option<LuaValue>>("offset")? {
            None => return Ok(0),
            Some(LuaValue::Integer(int)) => {
                return if int < 0 {
                    Err(invalid(format!("{int} is negative")))
                } else {
                    Ok(u16::try_from(int).unwrap_or(u16::MAX))
                };
            }
            Some(LuaValue::Number(number)) => number,
            // Not a number: a numeric string still converts, anything else is an error.
            Some(_) => match table.get::<Option<f64>>("offset")? {
                Some(number) => number,
                None => return Ok(0),
            },
        };
        if number.is_nan() || number < 0.0 || number.fract() != 0.0 && number.is_finite() {
            return Err(invalid(format!("{number} is not a column")));
        }
        // `as` saturates (and maps +inf to u16::MAX).
        Ok(number as u16)
    }
}

impl FromLua for LuaPluginAnnotation {
    fn from_lua(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::Table(table) => Self::from_table(&table),
            LuaValue::UserData(ud) => ud.borrow::<Self>().map(|s| s.clone()),
            _ => Err(LuaError::FromLuaConversionError {
                from: "LuaValue",
                to: "LuaPluginAnnotation".to_string(),
                message: Some("Expected UserData".to_string()),
            }),
        }
    }
}

/// Read a panel (`buffer:set_panel`) from a table: `char_idx` and `items` (each with `text`,
/// and an optional `icon` and `icon_fg`) are required; the other fields keep their defaults
/// when absent. A field of the wrong type is an error.
fn panel_from_table(table: &LuaTable) -> LuaResult<helix_view::annotations::panel::PluginPanel> {
    use helix_view::annotations::panel::{PanelItem, PluginPanel};

    let defaults = PluginPanel::default();
    let items = table
        .get::<Vec<LuaTable>>("items")?
        .iter()
        .map(|item| {
            Ok(PanelItem {
                icon: item.get::<Option<String>>("icon")?.unwrap_or_default(),
                icon_fg: item.get("icon_fg")?,
                text: item.get("text")?,
            })
        })
        .collect::<LuaResult<_>>()?;
    let min_width = match table.get::<Option<i64>>("min_width")? {
        None => defaults.min_width,
        Some(width) if width < 0 => {
            return Err(LuaError::RuntimeError(format!(
                "invalid panel min_width: {width} is negative"
            )))
        }
        Some(width) => u16::try_from(width).unwrap_or(u16::MAX),
    };
    Ok(PluginPanel {
        char_idx: table.get("char_idx")?,
        items,
        footer: table.get("footer")?,
        fg: table.get("fg")?,
        bg: table.get("bg")?,
        arrow: table
            .get::<Option<String>>("arrow")?
            .unwrap_or(defaults.arrow),
        under_arrow: table
            .get::<Option<String>>("under_arrow")?
            .unwrap_or(defaults.under_arrow),
        arrow_fg: table.get("arrow_fg")?,
        left_cap: table
            .get::<Option<String>>("left_cap")?
            .unwrap_or(defaults.left_cap),
        right_cap: table
            .get::<Option<String>>("right_cap")?
            .unwrap_or(defaults.right_cap),
        min_width,
        // Strictly a boolean: Lua would read any other value as true.
        show_in_insert: match table.get::<LuaValue>("show_in_insert")? {
            LuaValue::Nil => defaults.show_in_insert,
            LuaValue::Boolean(show) => show,
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "invalid panel show_in_insert: expected a boolean, got {}",
                    other.type_name()
                )))
            }
        },
    })
}

/// Lua wrapper for a text position
#[derive(Clone, Copy)]
pub struct LuaPosition {
    pub row: usize,
    pub col: usize,
}

impl LuaUserData for LuaPosition {
    fn add_fields<'lua, F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("row", |_lua, this| Ok(this.row));
        fields.add_field_method_get("col", |_lua, this| Ok(this.col));
    }

    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(LuaMetaMethod::ToString, |_lua, this, ()| {
            Ok(format!("Position({}:{})", this.row, this.col))
        });
    }
}

impl From<Position> for LuaPosition {
    fn from(pos: Position) -> Self {
        Self {
            row: pos.row,
            col: pos.col,
        }
    }
}

impl From<LuaPosition> for Position {
    fn from(pos: LuaPosition) -> Self {
        Position {
            row: pos.row,
            col: pos.col,
        }
    }
}

/// Lua wrapper for a text range
#[derive(Clone, Copy)]
pub struct LuaRange {
    pub start: usize,
    pub end: usize,
}

impl LuaUserData for LuaRange {
    fn add_fields<'lua, F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("start", |_lua, this| Ok(this.start));
        fields.add_field_method_get("end", |_lua, this| Ok(this.end));
    }

    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(LuaMetaMethod::ToString, |_lua, this, ()| {
            Ok(format!("Range({}-{})", this.start, this.end))
        });
    }
}

/// Lua wrapper for a diagnostic
#[derive(Clone)]
pub struct LuaDiagnostic {
    pub range: LuaRange,
    pub line: usize,
    pub message: String,
    pub severity: Option<String>,
}

impl LuaUserData for LuaDiagnostic {
    fn add_fields<'lua, F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("range", |_lua, this| Ok(this.range));
        fields.add_field_method_get("line", |_lua, this| Ok(this.line));
        fields.add_field_method_get("message", |_lua, this| Ok(this.message.clone()));
        fields.add_field_method_get("severity", |_lua, this| Ok(this.severity.clone()));
    }
}

impl From<helix_core::Diagnostic> for LuaDiagnostic {
    fn from(diag: helix_core::Diagnostic) -> Self {
        let severity = diag.severity.map(|s| {
            match s {
                helix_core::diagnostic::Severity::Hint => "hint",
                helix_core::diagnostic::Severity::Info => "info",
                helix_core::diagnostic::Severity::Warning => "warning",
                helix_core::diagnostic::Severity::Error => "error",
            }
            .to_string()
        });

        Self {
            range: LuaRange {
                start: diag.range.start,
                end: diag.range.end,
            },
            line: diag.line,
            message: diag.message,
            severity,
        }
    }
}

/// Register buffer API in the Helix Lua global table
pub fn register_buffer_api(lua: &Lua, helix_table: &LuaTable) -> Result<()> {
    let buffer_module = lua.create_table()?;

    // helix.buffer.get_current() - Get current buffer
    let get_current = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.buffer.get_current")?;
        let id = crate::lua::with_editor(|editor| Ok(crate::lua::focused_ref(editor)?.1.id()))?;
        Ok(LuaBuffer::new(id))
    })?;
    buffer_module.set("get_current", get_current)?;

    // helix.buffer.get_by_id(id) - The buffer with the id given by `buffer:id()` or an
    // event's `document_id`, or nil if there is none
    let get_by_id = lua.create_function(|lua, id: String| {
        require(lua, Permission::Read, "helix.buffer.get_by_id")?;
        let id = crate::lua::with_editor(|editor| {
            Ok(editor
                .documents
                .keys()
                .copied()
                .find(|doc_id| format!("{:?}", doc_id) == id))
        })?;
        Ok(id.map(LuaBuffer::new))
    })?;
    buffer_module.set("get_by_id", get_by_id)?;

    // helix.buffer.list() - List all buffers
    let list = lua.create_function(|lua, ()| {
        require(lua, Permission::Read, "helix.buffer.list")?;
        let ids: Vec<DocumentId> =
            crate::lua::with_editor(|editor| Ok(editor.documents.keys().copied().collect()))?;
        lua.create_sequence_from(ids.into_iter().map(LuaBuffer::new))
    })?;
    buffer_module.set("list", list)?;

    // helix.buffer.create(text, language) - A new scratch buffer with `text`, loaded but not
    // shown (show it with window:set_buffer or helix.window.split)
    let create =
        lua.create_function(|lua, (text, language): (Option<String>, Option<String>)| {
            require(lua, Permission::Edit, "helix.buffer.create")?;
            let id = crate::lua::with_editor(|editor| {
                // A new buffer is set up for the focused window.
                crate::lua::focused_ref(editor)?;
                Ok(editor.new_file(helix_view::editor::Action::Load))
            })?;
            if let Some(text) = text {
                // (a new buffer holds a line ending)
                with_doc_mut(id, |view, doc| {
                    let len = doc.text().len_chars();
                    change(view, doc, 0, len, Some(&text));
                    Ok(())
                })?;
            }
            if let Some(language) = language {
                crate::lua::with_editor(|editor| {
                    let loader = editor.syn_loader.load();
                    let doc = editor.documents.get_mut(&id).expect("just created");
                    doc.set_language_by_language_id(&language, &loader)
                        .map_err(|e| LuaError::RuntimeError(format!("{e}")))
                })?;
            }
            Ok(LuaBuffer::new(id))
        })?;
    buffer_module.set("create", create)?;

    // helix.buffer.annotation(table) - Create a new annotation
    let annotation =
        lua.create_function(|_lua, table: LuaTable| LuaPluginAnnotation::from_table(&table))?;
    buffer_module.set("annotation", annotation)?;

    helix_table.set("buffer", buffer_module)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_buffer_creation() {
        let buffer = LuaBuffer::new(DocumentId::default());
        assert_eq!(
            format!("{:?}", buffer.document_id),
            format!("{:?}", DocumentId::default())
        );
    }

    #[test]
    fn panels_are_read_with_defaults_and_validated() {
        let lua = Lua::new();
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
        let read = |code: &str| {
            let table: LuaTable = lua.load(code).eval().unwrap();
            panel_from_table(&table)
        };

        let panel = read(
            r##"{ char_idx = 7, items = {
                { icon = "●", icon_fg = "#ff0000", text = "first" },
                { text = "second" },
            } }"##,
        )
        .unwrap();
        assert_eq!(panel.char_idx, 7);
        assert_eq!(panel.items.len(), 2);
        assert_eq!(panel.items[0].icon, "●");
        assert_eq!(panel.items[0].icon_fg.as_deref(), Some("#ff0000"));
        assert_eq!(panel.items[1].icon, "");
        assert_eq!(panel.items[1].text, "second");
        let defaults = helix_view::annotations::panel::PluginPanel::default();
        assert_eq!(panel.arrow, defaults.arrow);
        assert_eq!(panel.min_width, 30);
        assert!(!panel.show_in_insert);
        assert_eq!(panel.footer, None);

        let panel = read(
            r##"{ char_idx = 0, items = {}, footer = "+2 more", arrow = " < ",
                  under_arrow = "^ ", left_cap = "(", right_cap = ")", min_width = 12,
                  show_in_insert = true, fg = "#000000", bg = "#ffffff", arrow_fg = "#123456" }"##,
        )
        .unwrap();
        assert_eq!(panel.footer.as_deref(), Some("+2 more"));
        assert_eq!(
            (panel.arrow.as_str(), panel.under_arrow.as_str()),
            (" < ", "^ ")
        );
        assert_eq!(
            (panel.left_cap.as_str(), panel.right_cap.as_str()),
            ("(", ")")
        );
        assert_eq!(panel.min_width, 12);
        assert!(panel.show_in_insert);
        assert_eq!(
            read("{ char_idx = 0, items = {}, min_width = 1e9 }")
                .unwrap()
                .min_width,
            65535
        );

        // missing or wrong fields
        assert!(read("{ items = {} }").is_err());
        assert!(read("{ char_idx = 0 }").is_err());
        assert!(read("{ char_idx = 0, items = { { icon = 'x' } } }").is_err());
        assert!(read("{ char_idx = 0, items = {}, min_width = -1 }").is_err());
        assert!(read("{ char_idx = 0, items = {}, show_in_insert = 'yes' }").is_err());
    }

    #[test]
    fn annotation_offset_saturates_but_rejects_wrong_types() {
        let lua = Lua::new();
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
        let offset = |value: &str| -> LuaResult<u16> {
            let table: LuaTable = lua
                .load(format!(
                    "return {{ char_idx = 0, text = 'x', offset = {value} }}"
                ))
                .eval()?;
            LuaPluginAnnotation::from_table(&table).map(|annotation| annotation.offset)
        };
        assert_eq!(offset("nil").unwrap(), 0);
        assert_eq!(offset("12").unwrap(), 12);
        assert_eq!(offset("12.0").unwrap(), 12);
        assert_eq!(offset("'7'").unwrap(), 7);
        assert_eq!(offset("65535").unwrap(), u16::MAX);
        // Columns on lines wider than u16::MAX saturate rather than raise.
        assert_eq!(offset("65536").unwrap(), u16::MAX);
        assert_eq!(offset("100000").unwrap(), u16::MAX);
        assert_eq!(offset("1e300").unwrap(), u16::MAX);
        assert_eq!(offset("math.huge").unwrap(), u16::MAX);
        assert_eq!(offset("math.maxinteger").unwrap(), u16::MAX);
        // Wrong types and non-columns are still errors.
        for wrong in ["true", "{}", "'abc'", "-1", "1.5", "0/0", "-math.huge"] {
            assert!(offset(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn test_lua_position() {
        let pos = LuaPosition { row: 10, col: 5 };
        assert_eq!(pos.row, 10);
        assert_eq!(pos.col, 5);

        let helix_pos: Position = pos.into();
        assert_eq!(helix_pos.row, 10);
        assert_eq!(helix_pos.col, 5);
    }

    #[test]
    fn test_lua_api_registration() {
        let lua = Lua::new();
        lua.set_app_data(crate::permissions::PluginIdentity {
            name: "test".into(),
            permissions: crate::permissions::Permissions::all(),
        });
        let helix_table = lua.create_table().unwrap();

        let result = register_buffer_api(&lua, &helix_table);
        assert!(result.is_ok());

        // Verify buffer module exists
        let buffer_module: LuaTable = helix_table.get("buffer").unwrap();
        assert!(buffer_module.contains_key("get_current").unwrap());
        assert!(buffer_module.contains_key("get_by_id").unwrap());
        assert!(buffer_module.contains_key("list").unwrap());
    }
}
