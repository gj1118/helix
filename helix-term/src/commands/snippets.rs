//! Inserting user-defined snippets: the picker, `:snippet-insert` and the files behind
//! `:snippets-open`. See `helix_view::snippets` for how snippets are defined and loaded.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context as _};
use helix_core::snippets::ActiveSnippet;
use helix_view::{
    document::Mode,
    editor::Action,
    snippets::{SnippetDef, Snippets},
    Editor,
};

use super::Context;
use crate::ui::{overlay::overlaid, Picker, PickerColumn};

/// Insert `def` at every selection of the focused view and enter insert mode so the tabstops
/// can be walked with Tab.
///
/// A selection is replaced by the snippet (`$TM_SELECTED_TEXT` can wrap it), a plain cursor
/// just gets the snippet inserted.
pub fn insert_snippet(editor: &mut Editor, def: &SnippetDef) -> anyhow::Result<()> {
    let snippet = def.parse()?;
    let mode = editor.mode;
    let clipboard = def
        .body
        .contains("CLIPBOARD")
        .then(|| {
            editor
                .registers
                .first('+', editor)
                .map(|text| text.into_owned())
        })
        .flatten();

    let (view, doc) = current!(editor);
    let selection = doc.selection(view.id).clone();
    let text = doc.text().slice(..);
    let mut ctx = doc.snippet_ctx(view.id, mode, clipboard);
    let (transaction, mapped_selection, rendered) = snippet.render(
        doc.text(),
        &selection,
        |range| {
            let len = range.len();
            if len > 1 || (mode == Mode::Select && len >= 1) {
                (range.from(), range.to())
            } else {
                let cursor = range.cursor(text);
                (cursor, cursor)
            }
        },
        &mut ctx,
    );
    let transaction = transaction.with_selection(rendered.first_selection(
        selection.primary().direction(),
        mapped_selection.primary_index(),
    ));

    // Make the snippet its own undo step.
    doc.append_changes_to_history(view);
    doc.apply(&transaction, view.id);
    doc.active_snippet = match doc.active_snippet.take() {
        Some(active) => active.insert_subsnippet(rendered),
        None => ActiveSnippet::new(rendered),
    };
    editor.mode = Mode::Insert;
    Ok(())
}

/// Insert the snippet called `name`.
pub fn insert_snippet_by_name(editor: &mut Editor, name: &str) -> anyhow::Result<()> {
    let snippets = editor.snippets.clone();
    let Some(def) = snippets.get(name) else {
        if snippets.is_empty() {
            bail!("No snippets are defined, see `:snippets-open`");
        }
        bail!("No snippet named \"{name}\"");
    };
    insert_snippet(editor, def)
}

/// Reload the snippets from disk, reporting the result in the status line.
pub fn reload(editor: &mut Editor) {
    match editor.reload_snippets() {
        Ok(count) => editor.set_status(format!(
            "Loaded {count} snippet{}",
            if count == 1 { "" } else { "s" }
        )),
        Err(err) => editor.set_error(format!("Snippets not reloaded: {err:#}")),
    }
}

pub fn snippet_picker(cx: &mut Context) {
    let language = doc!(cx.editor).language_name().map(str::to_owned);
    let snippets: Arc<Snippets> = cx.editor.snippets.clone();
    let defs: Vec<_> = snippets
        .for_language(language.as_deref())
        .cloned()
        .collect();
    if defs.is_empty() {
        cx.editor.set_status(if snippets.is_empty() {
            "No snippets are defined, see `:snippets-open`"
        } else {
            "No snippets for this language"
        });
        return;
    }

    let columns = [
        PickerColumn::new("name", |def: &Arc<SnippetDef>, _: &()| {
            def.name.as_str().into()
        }),
        PickerColumn::new("prefix", |def: &Arc<SnippetDef>, _: &()| {
            def.prefixes.join(", ").into()
        }),
        PickerColumn::new("scope", |def: &Arc<SnippetDef>, _: &()| {
            def.scopes.join(", ").into()
        }),
        PickerColumn::new("description", |def: &Arc<SnippetDef>, _: &()| {
            def.description.clone().unwrap_or_default().into()
        }),
        PickerColumn::new("body", |def: &Arc<SnippetDef>, _: &()| {
            def.body.replace('\n', "↵").into()
        }),
    ];
    let picker = Picker::new(columns, 0, defs, (), |cx, def: &Arc<SnippetDef>, _| {
        if let Err(err) = insert_snippet(cx.editor, def) {
            cx.editor.set_error(format!("{err:#}"));
        }
    })
    .with_title("Snippets");
    cx.push_layer(Box::new(overlaid(
        picker,
        cx.editor.config().fullscreen_overlay,
    )));
}

/// The file `:snippets-open [file]` opens below `base`: `snippets.toml` without an argument,
/// else `snippets/<file>` (with `.toml` added when `file` has no extension).
pub fn snippet_file(base: &Path, file: Option<&str>) -> anyhow::Result<PathBuf> {
    let Some(file) = file else {
        return Ok(base.join("snippets.toml"));
    };
    let relative = Path::new(file);
    if file.is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("`{file}` must be a plain relative path inside the snippets directory");
    }
    let mut path = base.join("snippets").join(relative);
    if path.extension().is_none() {
        path.set_extension("toml");
    }
    Ok(path)
}

/// Open a snippet file below `base`, creating the directory it lives in.
pub fn open_snippet_file(
    editor: &mut Editor,
    base: &Path,
    file: Option<&str>,
) -> anyhow::Result<()> {
    let path = snippet_file(base, file)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    editor.open(&path, Action::Replace)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_file_paths() {
        let base = Path::new("/cfg");
        assert_eq!(
            snippet_file(base, None).unwrap(),
            Path::new("/cfg/snippets.toml")
        );
        assert_eq!(
            snippet_file(base, Some("rust")).unwrap(),
            Path::new("/cfg/snippets/rust.toml")
        );
        assert_eq!(
            snippet_file(base, Some("lang/rust.toml")).unwrap(),
            Path::new("/cfg/snippets/lang/rust.toml")
        );
        for bad in ["", "../x", "/etc/passwd", "a/../../b"] {
            assert!(snippet_file(base, Some(bad)).is_err(), "{bad}");
        }
    }
}
