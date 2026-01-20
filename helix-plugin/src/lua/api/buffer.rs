use crate::error::Result;
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

impl LuaUserData for LuaBuffer {
    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // Get buffer text
        methods.add_method("get_text", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.text().to_string())
        });

        // Get buffer length
        methods.add_method("len", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.text().len_chars())
        });

        // Get buffer line count
        methods.add_method("line_count", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.text().len_lines())
        });

        // Get line at index (1-based)
        methods.add_method("get_line", |_lua, this, line_num: usize| {
            if line_num == 0 {
                return Err(LuaError::RuntimeError(
                    "Line numbers are 1-based (must be >= 1)".to_string(),
                ));
            }
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;

            let line_idx = line_num - 1;
            if line_idx >= doc.text().len_lines() {
                return Err(LuaError::RuntimeError(format!(
                    "Line number {} out of bounds (max {})",
                    line_num,
                    doc.text().len_lines()
                )));
            }

            Ok(doc.text().line(line_idx).to_string())
        });

        // Get buffer path
        methods.add_method("get_path", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.path().map(|p| p.to_string_lossy().to_string()))
        });

        // Get document ID
        methods.add_method("id", |_lua, this, ()| Ok(format!("{:?}", this.document_id)));

        // Check if buffer is modified
        methods.add_method("is_modified", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.is_modified())
        });

        // Get buffer language
        methods.add_method("get_language", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc.language_name().map(|s| s.to_string()))
        });

        // Insert text at position
        methods.add_method(
            "insert",
            |_lua, this, (line, col, text): (usize, usize, String)| {
                let editor = crate::lua::get_editor_mut()?;
                let (view, doc) = helix_view::current!(editor);

                // For now, only support current doc
                if doc.id() != this.document_id {
                    return Err(LuaError::RuntimeError(
                        "Modifications currently only supported for the active buffer.".to_string(),
                    ));
                }

                let text_rope = doc.text();
                let row = (line.saturating_sub(1)).min(text_rope.len_lines().saturating_sub(1));
                let line_start = text_rope.line_to_char(row);
                let line_len = text_rope.line(row).len_chars();
                let offset = line_start + col.min(line_len);

                let transaction = helix_core::Transaction::change(
                    text_rope,
                    std::iter::once((offset, offset, Some(text.into()))),
                );
                doc.apply(&transaction, view.id);

                Ok(())
            },
        );

        // Delete range
        methods.add_method(
            "delete",
            |_lua,
             this,
             (start_line, start_col, end_line, end_col): (usize, usize, usize, usize)| {
                let editor = crate::lua::get_editor_mut()?;
                let (view, doc) = helix_view::current!(editor);

                // For now, only support current doc
                if doc.id() != this.document_id {
                    return Err(LuaError::RuntimeError(
                        "Modifications currently only supported for the active buffer.".to_string(),
                    ));
                }

                let text_rope = doc.text();

                let start_row =
                    (start_line.saturating_sub(1)).min(text_rope.len_lines().saturating_sub(1));
                let start_offset = text_rope.line_to_char(start_row)
                    + start_col.min(text_rope.line(start_row).len_chars());

                let end_row =
                    (end_line.saturating_sub(1)).min(text_rope.len_lines().saturating_sub(1));
                let end_offset = text_rope.line_to_char(end_row)
                    + end_col.min(text_rope.line(end_row).len_chars());

                let transaction = helix_core::Transaction::change(
                    text_rope,
                    std::iter::once((start_offset, end_offset, None)),
                );
                doc.apply(&transaction, view.id);

                Ok(())
            },
        );

        // Get selections
        methods.add_method("get_selections", |lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;

            let (view, current_doc): (&helix_view::View, &helix_view::Document) =
                helix_view::current_ref!(editor);
            if current_doc.id() != this.document_id {
                return lua.create_table();
            }

            let selection = doc.selection(view.id);
            let selections = lua.create_table()?;
            for (i, range) in selection.iter().enumerate() {
                let s = lua.create_table()?;
                s.set("anchor", range.anchor)?;
                s.set("head", range.head)?;
                selections.set(i + 1, s)?;
            }
            Ok(selections)
        });

        // Get diagnostics
        methods.add_method("get_diagnostics", |lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;

            let diagnostics = lua.create_table()?;
            for (i, diag) in doc.diagnostics().iter().enumerate() {
                diagnostics.set(i + 1, LuaDiagnostic::from(diag.clone()))?;
            }
            Ok(diagnostics)
        });

        // Set annotations
        methods.add_method(
            "set_annotations",
            |_lua, this, annotations: Vec<LuaPluginAnnotation>| {
                let editor = crate::lua::get_editor_mut()?;
                let (view, doc) = helix_view::current!(editor);

                // For now, only support current doc
                if doc.id() != this.document_id {
                    return Err(LuaError::RuntimeError(
                        "Annotations currently only supported for the active buffer.".to_string(),
                    ));
                }

                let plugin_annots: Vec<helix_view::document::PluginAnnotation> = annotations
                    .into_iter()
                    .map(|a| helix_view::document::PluginAnnotation {
                        char_idx: a.char_idx,
                        text: a.text,
                        style: a.style,
                        fg: a.fg,
                        bg: a.bg,
                        offset: a.offset,
                        is_line: a.is_line,
                    })
                    .collect();

                doc.plugin_annotations.insert(view.id, plugin_annots);
                Ok(())
            },
        );

        // Get cursor position (char index)
        methods.add_method("get_cursor", |_lua, this, ()| {
            let editor = crate::lua::get_editor_mut()?;
            let (view, doc) = helix_view::current!(editor);
            if doc.id() != this.document_id {
                return Err(LuaError::RuntimeError(
                    "Current view is not showing this buffer".into(),
                ));
            }
            let cursor = doc
                .selection(view.id)
                .primary()
                .cursor(doc.text().slice(..));
            Ok(cursor)
        });

        // Convert char index to line index (0-based)
        methods.add_method("char_to_line", |_lua, this, char_idx: usize| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc
                .text()
                .char_to_line(char_idx.min(doc.text().len_chars())))
        });

        // Convert line index to char index (0-based)
        methods.add_method("line_to_char", |_lua, this, line_idx: usize| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            Ok(doc
                .text()
                .line_to_char(line_idx.min(doc.text().len_lines())))
        });

        // Get visual column for char index
        methods.add_method("get_visual_column", |_lua, this, char_idx: usize| {
            let editor = crate::lua::get_editor_mut()?;
            let doc = editor.document(this.document_id).ok_or_else(|| {
                LuaError::RuntimeError(format!("Buffer {:?} no longer exists", this.document_id))
            })?;
            let text = doc.text();
            let line_idx = text.char_to_line(char_idx);
            let line_start = text.line_to_char(line_idx);
            let tab_width = doc.tab_width();

            let mut column = 0;
            for ch in text.slice(line_start..char_idx).chars() {
                if ch == '\t' {
                    column += tab_width - (column % tab_width);
                } else {
                    column += helix_core::unicode::width::UnicodeWidthChar::width(ch).unwrap_or(1);
                }
            }
            Ok(column)
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
    }

    fn add_methods<'lua, M: LuaUserDataMethods<Self>>(_methods: &mut M) {}
}

impl FromLua for LuaPluginAnnotation {
    fn from_lua(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::Table(table) => Ok(LuaPluginAnnotation {
                char_idx: table.get("char_idx")?,
                text: table.get("text")?,
                style: table.get("style").ok(),
                fg: table.get("fg").ok(),
                bg: table.get("bg").ok(),
                offset: table.get("offset").unwrap_or(0),
                is_line: table.get("is_line").unwrap_or(false),
            }),
            LuaValue::UserData(ud) => ud.borrow::<Self>().map(|s| s.clone()),
            _ => Err(LuaError::FromLuaConversionError {
                from: "LuaValue",
                to: "LuaPluginAnnotation".to_string(),
                message: Some("Expected UserData".to_string()),
            }),
        }
    }
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
    let get_current = lua.create_function(|_lua, ()| {
        let editor = crate::lua::get_editor_mut()?;
        let (_view, doc): (&helix_view::View, &helix_view::Document) =
            helix_view::current_ref!(editor);
        Ok(LuaBuffer::new(doc.id()))
    })?;
    buffer_module.set("get_current", get_current)?;

    // helix.buffer.get_by_id(id) - Get buffer by ID
    let get_by_id = lua.create_function(|_lua, _id: String| {
        // TODO: Implement actual buffer lookup
        Ok(LuaValue::Nil)
    })?;
    buffer_module.set("get_by_id", get_by_id)?;

    // helix.buffer.list() - List all buffers
    let list = lua.create_function(|lua, ()| {
        let editor = crate::lua::get_editor_mut()?;
        let buffers = lua.create_table()?;
        for (i, (&id, _)) in editor.documents.iter().enumerate() {
            buffers.set(i + 1, LuaBuffer::new(id))?;
        }
        Ok(buffers)
    })?;
    buffer_module.set("list", list)?;

    // helix.buffer.annotation(table) - Create a new annotation
    let annotation = lua.create_function(|_lua, table: LuaTable| {
        Ok(LuaPluginAnnotation {
            char_idx: table.get("char_idx")?,
            text: table.get("text")?,
            style: table.get("style").ok(),
            fg: table.get("fg").ok(),
            bg: table.get("bg").ok(),
            offset: table.get("offset").unwrap_or(0),
            is_line: table.get("is_line").unwrap_or(false),
        })
    })?;
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
