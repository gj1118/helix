use crate::editor::{Action, Breakpoint};
use crate::{align_view, Align, Editor};
use anyhow::bail;
use dap::requests::DisconnectArguments;
use futures_util::future::Either;
use helix_core::Selection;
use helix_dap::{
    self as dap, registry::DebugAdapterId, Client, ConnectionType, Payload, Request, ThreadId,
};
use log::{error, warn};
use serde_json::{json, Value};
use std::fmt::Write;
use std::path::PathBuf;

#[macro_export]
macro_rules! debugger {
    ($editor:expr) => {{
        let Some(debugger) = $editor.debug_adapters.get_active_client_mut() else {
            return;
        };
        debugger
    }};
}

// general utils:
/// The char index of a (one based) DAP position. A line past the end of the text is its last
/// line, a column past the end of its line the end of the line (the file may have changed since
/// the adapter read it). `None` for line 0, which adapters send for frames without a position.
pub fn dap_pos_to_pos(doc: &helix_core::Rope, line: usize, column: usize) -> Option<usize> {
    // 1-indexing to 0 indexing
    let line = line.checked_sub(1)?.min(doc.len_lines().saturating_sub(1));
    let start = doc.try_line_to_char(line).ok()?;
    let end = helix_core::line_ending::line_end_char_index(&doc.slice(..), line);
    // TODO: this is probably utf-16 offsets
    Some((start + column.saturating_sub(1)).min(end))
}

/// Makes `thread_id` the current thread of the selected session. Its stack is fetched in the
/// background and shown when it arrives (if the thread is still the current one by then).
pub fn switch_thread(editor: &mut Editor, thread_id: ThreadId) {
    let Some(id) = editor.debug_adapters.active_id() else {
        return;
    };
    let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
        return;
    };
    debugger.thread_id = Some(thread_id);
    debugger.active_frame = Some(0);
    let page = debugger.stack_trace_page(thread_id, 0, STACK_FRAMES_PAGE);
    // The values are those of the thread that was current.
    editor.clear_dap_inline_values();
    spawn_job(editor, id, async move {
        let page = page.await;
        Some(Box::new(move |editor: &mut Editor| {
            let page = page.map(|(frames, total)| (with_open_buffer_paths(editor, frames), total));
            let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
                return;
            };
            store_stack_trace(debugger, thread_id, page);
            if debugger.thread_id == Some(thread_id)
                && editor.debug_adapters.active_id() == Some(id)
            {
                show_current_frame(editor);
            }
        }) as Job)
    });
}

/// The session `id` stopped: fetches the stack of the thread that stopped (of all threads, when
/// they all stopped) in the background, then makes that thread current, unless the session has
/// a current thread already, and shows where it stopped.
fn fetch_stop(
    editor: &mut Editor,
    id: DebugAdapterId,
    thread_id: Option<ThreadId>,
    all_threads_stopped: bool,
) {
    let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
        return;
    };
    if !all_threads_stopped && thread_id.is_none() {
        return;
    }
    debugger.stops += 1;
    let stop = debugger.stops;
    let requester = debugger.requester();
    spawn_job(editor, id, async move {
        let threads: Vec<ThreadId> = match thread_id {
            Some(thread_id) if !all_threads_stopped => vec![thread_id],
            _ => match requester.threads().await {
                Ok(threads) => threads.into_iter().map(|thread| thread.id).collect(),
                Err(err) => {
                    warn!("Failed to fetch the threads of a stopped session: {err}");
                    return None;
                }
            },
        };
        let pages = futures_util::future::join_all(
            threads
                .iter()
                .map(|&thread| requester.stack_trace_page(thread, 0, STACK_FRAMES_PAGE)),
        )
        .await;
        Some(Box::new(move |editor: &mut Editor| {
            let pages: Vec<_> = pages
                .into_iter()
                .map(|page| {
                    page.map(|(frames, total)| (with_open_buffer_paths(editor, frames), total))
                })
                .collect();
            let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
                return;
            };
            // It stopped again since: that stop's stacks count.
            if debugger.stops != stop {
                return;
            }
            for (&thread, page) in threads.iter().zip(pages) {
                store_stack_trace(debugger, thread, page);
            }
            let Some(thread) = thread_id.or_else(|| threads.first().copied()) else {
                return;
            };
            // It runs again by now (the user continued, or the adapter did).
            if debugger
                .thread_states
                .get(&thread)
                .is_some_and(|state| state == "running")
            {
                return;
            }
            select_stopped_thread(editor, id, thread);
        }) as Job)
    });
}

/// Makes `thread_id` (whose stack was fetched) the current thread of the session `id` and, when
/// that session is the selected one, shows where it stopped. A session that has a current
/// thread keeps it.
fn select_stopped_thread(editor: &mut Editor, id: DebugAdapterId, thread_id: ThreadId) {
    let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
        return;
    };

    if debugger.thread_id.is_some() {
        return;
    }

    debugger.thread_id = Some(thread_id);
    debugger.active_frame = Some(0);

    // A session that stops while the selected one is running takes over: that is where the
    // attention is needed.
    let selected_is_stopped = editor
        .debug_adapters
        .get_active_client()
        .is_some_and(|selected| selected.state_label() == "stopped");
    if editor.debug_adapters.active_id() != Some(id) && !selected_is_stopped {
        editor.debug_adapters.set_active_client(id);
    }
    if editor.debug_adapters.active_id() != Some(id) {
        return;
    }

    show_current_frame(editor);
}

/// Jumps to the selected frame of the selected session and shows its inline values.
pub fn show_current_frame(editor: &mut Editor) {
    let frame = editor
        .debug_adapters
        .get_active_client()
        .and_then(|debugger| debugger.current_stack_frame().cloned());
    if let Some(frame) = &frame {
        jump_to_stack_frame(editor, frame);
    }
    refresh_inline_values(editor);
}

/// How many lines above the current line of the stack frame get inline values.
const INLINE_VALUES_LINES: usize = 40;
/// How many values are shown at the end of one line.
const INLINE_VALUES_PER_LINE: usize = 4;
/// Values longer than this many characters are cut.
const INLINE_VALUE_WIDTH: usize = 24;
/// An adapter that takes longer than this for the variables gets no inline values.
const INLINE_VALUES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Shows the values of the variables the lines around the selected stack frame use at the end
/// of those lines. Best effort: any problem leaves the lines without values. The variables are
/// requested in the background; values that arrive after the selected frame changed are dropped.
pub fn refresh_inline_values(editor: &mut Editor) {
    editor.clear_dap_inline_values();
    // Values for an earlier stop (even with a reused frame id) are dropped on arrival.
    let generation = editor.dap_inline_values_generation;
    if !editor.config().debugger.inline_values {
        return;
    }
    let Some(session) = editor.debug_adapters.active_id() else {
        return;
    };
    let Some(debugger) = editor.debug_adapters.get_active_client() else {
        return;
    };
    let Some(thread_id) = debugger.thread_id else {
        return;
    };
    let Some(frame) = debugger.current_stack_frame().cloned() else {
        return;
    };
    let Some(path) = frame.source.as_ref().and_then(|source| source.path.clone()) else {
        return;
    };

    let requester = debugger.requester();
    let fetch = async move {
        let mut variables = Vec::new();
        for scope in requester.scopes(frame.id).await? {
            // Expensive scopes (globals, registers) are only loaded on request.
            if scope.expensive {
                continue;
            }
            variables.extend(requester.variables(scope.variables_reference).await?);
        }
        Ok::<_, helix_dap::Error>(variables)
    };
    spawn_job(editor, session, async move {
        let Ok(Ok(variables)) = tokio::time::timeout(INLINE_VALUES_TIMEOUT, fetch).await else {
            return None;
        };
        let variables: Vec<(String, String)> = variables
            .into_iter()
            .map(|variable| (variable.name, variable.value))
            .collect();
        Some(Box::new(move |editor: &mut Editor| {
            // Stale: refreshed or cleared since, or another session, thread or frame
            // is shown by now.
            let current = editor.dap_inline_values_generation == generation
                && editor.debug_adapters.active_id() == Some(session)
                && editor
                    .debug_adapters
                    .get_active_client()
                    .is_some_and(|debugger| {
                        debugger.thread_id == Some(thread_id)
                            && debugger
                                .current_stack_frame()
                                .is_some_and(|current| current.id == frame.id)
                    });
            if !current {
                return;
            }
            if let Some(doc) = editor.document_by_path_mut(&path) {
                let line = frame.line.saturating_sub(1);
                doc.dap_inline_values = inline_value_annotations(doc.text(), line, &variables);
            }
        }) as Job)
    });
}

/// Work the DAP handler finished in the background, applied to the editor on its own task.
type Job = Box<dyn FnOnce(&mut Editor) + Send>;

/// The name of the events that carry [`Job`]s (see [`spawn_job`]). No adapter sends it.
const JOB_EVENT: &str = "helix/job";

/// Jobs waiting for their event to be handled, by the number the event carries.
static JOBS: std::sync::Mutex<Vec<(u64, Job)>> = std::sync::Mutex::new(Vec::new());
static NEXT_JOB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Runs `future` in the background and applies the job it returns, if any, to the editor. The
/// job comes back like the messages of the adapters do (as an event on
/// `editor.debug_adapters.incoming`), so it runs on the editor's task, in order with them.
fn spawn_job<F>(editor: &mut Editor, session: DebugAdapterId, future: F)
where
    F: std::future::Future<Output = Option<Job>> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    editor
        .debug_adapters
        .incoming
        .push(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
    tokio::spawn(async move {
        // When there is nothing to do the channel closes and the stream goes away.
        let Some(job) = future.await else {
            return;
        };
        let key = NEXT_JOB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        JOBS.lock().unwrap().push((key, job));
        let event = Client::internal_event(JOB_EVENT, json!(key));
        if tx.send((session, event)).is_err() {
            // The editor is gone.
            JOBS.lock().unwrap().retain(|(pending, _)| *pending != key);
        }
    });
}

/// Takes the job an event of [`spawn_job`] carries.
fn take_job(body: Option<&Value>) -> Option<Job> {
    let key = body?.as_u64()?;
    let mut jobs = JOBS.lock().unwrap();
    let index = jobs.iter().position(|(pending, _)| *pending == key)?;
    Some(jobs.swap_remove(index).1)
}

/// The text shown at the end of the lines up to `line` (zero based): `name = value` for the
/// variables the line mentions, the innermost scope's value when a name is in several. This
/// matches names as words, without parsing the language, so a name in a comment or a string
/// counts too.
pub fn inline_value_annotations(
    text: &helix_core::Rope,
    line: usize,
    variables: &[(String, String)],
) -> Vec<helix_core::text_annotations::InlineAnnotation> {
    use helix_core::text_annotations::InlineAnnotation;

    let mut values: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for (name, value) in variables {
        // "Special variables", "Function variables" and the like are not names in the source.
        if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            values.entry(name.as_str()).or_insert(value.as_str());
        }
    }
    if values.is_empty() || text.len_lines() == 0 {
        return Vec::new();
    }

    let last = line.min(text.len_lines() - 1);
    let first = last.saturating_sub(INLINE_VALUES_LINES);
    let mut annotations = Vec::new();
    for index in first..=last {
        let content = text.line(index);
        let line_text: String = content.chars().collect();
        let line_text = line_text.trim_end_matches(['\n', '\r']);

        let mut shown: Vec<&str> = Vec::new();
        for word in line_text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if values.contains_key(word) && !shown.contains(&word) {
                shown.push(word);
            }
        }
        if shown.is_empty() {
            continue;
        }
        let rendered = shown
            .iter()
            .take(INLINE_VALUES_PER_LINE)
            .map(|name| format!("{name} = {}", short_value(values[name])))
            .collect::<Vec<_>>()
            .join(", ");
        let end = text.line_to_char(index) + line_text.chars().count();
        annotations.push(InlineAnnotation::new(end, format!("  {rendered}")));
    }
    annotations
}

fn short_value(value: &str) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > INLINE_VALUE_WIDTH {
        let cut: String = value.chars().take(INLINE_VALUE_WIDTH - 1).collect();
        format!("{cut}…")
    } else {
        value
    }
}

#[cfg(test)]
mod dap_pos_tests {
    use super::dap_pos_to_pos;
    use helix_core::Rope;

    #[test]
    fn positions_stay_inside_the_text() {
        let text = Rope::from("ab\ncd\n");
        assert_eq!(dap_pos_to_pos(&text, 2, 2), Some(4));
        // past the end of the line: the end of the line, not the next one
        assert_eq!(dap_pos_to_pos(&text, 1, 40), Some(2));
        // past the last line: the last line
        assert_eq!(dap_pos_to_pos(&text, 9, 1), Some(6));
        // line 0: no position (used to underflow)
        assert_eq!(dap_pos_to_pos(&text, 0, 1), None);
        assert_eq!(dap_pos_to_pos(&Rope::new(), 1, 1), Some(0));
    }
}

#[cfg(test)]
mod inline_value_tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn rendered(text: &str, line: usize, variables: &[(&str, &str)]) -> Vec<(usize, String)> {
        let text = helix_core::Rope::from_str(text);
        inline_value_annotations(&text, line, &vars(variables))
            .into_iter()
            .map(|a| (a.char_idx, a.text.to_string()))
            .collect()
    }

    #[test]
    fn values_go_to_the_end_of_the_lines_that_use_them() {
        let out = rendered(
            "a = 1\nb = a + c\nprint(b)\n",
            2,
            &[("a", "1"), ("b", "3"), ("c", "2")],
        );
        assert_eq!(
            out,
            [
                (5, "  a = 1".to_string()),
                (15, "  b = 3, a = 1, c = 2".to_string()),
                (24, "  b = 3".to_string()),
            ]
        );
    }

    #[test]
    fn names_match_as_whole_words_only() {
        assert!(rendered("answers = 1\n", 0, &[("answer", "42")]).is_empty());
        assert_eq!(rendered("x.answer\n", 0, &[("answer", "42")]).len(), 1);
    }

    #[test]
    fn lines_after_the_current_one_and_special_scopes_are_skipped() {
        let out = rendered("a\na\na\n", 1, &[("a", "1"), ("Special variables", "x")]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_innermost_value_wins_and_long_values_are_cut() {
        let out = rendered("a\n", 0, &[("a", "inner"), ("a", "outer")]);
        assert_eq!(out[0].1, "  a = inner");
        let long = "x".repeat(100);
        let out = rendered("a\n", 0, &[("a", &long)]);
        assert_eq!(
            out[0].1.chars().count(),
            "  a = ".len() + INLINE_VALUE_WIDTH
        );
        assert!(out[0].1.ends_with('…'));
    }
}

/// How many stack frames are requested at a time. Deep stacks are paged in with "Load more…"
/// in the stack frame picker.
pub const STACK_FRAMES_PAGE: usize = 20;

/// Keeps the first page of the stack of `thread_id` (a failed request keeps what was there).
fn store_stack_trace(
    debugger: &mut Client,
    thread_id: ThreadId,
    page: helix_dap::Result<(Vec<helix_dap::StackFrame>, Option<usize>)>,
) {
    let Ok((frames, total)) = page else {
        return;
    };
    match total {
        Some(total) => debugger.total_frames.insert(thread_id, total),
        None => debugger.total_frames.remove(&thread_id),
    };
    debugger.stack_frames.insert(thread_id, frames);
}

/// Makes sure there is a read-only buffer with the text of a source the adapter generated
/// (it has a `sourceReference` and no path) and returns its path. The text is requested once;
/// the buffer is kept until the adapter reports that the source changed or the session ends.
///
/// When there is no buffer yet, the text is requested in the background and `None` is
/// returned; once it arrives the buffer is opened and, if `frame` is still the selected frame,
/// the editor jumps to it.
pub fn open_adapter_source(
    editor: &mut Editor,
    source: &helix_dap::Source,
    frame: Option<&helix_dap::StackFrame>,
) -> Option<PathBuf> {
    let id = editor.debug_adapters.active_id()?;
    let path = editor
        .debug_adapters
        .get_client_mut(id)?
        .register_adapter_source(source);
    if editor.document_by_path(&path).is_some() {
        return Some(path);
    }

    let content = editor
        .debug_adapters
        .get_client(id)?
        .source_content(source.clone());
    let frame = frame.cloned();
    spawn_job(editor, id, async move {
        let content = tokio::time::timeout(ADAPTER_SOURCE_TIMEOUT, content).await;
        Some(Box::new(move |editor: &mut Editor| {
            let content = match content {
                Ok(Ok(content)) => content,
                Ok(Err(err)) => {
                    editor.set_error(format!("Unable to load the source from the adapter: {err}"));
                    return;
                }
                Err(_) => {
                    editor.set_error("The adapter did not send the source in time");
                    return;
                }
            };
            // The session ended or the source changed while it was loading.
            let known = editor
                .debug_adapters
                .get_client(id)
                .is_some_and(|debugger| debugger.adapter_sources.contains_key(&path));
            if !known {
                return;
            }
            if editor.document_by_path(&path).is_none() {
                let mut doc = crate::Document::from(
                    helix_core::Rope::from(content),
                    None,
                    editor.config.clone(),
                    editor.syn_loader.clone(),
                );
                doc.set_path(Some(&path));
                // Nothing of it can be saved: it only exists in the adapter.
                doc.readonly = true;
                let loader = editor.syn_loader.load();
                doc.detect_language(&loader);
                editor.new_file_from_document(Action::Load, doc);
            }
            let Some(frame) = frame else {
                return;
            };
            let selected = editor.debug_adapters.active_id() == Some(id)
                && editor
                    .debug_adapters
                    .get_active_client()
                    .and_then(|debugger| debugger.current_stack_frame())
                    .is_some_and(|current| current.id == frame.id);
            if selected {
                jump_to_stack_frame(editor, &frame);
            }
        }) as Job)
    });
    None
}

/// Forgets the buffer of a source the adapter generated and the breakpoints set in it, so the
/// next stop in it loads it again.
pub fn forget_adapter_source(
    editor: &mut Editor,
    session: DebugAdapterId,
    source: &helix_dap::Source,
) {
    let Some(debugger) = editor.debug_adapters.get_client_mut(session) else {
        return;
    };
    let gone: Vec<PathBuf> = debugger
        .adapter_sources
        .iter()
        .filter(|(_, known)| {
            known.source_reference.is_some() && known.source_reference == source.source_reference
        })
        .map(|(path, _)| path.clone())
        .collect();
    for path in gone {
        if let Some(debugger) = editor.debug_adapters.get_client_mut(session) {
            debugger.adapter_sources.remove(&path);
        }
        editor.breakpoints.remove(&path);
        if let Some(id) = editor.document_id_by_path(&path) {
            let _ = editor.close_document(id, true);
        }
    }
}

/// A session is over: its generated sources and the breakpoints in them are gone with it.
pub fn forget_session_sources(editor: &mut Editor, session: DebugAdapterId) {
    let dir = helix_dap::adapter_source_dir(session);
    editor.breakpoints.retain(|path, _| !path.starts_with(&dir));
}

/// An adapter that takes longer than this to send a generated source is given up on.
const ADAPTER_SOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The path of an open buffer of the same file as `path`, or `path`. An adapter names files by
/// the path it was given, which `:debug-start` resolves through symlinks (`/var` is
/// `/private/var` on macOS): opening that path again would make a second buffer of a file
/// that is already open, and breakpoints set in one wouldn't show in the other.
fn open_path_of_same_file(editor: &Editor, path: PathBuf) -> PathBuf {
    let open = |candidate: &std::path::Path| editor.document_by_path(candidate).is_some();
    if open(&path) {
        return path;
    }
    let Ok(real) = std::fs::canonicalize(&path) else {
        return path;
    };
    editor
        .documents()
        .filter_map(|doc| doc.path())
        .find(|doc_path| std::fs::canonicalize(doc_path).is_ok_and(|doc_real| doc_real == real))
        .map(ToOwned::to_owned)
        .unwrap_or(path)
}

/// Names the sources of `frames` by the paths of the buffers already open for them, see
/// [`open_path_of_same_file`]: the gutter, inline values and the stack frame picker find a
/// frame's buffer by its path.
pub fn with_open_buffer_paths(
    editor: &Editor,
    mut frames: Vec<helix_dap::StackFrame>,
) -> Vec<helix_dap::StackFrame> {
    let mut known: std::collections::HashMap<PathBuf, PathBuf> = Default::default();
    for frame in &mut frames {
        let Some(path) = frame
            .source
            .as_mut()
            .and_then(|source| source.path.as_mut())
        else {
            continue;
        };
        let open = known
            .entry(path.clone())
            .or_insert_with(|| open_path_of_same_file(editor, path.clone()));
        path.clone_from(open);
    }
    frames
}

pub fn jump_to_stack_frame(editor: &mut Editor, frame: &helix_dap::StackFrame) {
    let path = match &frame.source {
        Some(helix_dap::Source {
            path: Some(path), ..
        }) => path.clone(),
        Some(source)
            if source
                .source_reference
                .is_some_and(|reference| reference > 0) =>
        {
            match open_adapter_source(editor, source, Some(frame)) {
                Some(path) => path,
                None => return,
            }
        }
        _ => return,
    };
    let path = open_path_of_same_file(editor, path);

    if let Err(e) = editor.open(&path, Action::Replace) {
        editor.set_error(format!("Unable to jump to stack frame: {}", e));
        return;
    }

    let (view, doc) = current!(editor);

    let text_end = doc.text().len_chars().saturating_sub(1);
    let start = dap_pos_to_pos(doc.text(), frame.line, frame.column).unwrap_or(0);
    let end = frame
        .end_line
        .and_then(|end_line| dap_pos_to_pos(doc.text(), end_line, frame.end_column.unwrap_or(0)))
        .unwrap_or(start);

    let selection = Selection::single(start.min(text_end), end.min(text_end));
    doc.set_selection(view.id, selection);
    align_view(doc, view, Align::Center);
}

/// Sends the breakpoints the editor has for `path` to the session `session`. What the adapter
/// answers (whether it could set them, and where) is merged in when the answer arrives; an
/// error is only returned for breakpoints the adapter can't take.
pub fn breakpoints_changed(
    editor: &mut Editor,
    session: DebugAdapterId,
    path: PathBuf,
) -> Result<(), anyhow::Error> {
    let Some(debugger) = editor.debug_adapters.get_client_mut(session) else {
        return Ok(());
    };
    let breakpoints: &[Breakpoint] = editor.breakpoints.get(&path).map_or(&[], Vec::as_slice);
    if let Some(caps) = debugger.caps.as_ref() {
        if breakpoints.iter().any(|b| b.condition.is_some())
            && !caps.supports_conditional_breakpoints.unwrap_or_default()
        {
            bail!("Can't edit breakpoint: debugger does not support conditional breakpoints")
        }
        if breakpoints.iter().any(|b| b.hit_condition.is_some())
            && !caps
                .supports_hit_conditional_breakpoints
                .unwrap_or_default()
        {
            bail!("Can't edit breakpoint: debugger does not support hit conditional breakpoints")
        }
        if breakpoints.iter().any(|b| b.log_message.is_some())
            && !caps.supports_log_points.unwrap_or_default()
        {
            bail!("Can't edit breakpoint: debugger does not support logpoints")
        }
    }
    let source_breakpoints = breakpoints
        .iter()
        .map(|breakpoint| helix_dap::SourceBreakpoint {
            line: breakpoint.line + 1, // convert from 0-indexing to 1-indexing (TODO: could set debugger to 0-indexing on init)
            column: breakpoint.column,
            condition: breakpoint.condition.clone(),
            hit_condition: breakpoint.hit_condition.clone(),
            log_message: breakpoint.log_message.clone(),
        })
        .collect::<Vec<_>>();

    let sent = source_breakpoints.len();
    let request = debugger.next_breakpoints_request(&path);
    let response = debugger.set_breakpoints(path.clone(), source_breakpoints);
    spawn_job(editor, session, async move {
        let response = response.await;
        Some(Box::new(move |editor: &mut Editor| {
            // The breakpoints changed again since: the answer to that request counts.
            let latest = editor
                .debug_adapters
                .get_client(session)
                .is_some_and(|debugger| debugger.is_latest_breakpoints_request(&path, request));
            if !latest {
                return;
            }
            let dap_breakpoints = match response {
                Ok(Some(dap_breakpoints)) => dap_breakpoints,
                Ok(None) => return,
                Err(err) => {
                    editor.set_error(format!("Failed to set breakpoints: {err}"));
                    return;
                }
            };
            // The adapter answers for the breakpoints in the order they were sent.
            let Some(breakpoints) = editor
                .breakpoints
                .get_mut(&path)
                .filter(|breakpoints| breakpoints.len() == sent)
            else {
                return;
            };
            for (breakpoint, dap_breakpoint) in breakpoints.iter_mut().zip(dap_breakpoints) {
                breakpoint.id = dap_breakpoint.id;
                breakpoint.verified = dap_breakpoint.verified;
                breakpoint.message = dap_breakpoint.message;
                // TODO: verify source matches
                // A breakpoint the adapter couldn't place (no line) stays where it was set.
                if let Some(line) = dap_breakpoint.line {
                    breakpoint.line = line.saturating_sub(1); // convert to 0-indexing
                }
                breakpoint.column = dap_breakpoint.column.or(breakpoint.column);
                // TODO: verify end_linef/col instruction reference, offset
            }
        }) as Job)
    });
    Ok(())
}

impl Editor {
    pub async fn handle_debugger_message(
        &mut self,
        id: DebugAdapterId,
        payload: helix_dap::Payload,
    ) -> bool {
        use helix_dap::{events, Event};

        match payload {
            Payload::Event(event) if event.event == JOB_EVENT => {
                if let Some(job) = take_job(event.body.as_ref()) {
                    job(self);
                }
            }
            Payload::Event(event) => {
                let event = match Event::parse(&event.event, event.body) {
                    Ok(event) => event,
                    Err(dap::Error::Unhandled) => {
                        log::info!("Discarding unknown DAP event '{}'", event.event);
                        return false;
                    }
                    Err(err) => {
                        log::warn!("Discarding invalid DAP event '{}': {err}", event.event);
                        return false;
                    }
                };
                match event {
                    Event::Stopped(events::StoppedBody {
                        thread_id,
                        description,
                        text,
                        reason,
                        all_threads_stopped,
                        ..
                    }) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let all_threads_stopped = all_threads_stopped.unwrap_or_default();

                        if let Some(thread_id) = thread_id {
                            debugger.thread_states.insert(thread_id, reason.clone());
                            // TODO: dap uses "type" || "reason" here
                        }
                        // whichever thread stops is made "current" (if no previously selected
                        // thread) once its stack arrived.
                        fetch_stop(self, id, thread_id, all_threads_stopped);

                        let scope = match thread_id {
                            Some(id) => format!("Thread {}", id),
                            None => "Target".to_owned(),
                        };

                        let mut status = format!("{} stopped because of {}", scope, reason);
                        if let Some(desc) = description {
                            write!(status, " {}", desc).unwrap();
                        }
                        if let Some(text) = text {
                            write!(status, " {}", text).unwrap();
                        }
                        if all_threads_stopped {
                            status.push_str(" (all threads stopped)");
                        }

                        self.set_status(status);
                    }
                    Event::Continued(events::ContinuedBody { thread_id, .. }) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger
                            .thread_states
                            .insert(thread_id, "running".to_owned());
                        if debugger.thread_id == Some(thread_id) {
                            debugger.resume_application();
                            self.clear_dap_inline_values();
                        }
                    }
                    Event::Thread(thread) => {
                        self.set_status(format!("Thread {}: {}", thread.thread_id, thread.reason));
                    }
                    Event::Breakpoint(events::BreakpointBody { reason, breakpoint }) => {
                        match &reason[..] {
                            "new" => {
                                if let Some(source) = breakpoint.source {
                                    let Some(path) = source.path else {
                                        warn!("DAP breakpoint event missing source path");
                                        return false;
                                    };
                                    let Some(line) = breakpoint.line else {
                                        warn!("DAP breakpoint event missing line");
                                        return false;
                                    };
                                    self.breakpoints.entry(path).or_default().push(Breakpoint {
                                        id: breakpoint.id,
                                        verified: breakpoint.verified,
                                        message: breakpoint.message.clone(),
                                        line: line.saturating_sub(1),
                                        column: breakpoint.column,
                                        ..Default::default()
                                    });
                                    if let Some(message) = &breakpoint.message {
                                        self.set_status(format!("Breakpoint: {}", message));
                                    }
                                }
                            }
                            "changed" => {
                                let Some(id) = breakpoint.id else {
                                    warn!("DAP breakpoint event missing id");
                                    return false;
                                };
                                if let Some(source) = &breakpoint.source {
                                    if source.path.is_none() {
                                        warn!("DAP breakpoint event missing source path");
                                    }
                                }
                                if breakpoint.line.is_none() {
                                    warn!("DAP breakpoint event missing line");
                                }
                                for breakpoints in self.breakpoints.values_mut() {
                                    if let Some(i) =
                                        breakpoints.iter().position(|b| b.id == Some(id))
                                    {
                                        breakpoints[i].verified = breakpoint.verified;
                                        breakpoints[i].message = breakpoint
                                            .message
                                            .clone()
                                            .or_else(|| breakpoints[i].message.take());
                                        breakpoints[i].line =
                                            breakpoint.line.map_or(breakpoints[i].line, |line| {
                                                line.saturating_sub(1)
                                            });
                                        breakpoints[i].column =
                                            breakpoint.column.or(breakpoints[i].column);
                                    }
                                }
                                if let Some(message) = &breakpoint.message {
                                    self.set_status(format!("Breakpoint: {}", message));
                                }
                            }
                            "removed" => {
                                let Some(id) = breakpoint.id else {
                                    warn!("DAP breakpoint event missing id");
                                    return false;
                                };
                                for breakpoints in self.breakpoints.values_mut() {
                                    if let Some(i) =
                                        breakpoints.iter().position(|b| b.id == Some(id))
                                    {
                                        breakpoints.remove(i);
                                    }
                                }
                            }
                            reason => {
                                warn!("Unknown breakpoint event: {}", reason);
                            }
                        }
                    }
                    Event::Output(events::OutputBody {
                        category, output, ..
                    }) => {
                        let prefix = match category {
                            Some(category) => {
                                if &category == "telemetry" {
                                    return false;
                                }
                                format!("Debug ({}):", category)
                            }
                            None => "Debug:".to_owned(),
                        };

                        log::info!("{}", output);
                        self.set_status(format!("{} {}", prefix, output));
                    }
                    Event::ProgressStart(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_start(body)
                        };

                        self.set_status(status);
                    }
                    Event::ProgressUpdate(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_update(body)
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::ProgressEnd(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_end(body)
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::Initialized(_) => {
                        self.set_status("Debugger initialized...");
                        let debugger = match self.debug_adapters.get_client(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        // send existing breakpoints
                        let paths: Vec<PathBuf> = self
                            .breakpoints
                            .keys()
                            // Sources generated by another session mean nothing to this one.
                            .filter(|path| {
                                !path.starts_with(helix_dap::adapter_source_dir_root())
                                    || debugger.adapter_sources.contains_key(*path)
                            })
                            .cloned()
                            .collect();
                        for path in paths {
                            let _ = breakpoints_changed(self, id, path);
                        }
                        // TODO: fetch breakpoints (in case we're attaching)

                        // Requests go out in order: the breakpoints are set before the adapter
                        // hears that the configuration is done.
                        let Some(debugger) = self.debug_adapters.get_client(id) else {
                            return false;
                        };
                        let configuration_done = debugger.configuration_done();
                        spawn_job(self, id, async move {
                            let err = configuration_done.await.err()?;
                            Some(Box::new(move |editor: &mut Editor| {
                                editor.set_error(format!("Debugger configuration failed: {}", err));
                            }) as Job)
                        });
                        self.set_status("Debugged application started");

                        self.debug_adapters.set_active_client(id);
                    }
                    Event::Terminated(terminated) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let restart_arg = if let Some(terminated) = terminated {
                            terminated.restart
                        } else {
                            None
                        };

                        let restart_bool = restart_arg
                            .as_ref()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let disconnect_args = Some(DisconnectArguments {
                            restart: Some(restart_bool),
                            terminate_debuggee: None,
                            suspend_debuggee: None,
                        });

                        // `disconnect` forgets how the session was started: a restart needs it.
                        let connection_type = debugger.connection_type();
                        let disconnect = debugger.disconnect(disconnect_args);
                        spawn_job(self, id, async move {
                            let disconnected = disconnect.await;
                            Some(Box::new(move |editor: &mut Editor| {
                                session_terminated(
                                    editor,
                                    id,
                                    restart_arg,
                                    connection_type,
                                    disconnected,
                                )
                            }) as Job)
                        });
                    }
                    Event::LoadedSource(events::LoadedSourceBody { reason, source }) => {
                        // A generated source that changed or went away is not what its buffer
                        // shows (and what the breakpoints in it refer to) any more.
                        if reason == "changed" || reason == "removed" {
                            forget_adapter_source(self, id, &source);
                        }
                    }
                    Event::Exited(resp) => {
                        let exit_code = resp.exit_code;
                        if exit_code != 0 {
                            self.set_error(format!(
                                "Debuggee failed to exit successfully (exit code: {exit_code})."
                            ));
                        }
                    }
                    ev => {
                        log::warn!("Unhandled event {:?}", ev);
                        return false; // return early to skip render
                    }
                }
            }
            Payload::Response(_) => unreachable!(),
            Payload::Request(request) => {
                let reply = match Request::parse(&request.command, request.arguments) {
                    Ok(Request::RunInTerminal(arguments)) => {
                        let config = self.config();
                        let Some(config) = config.terminal.as_ref() else {
                            self.set_error("No external terminal defined");
                            return true;
                        };

                        match run_in_terminal(config, &arguments) {
                            Ok(Terminal::Started(process_id)) => {
                                Ok(json!(dap::requests::RunInTerminalResponse {
                                    process_id: Some(process_id),
                                    shell_process_id: None,
                                }))
                            }
                            Ok(Terminal::Launcher { program, args }) => {
                                // The launcher may take a while (or ask the user something):
                                // wait for it in the background and reply from there.
                                let Some(debugger) = self.debug_adapters.get_client(id) else {
                                    return true;
                                };
                                let reply = debugger.deferred_reply(request.seq, &request.command);
                                spawn_job(self, id, async move {
                                    match run_launcher(&program, &args, LAUNCHER_TIMEOUT).await {
                                        Ok(()) => {
                                            // The launcher is not the debuggee (it has exited by
                                            // now): there is no process id to tell.
                                            let _ = reply(Ok(json!(
                                                dap::requests::RunInTerminalResponse {
                                                    process_id: None,
                                                    shell_process_id: None,
                                                }
                                            )));
                                            None
                                        }
                                        Err(err) => Some(Box::new(move |editor: &mut Editor| {
                                            editor.set_error(err)
                                        })
                                            as Job),
                                    }
                                });
                                return true;
                            }
                            Err(err) => {
                                self.set_error(err);
                                return true;
                            }
                        }
                    }
                    Ok(Request::StartDebugging(arguments)) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => {
                                self.set_error("No active debugger found.");
                                return true;
                            }
                        };
                        // Currently we only support starting a child debugger if the parent is using the TCP transport
                        let socket = match debugger.socket {
                            Some(socket) => socket,
                            None => {
                                self.set_error("Child debugger can only be started if the parent debugger is using TCP transport.");
                                return true;
                            }
                        };

                        let config = match debugger.config.clone() {
                            Some(config) => config,
                            None => {
                                error!("No configuration found for the debugger.");
                                return true;
                            }
                        };

                        // Starting the child waits for it: that happens in the background, and
                        // the reply is sent from there.
                        let reply = debugger.deferred_reply(request.seq, &request.command);
                        let start = dap::registry::Registry::start_client(Some(socket), &config);
                        spawn_job(self, id, async move {
                            let child = match start.await {
                                Ok(child) => child,
                                Err(err) => {
                                    let message =
                                        format!("Failed to create child debugger: {:?}", err);
                                    let _ = reply(Err(err));
                                    return Some(Box::new(move |editor: &mut Editor| {
                                        editor.set_error(message)
                                    }) as Job);
                                }
                            };
                            Some(Box::new(move |editor: &mut Editor| {
                                start_child_session(editor, id, child, arguments, reply)
                            }) as Job)
                        });
                        return true;
                    }
                    Err(err) => Err(err),
                };

                if let Some(debugger) = self.debug_adapters.get_client_mut(id) {
                    debugger
                        .reply(request.seq, &request.command, reply)
                        .await
                        .ok();
                }
            }
        }
        true
    }
}

/// The child debugger the session `parent` asked for (`startDebugging`) runs: it joins the
/// sessions and is launched (or attached); `reply` answers the parent's request once that is done.
fn start_child_session(
    editor: &mut Editor,
    parent: DebugAdapterId,
    child: dap::registry::StartedClient,
    arguments: dap::requests::StartDebuggingArguments,
    reply: impl FnOnce(Result<Value, dap::Error>) -> dap::Result<()> + Send + 'static,
) {
    let child_id = editor.debug_adapters.add_client(child);
    let Some(client) = editor.debug_adapters.get_client_mut(child_id) else {
        return;
    };
    let child_name = arguments
        .configuration
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    client.parent = Some(parent);
    client.name = Some(child_name.unwrap_or_else(|| "child".to_owned()));

    let launched = if let ConnectionType::Launch = arguments.request {
        Either::Left(client.launch(arguments.configuration))
    } else {
        Either::Right(client.attach(arguments.configuration))
    };
    spawn_job(editor, child_id, async move {
        match launched.await {
            Ok(_) => {
                let _ = reply(Ok(json!({
                    "success": true,
                })));
                None
            }
            Err(err) => {
                let message = format!("Failed to start debugging session: {:?}", err);
                let _ = reply(Err(err));
                Some(Box::new(move |editor: &mut Editor| {
                    // Nothing runs in it.
                    editor.debug_adapters.remove_client(child_id);
                    editor.set_error(message)
                }) as Job)
            }
        }
    });
}

/// The adapter of session `id` answered the `disconnect` sent after the debuggee terminated: the
/// session ends, or starts again when the adapter asked for a restart.
fn session_terminated(
    editor: &mut Editor,
    id: DebugAdapterId,
    restart_arg: Option<Value>,
    connection_type: Option<ConnectionType>,
    disconnected: helix_dap::Result<Value>,
) {
    // Ended in the meantime (`debug-terminate`).
    if editor.debug_adapters.get_client(id).is_none() {
        return;
    }

    match restart_arg {
        Some(Value::Bool(false)) | None => {
            // An adapter that exits right after the debuggee doesn't answer: the session is
            // over all the same.
            if let Err(err) = disconnected {
                log::warn!("Cannot disconnect debugger upon terminated event receival: {err}");
            }
            let was_selected = editor.debug_adapters.active_id() == Some(id);
            editor.debug_adapters.remove_client(id);
            forget_session_sources(editor, id);
            if was_selected {
                editor.clear_dap_inline_values();
                if editor.debug_adapters.active_id().is_some() {
                    // Another session takes the place of the one that ended.
                    show_current_frame(editor);
                }
            }
            editor.set_status("Terminated debugging session and disconnected debugger.");

            // Go through all breakpoints and set verfified to false
            // this should update the UI to show the breakpoints are no longer connected
            for breakpoints in editor.breakpoints.values_mut() {
                for breakpoint in breakpoints.iter_mut() {
                    breakpoint.verified = false;
                }
            }
        }
        Some(val) => {
            if let Err(err) = disconnected {
                editor.set_error(format!(
                    "Cannot disconnect debugger upon terminated event receival {:?}",
                    err
                ));
                return;
            }
            log::info!("Attempting to restart debug session.");
            let Some(connection_type) = connection_type else {
                editor.set_error(
                    "No starting request found, to be used in restarting the debugging session.",
                );
                return;
            };
            let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
                return;
            };
            let relaunch = if let ConnectionType::Launch = connection_type {
                Either::Left(debugger.launch(val))
            } else {
                Either::Right(debugger.attach(val))
            };
            spawn_job(editor, id, async move {
                let err = relaunch.await.err()?;
                Some(Box::new(move |editor: &mut Editor| {
                    editor.set_error(format!("Failed to restart debugging session: {:?}", err));
                }) as Job)
            });
        }
    }
}

/// How long a launcher gets to open the pane before it is given up on.
const LAUNCHER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// What [`run_in_terminal`] did.
#[derive(Debug)]
enum Terminal {
    /// The terminal was started: the id of its process.
    Started(u32),
    /// A launcher has to open the pane: run it with [`run_launcher`].
    Launcher { program: String, args: Vec<String> },
}

/// Opens the terminal for a debuggee. A configured terminal is started right away; a launcher
/// (a command that asks a running terminal or multiplexer for a pane) is returned for the
/// caller to run without blocking.
fn run_in_terminal(
    config: &crate::editor::TerminalConfig,
    arguments: &dap::requests::RunInTerminalArguments,
) -> Result<Terminal, String> {
    use crate::terminal_launcher::Launch;
    use std::process::Command;

    let cwd = if arguments.cwd.is_empty() {
        helix_stdx::env::current_working_dir()
            .to_string_lossy()
            .into_owned()
    } else {
        arguments.cwd.clone()
    };

    if let Some(launcher) = config.launcher {
        let (program, args) = launcher.command_line(&Launch {
            cwd: &cwd,
            title: arguments.title.as_deref(),
            args: &arguments.args,
            env: arguments.env.as_ref(),
        });
        return Ok(Terminal::Launcher { program, args });
    }

    if config.command.is_empty() {
        return Err("No external terminal defined".to_string());
    }
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .args(&arguments.args)
        .current_dir(&cwd);
    for (name, value) in arguments.env.iter().flatten() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("Error starting external terminal: {err}"))?;
    let process_id = child.id();
    // Reap the terminal once it is closed.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(Terminal::Started(process_id))
}

/// Runs a command that opens a pane and returns as soon as it exists, so that we can wait for it
/// and tell the user why there is no pane (kitty without `allow_remote_control`, no zellij
/// session, ...).
async fn run_launcher(
    program: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<(), String> {
    use std::process::Stdio;

    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // A launcher that is given up on doesn't stay around.
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("Error starting {program}: {err}"))?;

    // stderr is read while waiting: a launcher that writes a lot of it doesn't block on a full
    // pipe.
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("Error waiting for {program}: {err}")),
        Err(_) => return Err(format!("{program} did not open a terminal in time")),
    };
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    Err(if stderr.is_empty() {
        format!("{program} failed to open a terminal ({})", output.status)
    } else {
        format!("{program} failed to open a terminal: {stderr}")
    })
}

#[cfg(all(test, unix))]
mod run_in_terminal_tests {
    use super::*;
    use crate::editor::TerminalConfig;
    use std::collections::HashMap;
    use std::time::Duration;

    fn arguments(cwd: &str, args: &[&str]) -> dap::requests::RunInTerminalArguments {
        dap::requests::RunInTerminalArguments {
            kind: None,
            title: Some("Debug".into()),
            cwd: cwd.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: None,
        }
    }

    fn wait_for(path: &std::path::Path) -> String {
        for _ in 0..500 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{} was never written", path.display());
    }

    #[test]
    fn custom_terminal_gets_the_cwd_the_environment_and_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let config = TerminalConfig {
            command: "sh".into(),
            // `$0` is the first debuggee argument, `$@` the others
            args: vec![
                "-c".into(),
                format!(
                    "echo \"$PWD|$GREETING|${{UNSET_ME-unset}}|$0 $@\" > {}",
                    out.display()
                ),
            ],
            launcher: None,
        };
        let mut request = arguments(dir.path().to_str().unwrap(), &["./app", "--flag"]);
        request.env = Some(HashMap::from([
            ("GREETING".to_string(), Some("hello".to_string())),
            ("UNSET_ME".to_string(), None),
        ]));

        run_in_terminal(&config, &request).unwrap();

        let text = wait_for(&out);
        let (pwd, rest) = text.split_once('|').unwrap();
        assert_eq!(
            std::fs::canonicalize(pwd).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert_eq!(rest.trim(), "hello|unset|./app --flag");
    }

    #[test]
    fn no_terminal_is_an_error() {
        let err = run_in_terminal(&TerminalConfig::default(), &arguments("/", &["app"]));
        assert_eq!(err.unwrap_err(), "No external terminal defined");
    }

    #[tokio::test]
    async fn a_failing_launcher_reports_why() {
        let err = run_launcher(
            "sh",
            &[
                "-c".to_string(),
                "echo remote control is off >&2; exit 1".to_string(),
            ],
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(err, "sh failed to open a terminal: remote control is off");

        let err = run_launcher("/definitely/not/installed", &[], Duration::from_secs(5)).await;
        assert!(err.unwrap_err().starts_with("Error starting"));
    }

    #[tokio::test]
    async fn a_launcher_that_hangs_is_given_up_on() {
        let err = run_launcher(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(err.unwrap_err(), "sh did not open a terminal in time");
    }

    #[tokio::test]
    async fn a_launcher_that_writes_a_lot_of_stderr_does_not_block() {
        // More than a pipe holds (64 KiB), then a failure.
        let err = run_launcher(
            "sh",
            &[
                "-c".to_string(),
                "head -c 200000 /dev/zero | tr '\\0' x >&2; exit 1".to_string(),
            ],
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(
            err.starts_with("sh failed to open a terminal: xxx"),
            "{}",
            &err[..60]
        );
    }

    #[tokio::test]
    async fn a_launcher_that_succeeds_is_ok() {
        run_launcher("true", &[], Duration::from_secs(5))
            .await
            .unwrap();
    }

    #[test]
    fn a_launcher_is_left_to_the_caller() {
        let config = TerminalConfig {
            launcher: Some(crate::terminal_launcher::Launcher::Tmux),
            ..TerminalConfig::default()
        };
        let launch = run_in_terminal(&config, &arguments("/", &["app"])).unwrap();
        assert!(matches!(launch, Terminal::Launcher { .. }), "{launch:?}");
    }
}
