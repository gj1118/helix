//! Tests that drive the plugin API against a real (headless) `Editor`.

use super::*;
use crate::types::{EventData, UiHandler, UiHandlerWrapper};
use arc_swap::ArcSwap;
use helix_view::editor::{Action, Config};
use helix_view::graphics::Rect;
use helix_view::handlers::Handlers;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

/// Build a minimal headless editor with one scratch buffer in one view.
/// Must be called inside a runtime context; background tasks are never driven.
fn test_editor() -> Editor {
    fn ch<T>() -> tokio::sync::mpsc::Sender<T> {
        tokio::sync::mpsc::channel(1).0
    }
    let handlers = Handlers {
        completions: helix_view::handlers::completion::CompletionHandler::new(ch()),
        signature_hints: ch(),
        auto_save: ch(),
        auto_reload: ch(),
        document_colors: ch(),
        inline_completion: ch(),
        document_symbols: ch(),
        blame: ch(),
        document_links: ch(),
        code_lenses: ch(),
        word_index: helix_view::handlers::word_index::Handler::spawn(),
        pull_diagnostics: ch(),
        pull_all_documents_diagnostics: ch(),
        code_action_hint: ch(),
    };
    let config = Arc::new(ArcSwap::from_pointee(Config::default()));
    let mut editor = Editor::new(
        Rect::new(0, 0, 120, 40),
        Arc::new(helix_view::theme::Loader::new(&[])),
        Arc::new(ArcSwap::from_pointee(
            helix_core::config::default_lang_loader(),
        )),
        config,
        handlers,
        helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        HashMap::new(),
    );
    editor.new_file(Action::VerticalSplit);
    editor
}

fn engine() -> LuaEngine {
    let engine = LuaEngine::new().unwrap();
    engine
        .register_api(crate::types::PluginConfig::default())
        .unwrap();
    engine
}

fn run(engine: &LuaEngine, editor: &mut Editor, code: &str) -> LuaResult<()> {
    with_editor_context(editor, || engine.lua.load(code).exec())
}

#[test]
fn nested_editor_access_is_rejected() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    with_editor_context(&mut editor, || {
        let inner = with_editor(|_outer| Ok(with_editor(|_inner| Ok(())).is_err())).unwrap();
        assert!(inner, "a second borrow must fail");
        // released again after the outer borrow ended
        assert!(with_editor(|_| Ok(())).is_ok());
    });
    // context restored after the call
    assert!(matches!(try_with_editor(|_| Ok(())), Ok(None)));
}

#[test]
fn context_is_restored_after_panic() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_editor_context(&mut editor, || {
            let _ = with_editor(|_| -> LuaResult<()> { panic!("boom") });
        })
    }));
    assert!(result.is_err());
    assert!(matches!(try_with_editor(|_| Ok(())), Ok(None)));
    assert!(DEADLINE.with(|d| d.get()).is_none());
}

#[test]
fn set_selections_clamps_and_ignores_metamethods() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        helix.buffer.get_current():insert(1, 0, "hello")
        helix.editor.set_selections({ { anchor = 1000, head = 2000 } })
        local s = helix.editor.get_selections()
        assert(s[1].anchor == 6 and s[1].head == 6, s[1].anchor .. ":" .. s[1].head)

        -- values only reachable through __index are not read (no Lua runs under the borrow)
        local t = setmetatable({}, { __index = function() return helix.editor.get_cursor().col end })
        assert(not pcall(helix.editor.set_selections, { t }))
        "#,
    )
    .unwrap();
}

#[test]
fn buffer_edits_are_bounds_checked() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local buf = helix.buffer.get_current()
        buf:insert(1, 0, "abcdef\nxyz")
        assert(buf:get_text() == "abcdef\nxyz\n", buf:get_text())
        assert(buf:get_visual_column(9) == 2)
        assert(buf:get_visual_column(100000) == 0) -- clamped to the end of the text
        -- reversed range
        buf:delete(1, 4, 1, 1)
        assert(buf:get_text() == "aef\nxyz\n", buf:get_text())
        -- out of range positions are clamped
        buf:delete(99, 99, 1, 1)
        assert(buf:get_text() == "a", buf:get_text())
        "#,
    )
    .unwrap();
}

#[test]
fn closing_last_view_is_refused() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local ok, err = pcall(helix.editor.close)
        assert(not ok and tostring(err):find("last window"), tostring(err))
        assert(helix.editor.get_cursor() ~= nil)
        "#,
    )
    .unwrap();
    assert_eq!(editor.tree.views().count(), 1);
}

#[test]
fn handlers_can_register_handlers_and_commands() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    engine
        .lua
        .load(
            r#"
            count = 0
            helix.on("init", function()
                count = count + 1
                helix.on("init", function() end)
            end)
            helix.register_command({ name = "a", handler = function()
                helix.register_command({ name = "b", handler = function() count = count + 10 end })
            end })
            "#,
        )
        .exec()
        .unwrap();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    engine.call_event_handlers(&mut editor, &event).unwrap();
    engine.execute_command(&mut editor, "a", vec![]).unwrap();
    engine.execute_command(&mut editor, "b", vec![]).unwrap();
    let count: i64 = engine.lua.globals().get("count").unwrap();
    assert_eq!(count, 11);
}

struct NoopUi;
impl UiHandler for NoopUi {
    fn prompt(&self, _: &mut Editor, _: String, _: Option<String>, _: String, _: u64) {}
    fn confirm(&self, _: &mut Editor, _: String, _: String, _: u64) {}
    fn picker(&self, _: &mut Editor, _: Vec<String>, _: String, _: String, _: u64) {}
}

#[test]
fn ui_callbacks_can_reopen_ui_and_are_released() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    engine.lua.set_app_data(UiHandlerWrapper(Arc::new(NoopUi)));
    engine
        .lua
        .globals()
        .set("_current_plugin_name", "p")
        .unwrap();
    run(
        &engine,
        &mut editor,
        r#"helix.ui.prompt("first", nil, function(v)
            answer = v
            helix.ui.prompt("second", nil, function() end)
        end)"#,
    )
    .unwrap();
    let id = *engine
        .ui_callbacks
        .read()
        .keys()
        .next()
        .map(|(_, id)| id)
        .unwrap();
    engine
        .handle_ui_callback(&mut editor, "p".into(), id, serde_json::json!("x"))
        .unwrap();
    let answer: String = engine.lua.globals().get("answer").unwrap();
    assert_eq!(answer, "x");
    // the second prompt is pending; aborting it releases it
    let second = *engine
        .ui_callbacks
        .read()
        .keys()
        .next()
        .map(|(_, id)| id)
        .unwrap();
    assert!(engine.release_ui_callback("p".into(), second));
    assert!(engine.ui_callbacks.read().is_empty());
}

#[test]
fn runaway_handler_is_aborted() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    engine
        .lua
        .load(r#"helix.on("init", function() while true do end end)"#)
        .exec()
        .unwrap();
    let start = Instant::now();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    let err = engine.call_event_handlers(&mut editor, &event).unwrap_err();
    assert!(err.to_string().contains("time limit"), "{err}");
    assert!(start.elapsed() < PLUGIN_EXECUTION_TIMEOUT * 3);
    assert!(DEADLINE.with(|d| d.get()).is_none());
}

/// Runs `code` as an `init` handler and asserts that it is aborted by the time limit.
fn assert_aborted(code: &str) {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    engine
        .lua
        .load(format!(r#"helix.on("init", function() {code} end)"#))
        .exec()
        .unwrap();
    let start = Instant::now();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    let err = engine.call_event_handlers(&mut editor, &event).unwrap_err();
    assert!(err.to_string().contains("time limit"), "{code}: {err}");
    assert!(start.elapsed() < PLUGIN_EXECUTION_TIMEOUT * 3, "{code}");
    assert!(DEADLINE.with(|d| d.get()).is_none());
}

#[test]
fn timeout_cannot_be_caught_with_pcall() {
    assert_aborted("while true do pcall(function() while true do end end) end");
}

#[test]
fn timeout_cannot_be_caught_with_nested_pcall() {
    assert_aborted(
        "while true do pcall(function() pcall(function() while true do end end) end) end",
    );
}

#[test]
fn timeout_cannot_be_caught_with_xpcall() {
    assert_aborted(
        "while true do xpcall(function() while true do end end, function(e) return e end) end",
    );
}

#[test]
fn timeout_cannot_be_caught_by_a_looping_xpcall_handler() {
    assert_aborted("xpcall(function() while true do end end, function() while true do end end)");
}

#[test]
fn timeout_cannot_be_caught_with_coroutine_resume() {
    assert_aborted(
        "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
    );
}

#[test]
fn errors_are_still_caught_before_the_deadline() {
    let engine = engine();
    let caught: (bool, bool, bool) = engine
        .lua
        .load(
            r#"
            local a = pcall(error, "boom")
            local b = xpcall(function() error("boom") end, function(e) return e end)
            local c = coroutine.resume(coroutine.create(function() error("boom") end))
            return not a, not b, not c
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(caught, (true, true, true));
    let value: i64 = engine
        .lua
        .load("local ok, v = pcall(function() return 42 end) return v")
        .eval()
        .unwrap();
    assert_eq!(value, 42);
}

#[test]
fn timeout_reaches_into_wrapped_and_nested_coroutines() {
    assert_aborted("while true do pcall(coroutine.wrap(function() while true do end end)) end");
    assert_aborted(
        "local outer = coroutine.create(function() \
             coroutine.resume(coroutine.create(function() while true do end end)) \
         end) \
         while true do coroutine.resume(outer) end",
    );
}
