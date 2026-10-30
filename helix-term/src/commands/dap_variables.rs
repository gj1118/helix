//! The variable browser: scopes and variables of a stack frame (or the result of an evaluated
//! expression) shown in a picker that can be navigated into and back out of.
//!
//! Every level is a picker. Selecting a variable with children closes the picker and opens the
//! next level; `.. (back)` reopens the parent with the filter and the selection it was left
//! with. Variables with many indexed children are paged: `Load more…` fetches the next page.

use std::sync::{Arc, Mutex};

use helix_dap::{registry::DebugAdapterId, Scope, Variable};
use helix_view::Editor;

use crate::{
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{overlay::overlaid, picker::PickerState, Picker, PickerColumn},
};

/// How many indexed children are requested at a time.
pub const PAGE: usize = 100;

#[derive(Debug, Clone)]
pub enum Item {
    Scope(Scope),
    Variable(Variable),
    Back,
    More { remaining: usize },
}

impl Item {
    fn name(&self) -> String {
        match self {
            Item::Scope(scope) => format!("▸ {}", scope.name),
            Item::Variable(var) if var.variables_reference > 0 => format!("▸ {}", var.name),
            Item::Variable(var) => format!("  {}", var.name),
            Item::Back => ".. (back)".to_owned(),
            Item::More { .. } => "Load more…".to_owned(),
        }
    }

    fn value(&self) -> String {
        match self {
            Item::Scope(scope) if scope.expensive => "(expensive)".to_owned(),
            Item::Scope(_) => String::new(),
            Item::Variable(var) => var.value.clone(),
            Item::Back => String::new(),
            Item::More { remaining } => format!("{remaining} more"),
        }
    }

    fn ty(&self) -> String {
        match self {
            Item::Variable(var) => var.ty.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// What a level lists.
#[derive(Debug, Clone)]
enum Kind {
    /// The scopes of a frame.
    Scopes,
    /// The children of a variable.
    Children {
        reference: usize,
        /// How many indexed children there are, if the adapter said so.
        indexed: usize,
    },
}

#[derive(Debug, Clone)]
struct Level {
    kind: Kind,
    items: Vec<Item>,
    /// How many indexed children were loaded so far (for paged levels).
    loaded_indexed: usize,
    paged: bool,
    /// The filter and selection to restore when the level is shown again.
    state: PickerState,
}

impl Level {
    /// The rows of the picker: a way back, the items, and a way to load more of them.
    fn rows(&self, depth: usize) -> Vec<Item> {
        let mut rows = Vec::with_capacity(self.items.len() + 2);
        if depth > 0 {
            rows.push(Item::Back);
        }
        rows.extend(self.items.iter().cloned());
        if let Kind::Children { indexed, .. } = self.kind {
            if self.paged && self.loaded_indexed < indexed {
                rows.push(Item::More {
                    remaining: indexed - self.loaded_indexed,
                });
            }
        }
        rows
    }
}

struct Browser {
    session: DebugAdapterId,
    levels: Vec<Level>,
}

type Shared = Arc<Mutex<Browser>>;

fn lock(browser: &Shared) -> std::sync::MutexGuard<'_, Browser> {
    browser
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The future that loads the first page of the children of a variable.
fn load_children(
    editor: &Editor,
    session: DebugAdapterId,
    reference: usize,
    named: usize,
    indexed: usize,
) -> anyhow::Result<impl std::future::Future<Output = helix_dap::Result<Vec<Variable>>>> {
    let client = editor
        .debug_adapters
        .get_client(session)
        .ok_or_else(|| anyhow::anyhow!("The debug session has ended"))?;
    Ok(if indexed > PAGE {
        // Named children (if any) first, then the first page of the indexed ones.
        let named_call =
            (named > 0).then(|| client.variables_page(reference, Some("named"), None, None));
        let indexed_call = client.variables_page(reference, Some("indexed"), Some(0), Some(PAGE));
        futures_util::future::Either::Left(async move {
            let mut variables = match named_call {
                Some(call) => call.await?,
                None => Vec::new(),
            };
            variables.extend(indexed_call.await?);
            Ok(variables)
        })
    } else {
        futures_util::future::Either::Right(client.variables_page(reference, None, None, None))
    })
}

fn new_level(reference: usize, named: usize, indexed: usize, variables: Vec<Variable>) -> Level {
    let paged = indexed > PAGE;
    let loaded_indexed = if paged {
        variables
            .iter()
            .filter(|var| var.name.starts_with('['))
            .count()
            .min(PAGE)
    } else {
        0
    };
    let _ = named;
    Level {
        kind: Kind::Children { reference, indexed },
        items: variables.into_iter().map(Item::Variable).collect(),
        loaded_indexed,
        paged,
        state: PickerState::default(),
    }
}

/// Open the browser on the scopes of the selected stack frame.
pub fn open_frame(editor: &mut Editor, jobs: &mut Jobs) {
    let Some(client) = editor.debug_adapters.get_active_client() else {
        editor.set_status("No debug session");
        return;
    };
    let (Some(thread_id), Some(frame)) = (client.thread_id, client.active_frame) else {
        editor.set_status("Cannot access variables while target is running.");
        return;
    };
    let Some(frame_id) = client
        .stack_frames
        .get(&thread_id)
        .and_then(|frames| frames.get(frame))
        .map(|frame| frame.id)
    else {
        editor.set_error("Cannot find the current stack frame to access variables.");
        return;
    };
    let session = client.id();
    let scopes = client.scopes_future(frame_id);
    jobs.callback(async move {
        let scopes = scopes.await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                let level = Level {
                    kind: Kind::Scopes,
                    items: scopes.into_iter().map(Item::Scope).collect(),
                    loaded_indexed: 0,
                    paged: false,
                    state: PickerState::default(),
                };
                let browser = Arc::new(Mutex::new(Browser {
                    session,
                    levels: vec![level],
                }));
                show(editor, compositor, browser);
            },
        )))
    });
}

/// Open the browser on the children of an evaluated expression. This is meant to be called from
/// a callback that has the editor but no job queue, so it spawns the request itself.
pub fn open_result(editor: &mut Editor, reference: usize, named: usize, indexed: usize) {
    let Some(session) = editor.debug_adapters.get_active_client().map(|c| c.id()) else {
        return;
    };
    let load = match load_children(editor, session, reference, named, indexed) {
        Ok(load) => load,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };
    tokio::spawn(async move {
        match load.await {
            Ok(variables) => {
                crate::job::dispatch(move |editor: &mut Editor, compositor: &mut Compositor| {
                    let level = new_level(reference, named, indexed, variables);
                    let browser = Arc::new(Mutex::new(Browser {
                        session,
                        levels: vec![level],
                    }));
                    show(editor, compositor, browser);
                })
                .await
            }
            Err(err) => {
                crate::job::dispatch(move |editor: &mut Editor, _: &mut Compositor| {
                    editor.set_error(format!("Failed to load variables: {err}"));
                })
                .await
            }
        }
    });
}

/// Show the picker for the innermost level.
fn show(editor: &mut Editor, compositor: &mut Compositor, browser: Shared) {
    let (rows, state) = {
        let browser = lock(&browser);
        let depth = browser.levels.len() - 1;
        let level = &browser.levels[depth];
        (level.rows(depth), level.state.clone())
    };

    let columns = [
        PickerColumn::new("name", |item: &Item, _: &()| item.name().into()),
        PickerColumn::new("value", |item: &Item, _: &()| item.value().into()),
        PickerColumn::new("type", |item: &Item, _: &()| item.ty().into()),
    ];
    let tracker = Arc::new(Mutex::new(state.clone()));
    let callback = {
        let browser = browser.clone();
        let tracker = tracker.clone();
        move |cx: &mut compositor::Context, item: &Item, _action| {
            let state = tracker
                .lock()
                .map(|state| state.clone())
                .unwrap_or_default();
            select(cx, &browser, item, state);
        }
    };
    let picker = Picker::new(columns, 0, rows, (), callback)
        .with_query(&state.query, editor)
        .with_cursor(state.cursor)
        .with_state_tracker(tracker)
        .with_title("Variables");
    compositor.push(Box::new(overlaid(
        picker,
        editor.config().fullscreen_overlay,
    )));
}

/// Queue showing the browser again (after the picker that called us closed).
fn reshow(jobs: &mut Jobs, browser: Shared) {
    jobs.callback(async move {
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                show(editor, compositor, browser)
            },
        )))
    });
}

fn select(cx: &mut compositor::Context, browser: &Shared, item: &Item, state: PickerState) {
    // Remember the filter and selection of the level we are leaving or reloading.
    let session = {
        let mut guard = lock(browser);
        if let Some(level) = guard.levels.last_mut() {
            level.state = state;
        }
        guard.session
    };

    match item {
        Item::Back => {
            lock(browser).levels.pop();
            reshow(cx.jobs, browser.clone());
        }
        Item::Scope(scope) => {
            let (reference, named, indexed) = (
                scope.variables_reference,
                scope.named_variables.unwrap_or(0),
                scope.indexed_variables.unwrap_or(0),
            );
            descend(cx, browser, session, reference, named, indexed);
        }
        Item::Variable(var) if var.variables_reference > 0 => {
            descend(
                cx,
                browser,
                session,
                var.variables_reference,
                var.named_variables.unwrap_or(0),
                var.indexed_variables.unwrap_or(0),
            );
        }
        // Nothing to open: stay where we are.
        Item::Variable(_) => reshow(cx.jobs, browser.clone()),
        Item::More { .. } => load_more(cx, browser, session),
    }
}

fn descend(
    cx: &mut compositor::Context,
    browser: &Shared,
    session: DebugAdapterId,
    reference: usize,
    named: usize,
    indexed: usize,
) {
    let load = match load_children(cx.editor, session, reference, named, indexed) {
        Ok(load) => load,
        Err(err) => {
            cx.editor.set_error(err.to_string());
            return;
        }
    };
    let browser = browser.clone();
    cx.jobs.callback(async move {
        let variables = load.await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                lock(&browser)
                    .levels
                    .push(new_level(reference, named, indexed, variables));
                show(editor, compositor, browser);
            },
        )))
    });
}

fn load_more(cx: &mut compositor::Context, browser: &Shared, session: DebugAdapterId) {
    let (reference, start) = {
        let guard = lock(browser);
        match guard.levels.last() {
            Some(Level {
                kind: Kind::Children { reference, .. },
                loaded_indexed,
                ..
            }) => (*reference, *loaded_indexed),
            _ => return,
        }
    };
    let Some(client) = cx.editor.debug_adapters.get_client(session) else {
        cx.editor.set_error("The debug session has ended");
        return;
    };
    let page = client.variables_page(reference, Some("indexed"), Some(start), Some(PAGE));
    let browser = browser.clone();
    cx.jobs.callback(async move {
        let variables = page.await?;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                {
                    let mut guard = lock(&browser);
                    if let Some(level) = guard.levels.last_mut() {
                        level.loaded_indexed += variables.len();
                        level
                            .items
                            .extend(variables.into_iter().map(Item::Variable));
                    }
                }
                show(editor, compositor, browser);
            },
        )))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(name: &str, reference: usize) -> Variable {
        Variable {
            name: name.to_owned(),
            value: "v".to_owned(),
            ty: None,
            presentation_hint: None,
            evaluate_name: None,
            variables_reference: reference,
            named_variables: None,
            indexed_variables: None,
            memory_reference: None,
        }
    }

    #[test]
    fn rows_have_back_and_load_more() {
        let variables: Vec<_> = (0..PAGE).map(|i| variable(&format!("[{i}]"), 0)).collect();
        let level = new_level(4, 0, 250, variables);
        assert_eq!(level.loaded_indexed, PAGE);
        let rows = level.rows(1);
        assert!(matches!(rows[0], Item::Back));
        assert!(matches!(rows.last(), Some(Item::More { remaining: 150 })));
        assert_eq!(rows.len(), PAGE + 2);
        // the root level has no way back
        assert!(!matches!(level.rows(0)[0], Item::Back));

        // everything loaded: no "Load more…"
        let mut full = level.clone();
        full.loaded_indexed = 250;
        assert!(!matches!(full.rows(0).last(), Some(Item::More { .. })));
        // unpaged levels never have one
        let small = new_level(3, 2, 2, vec![variable("x", 0), variable("y", 0)]);
        assert!(!small.paged);
        assert_eq!(small.rows(0).len(), 2);
    }

    #[test]
    fn labels_mark_expandable_entries() {
        assert_eq!(Item::Variable(variable("obj", 3)).name(), "▸ obj");
        assert_eq!(Item::Variable(variable("x", 0)).name(), "  x");
        assert_eq!(Item::Back.name(), ".. (back)");
        assert_eq!(Item::More { remaining: 7 }.value(), "7 more");
    }
}
