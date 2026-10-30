use crate::editor::{Action, Breakpoint};
use crate::{align_view, Align, Editor};
use anyhow::bail;
use dap::requests::DisconnectArguments;
use dap::requests::ThreadsArguments;
use helix_core::Selection;
use helix_dap::{
    self as dap, registry::DebugAdapterId, Client, ConnectionType, Payload, Request, ThreadId,
};
use helix_lsp::block_on;
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
pub fn dap_pos_to_pos(doc: &helix_core::Rope, line: usize, column: usize) -> Option<usize> {
    // 1-indexing to 0 indexing
    let line = doc.try_line_to_char(line - 1).ok()?;
    let pos = line + column.saturating_sub(1);
    // TODO: this is probably utf-16 offsets
    Some(pos)
}

pub async fn select_thread_id(editor: &mut Editor, thread_id: ThreadId, force: bool) {
    let Some(id) = editor.debug_adapters.active_id() else {
        return;
    };
    select_thread_id_of(editor, id, thread_id, force).await
}

/// Makes `thread_id` the current thread of the session `id` and, when that session is the
/// selected one, shows where it stopped.
pub async fn select_thread_id_of(
    editor: &mut Editor,
    id: DebugAdapterId,
    thread_id: ThreadId,
    force: bool,
) {
    let Some(debugger) = editor.debug_adapters.get_client_mut(id) else {
        return;
    };

    if !force && debugger.thread_id.is_some() {
        return;
    }

    debugger.thread_id = Some(thread_id);
    debugger.active_frame = Some(0);
    fetch_stack_trace(debugger, thread_id).await;

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

    show_current_frame(editor).await;
}

/// Jumps to the selected frame of the selected session and shows its inline values.
pub async fn show_current_frame(editor: &mut Editor) {
    let frame = editor
        .debug_adapters
        .get_active_client()
        .and_then(|debugger| debugger.current_stack_frame().cloned());
    if let Some(frame) = &frame {
        jump_to_stack_frame(editor, frame);
    }
    refresh_inline_values(editor).await;
}

/// How many lines above the current line of the stack frame get inline values.
const INLINE_VALUES_LINES: usize = 40;
/// How many values are shown at the end of one line.
const INLINE_VALUES_PER_LINE: usize = 4;
/// Values longer than this many characters are cut.
const INLINE_VALUE_WIDTH: usize = 24;
/// Asking the adapter for variables must not freeze the editor for long.
const INLINE_VALUES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Shows the values of the variables the lines around the selected stack frame use at the end
/// of those lines. Best effort: any problem leaves the lines without values.
pub async fn refresh_inline_values(editor: &mut Editor) {
    editor.clear_dap_inline_values();
    if !editor.config().debugger.inline_values {
        return;
    }
    let Some(debugger) = editor.debug_adapters.get_active_client() else {
        return;
    };
    let Some(frame) = debugger.current_stack_frame().cloned() else {
        return;
    };
    let Some(path) = frame.source.as_ref().and_then(|source| source.path.clone()) else {
        return;
    };

    let fetch = async {
        let mut variables = Vec::new();
        for scope in debugger.scopes(frame.id).await? {
            // Expensive scopes (globals, registers) are only loaded on request.
            if scope.expensive {
                continue;
            }
            variables.extend(debugger.variables(scope.variables_reference).await?);
        }
        Ok::<_, helix_dap::Error>(variables)
    };
    let Ok(Ok(variables)) = tokio::time::timeout(INLINE_VALUES_TIMEOUT, fetch).await else {
        return;
    };
    let variables: Vec<(String, String)> = variables
        .into_iter()
        .map(|variable| (variable.name, variable.value))
        .collect();

    if let Some(doc) = editor.document_by_path_mut(&path) {
        let line = frame.line.saturating_sub(1);
        doc.dap_inline_values = inline_value_annotations(doc.text(), line, &variables);
    }
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

pub async fn fetch_stack_trace(debugger: &mut Client, thread_id: ThreadId) {
    let (frames, total) = match debugger
        .stack_trace_page(thread_id, 0, STACK_FRAMES_PAGE)
        .await
    {
        Ok(page) => page,
        Err(_) => return,
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
pub fn open_adapter_source(editor: &mut Editor, source: &helix_dap::Source) -> Option<PathBuf> {
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
    let content = match block_on(tokio::time::timeout(ADAPTER_SOURCE_TIMEOUT, content)) {
        Ok(Ok(content)) => content,
        Ok(Err(err)) => {
            editor.set_error(format!("Unable to load the source from the adapter: {err}"));
            return None;
        }
        Err(_) => {
            editor.set_error("The adapter did not send the source in time");
            return None;
        }
    };

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
    editor.new_file_from_document(Action::Replace, doc);
    Some(path)
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

/// Waiting for a generated source blocks the editor: don't wait long.
const ADAPTER_SOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
            match open_adapter_source(editor, source) {
                Some(path) => path,
                None => return,
            }
        }
        _ => return,
    };

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

pub fn breakpoints_changed(
    debugger: &mut dap::Client,
    path: PathBuf,
    breakpoints: &mut [Breakpoint],
) -> Result<(), anyhow::Error> {
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

    let request = debugger.set_breakpoints(path, source_breakpoints);
    match block_on(request) {
        Ok(Some(dap_breakpoints)) => {
            for (breakpoint, dap_breakpoint) in breakpoints.iter_mut().zip(dap_breakpoints) {
                breakpoint.id = dap_breakpoint.id;
                breakpoint.verified = dap_breakpoint.verified;
                breakpoint.message = dap_breakpoint.message;
                // TODO: handle breakpoint.message
                // TODO: verify source matches
                breakpoint.line = dap_breakpoint.line.unwrap_or(0).saturating_sub(1); // convert to 0-indexing
                                                                                      // TODO: no unwrap
                breakpoint.column = dap_breakpoint.column;
                // TODO: verify end_linef/col instruction reference, offset
            }
        }
        Err(e) => anyhow::bail!("Failed to set breakpoints: {}", e),
        _ => {}
    };
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

                        if all_threads_stopped {
                            if let Ok(response) = debugger
                                .request::<dap::requests::Threads>(Some(ThreadsArguments {}))
                                .await
                            {
                                for thread in response.threads {
                                    fetch_stack_trace(debugger, thread.id).await;
                                }
                                select_thread_id_of(self, id, thread_id.unwrap_or_default(), false)
                                    .await;
                            }
                        } else if let Some(thread_id) = thread_id {
                            debugger.thread_states.insert(thread_id, reason.clone()); // TODO: dap uses "type" || "reason" here

                            fetch_stack_trace(debugger, thread_id).await;
                            // whichever thread stops is made "current" (if no previously selected thread).
                            select_thread_id_of(self, id, thread_id, false).await;
                        }

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
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        // send existing breakpoints
                        for (path, breakpoints) in &mut self.breakpoints {
                            // Sources generated by another session mean nothing to this one.
                            if path.starts_with(helix_dap::adapter_source_dir_root())
                                && !debugger.adapter_sources.contains_key(path)
                            {
                                continue;
                            }
                            // TODO: call futures in parallel, await all
                            let _ = breakpoints_changed(debugger, path.clone(), breakpoints);
                        }
                        // TODO: fetch breakpoints (in case we're attaching)

                        if let Err(err) = debugger.configuration_done().await {
                            self.set_error(format!("Debugger configuration failed: {}", err));
                        } else {
                            self.set_status("Debugged application started");
                        }

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

                        if let Err(err) = debugger.disconnect(disconnect_args).await {
                            self.set_error(format!(
                                "Cannot disconnect debugger upon terminated event receival {:?}",
                                err
                            ));
                            return false;
                        }

                        match restart_arg {
                            Some(Value::Bool(false)) | None => {
                                let was_selected = self.debug_adapters.active_id() == Some(id);
                                self.debug_adapters.remove_client(id);
                                forget_session_sources(self, id);
                                self.clear_dap_inline_values();
                                if was_selected && self.debug_adapters.active_id().is_some() {
                                    // Another session takes the place of the one that ended.
                                    show_current_frame(self).await;
                                }
                                self.set_status(
                                    "Terminated debugging session and disconnected debugger.",
                                );

                                // Go through all breakpoints and set verfified to false
                                // this should update the UI to show the breakpoints are no longer connected
                                for breakpoints in self.breakpoints.values_mut() {
                                    for breakpoint in breakpoints.iter_mut() {
                                        breakpoint.verified = false;
                                    }
                                }
                            }
                            Some(val) => {
                                log::info!("Attempting to restart debug session.");
                                let connection_type = match debugger.connection_type() {
                                    Some(connection_type) => connection_type,
                                    None => {
                                        self.set_error("No starting request found, to be used in restarting the debugging session.");
                                        return false;
                                    }
                                };

                                let relaunch_resp = if let ConnectionType::Launch = connection_type
                                {
                                    debugger.launch(val).await
                                } else {
                                    debugger.attach(val).await
                                };

                                if let Err(err) = relaunch_resp {
                                    self.set_error(format!(
                                        "Failed to restart debugging session: {:?}",
                                        err
                                    ));
                                }
                            }
                        }
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

                        let process = match std::process::Command::new(&config.command)
                            .args(&config.args)
                            .args(&arguments.args)
                            .spawn()
                        {
                            Ok(process) => process,
                            Err(err) => {
                                self.set_error(format!(
                                    "Error starting external terminal: {}",
                                    err
                                ));
                                return true;
                            }
                        };

                        Ok(json!(dap::requests::RunInTerminalResponse {
                            process_id: Some(process.id()),
                            shell_process_id: None,
                        }))
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

                        let result = self.debug_adapters.start_client(Some(socket), &config);

                        let client_id = match result {
                            Ok(child) => child,
                            Err(err) => {
                                self.set_error(format!(
                                    "Failed to create child debugger: {:?}",
                                    err
                                ));
                                return true;
                            }
                        };

                        let child_name = arguments
                            .configuration
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        let client = match self.debug_adapters.get_client_mut(client_id) {
                            Some(child) => child,
                            None => {
                                self.set_error("Failed to get child debugger.");
                                return true;
                            }
                        };
                        client.parent = Some(id);
                        client.name = Some(child_name.unwrap_or_else(|| "child".to_owned()));

                        let relaunch_resp = if let ConnectionType::Launch = arguments.request {
                            client.launch(arguments.configuration).await
                        } else {
                            client.attach(arguments.configuration).await
                        };
                        if let Err(err) = relaunch_resp {
                            self.set_error(format!("Failed to start debugging session: {:?}", err));
                            return true;
                        }

                        Ok(json!({
                            "success": true,
                        }))
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
