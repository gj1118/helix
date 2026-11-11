use crate::error::Result;
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
                let mut plugin_annots: Vec<helix_view::document::PluginAnnotation> = annotations
                    .into_iter()
                    .map(|a| helix_view::document::PluginAnnotation {
                        char_idx: a.char_idx,
                        text: a.text,
                        style: a.style,
                        fg: a.fg,
                        bg: a.bg,
                        offset: a.offset,
                        is_line: a.is_line,
                        virt_line_idx: a.virt_line_idx,
                        dropped_text: a.dropped_text,
                    })
                    .collect();
                // The renderers binary-search annotations by char_idx; the sort is stable
                // so annotations at the same position keep the plugin's order.
                plugin_annots.sort_by_key(|a| a.char_idx);

                with_focused_doc(this.document_id, "Annotations", |view, doc| {
                    doc.plugin_annotations.insert(view.id, plugin_annots);
                    Ok(())
                })
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
        })
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
