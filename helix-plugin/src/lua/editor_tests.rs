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

/// An engine with the state of a plugin `p` that may do everything.
fn engine() -> LuaEngine {
    let mut engine = LuaEngine::new().unwrap();
    engine
        .add_state("p", crate::permissions::Permissions::all())
        .unwrap();
    engine
}

/// The state of the plugin `p`.
fn lua(engine: &LuaEngine) -> &Lua {
    engine.state("p").unwrap()
}

/// Runs `code` as code of `p`, with the editor.
fn run(engine: &LuaEngine, editor: &mut Editor, code: &str) -> LuaResult<()> {
    engine.run_as("p", editor, code)
}

/// Runs `code` as code of `p`, without the editor (like a plugin being loaded).
fn load(engine: &LuaEngine, code: impl AsRef<str>) -> LuaResult<()> {
    with_plugin(&Arc::from("p"), || lua(engine).load(code.as_ref()).exec())
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
    load(
        &engine,
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
    .unwrap();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    engine.call_event_handlers(&mut editor, &event).unwrap();
    engine.execute_command(&mut editor, "a", vec![]).unwrap();
    engine.execute_command(&mut editor, "b", vec![]).unwrap();
    let count: i64 = lua(&engine).globals().get("count").unwrap();
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
    lua(&engine).set_app_data(UiHandlerWrapper(Arc::new(NoopUi)));
    with_plugin(&Arc::from("p"), || {
        run(
            &engine,
            &mut editor,
            r#"helix.ui.prompt("first", nil, function(v)
            answer = v
            helix.ui.prompt("second", nil, function() end)
        end)"#,
        )
    })
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
    let answer: String = lua(&engine).globals().get("answer").unwrap();
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

/// The time limit of the tests about it: short, so they don't keep the CPU busy (the other
/// tests have a long one, see `TEST_EXECUTION_TIMEOUT`).
const SHORT_TIME_LIMIT: Duration = Duration::from_millis(300);

/// Runs `f` with the time limit set to [`SHORT_TIME_LIMIT`].
fn with_short_time_limit<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(Duration);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_EXECUTION_TIMEOUT.with(|t| t.set(self.0));
        }
    }
    let _restore = Restore(TEST_EXECUTION_TIMEOUT.with(|t| t.replace(SHORT_TIME_LIMIT)));
    f()
}

#[test]
fn runaway_handler_is_aborted() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    load(
        &engine,
        r#"helix.on("init", function() while true do end end)"#,
    )
    .unwrap();
    let start = Instant::now();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    let err =
        with_short_time_limit(|| engine.call_event_handlers(&mut editor, &event)).unwrap_err();
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
    load(
        &engine,
        format!(r#"helix.on("init", function() {code} end)"#),
    )
    .unwrap();
    let start = Instant::now();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    let err =
        with_short_time_limit(|| engine.call_event_handlers(&mut editor, &event)).unwrap_err();
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
    let caught: (bool, bool, bool) = lua(&engine)
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
    let value: i64 = lua(&engine)
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

#[test]
fn timeout_reaches_into_string_patterns() {
    // Backtracking in C would run for hours; the ported matcher checks the deadline.
    assert_aborted("string.find(string.rep('a', 100000), '.-.-.-.-b')");
    assert_aborted("string.match(string.rep('a', 100000), '(.-)(.-)(.-)b')");
    assert_aborted("for _ in string.gmatch(string.rep('a', 100000), '.-.-.-b') do end");
    assert_aborted("string.gsub(string.rep('a', 100000), '.-.-.-b', '')");
    assert_aborted(
        "local s = string.rep('a', 100000) while true do pcall(string.find, s, '.-.-.-b') end",
    );
}

#[test]
fn timeout_reaches_into_table_shifts() {
    // `#t` can be made huge (`__len`) without the memory to back it, and `table.move` takes
    // any range: the loops of the C functions would never return.
    assert_aborted("table.move({}, 1, 1 << 40, 1, {})");
    assert_aborted(
        "local t = setmetatable({}, { __len = function() return 1 << 40 end }) table.insert(t, 1, 'x')",
    );
    assert_aborted(
        "local t = setmetatable({}, { __len = function() return 1 << 40 end }) table.remove(t, 1)",
    );
}

#[test]
fn timeout_cannot_be_caught_by_load_or_close() {
    // `load` catches errors of its reader function, `coroutine.close` those of `__close`.
    assert_aborted("while true do load(function() while true do end end) end");
    assert_aborted(
        "while true do \
             local co = coroutine.create(function() \
                 local x <close> = setmetatable({}, { __close = function() while true do end end }) \
                 coroutine.yield() \
             end) \
             coroutine.resume(co) \
             coroutine.close(co) \
         end",
    );
    assert_aborted(
        "while true do pcall(function() \
             local x <close> = setmetatable({}, { __close = function() while true do end end }) \
         end) end",
    );
}

#[test]
fn table_functions_keep_their_semantics() {
    let engine = engine();
    let result: String = lua(&engine).load(
            r#"
            local t = {}
            for i = 1, 10000 do t[i] = i end
            table.insert(t, 1, 0)            -- long shift (Lua loop)
            table.insert(t, #t - 1, "near")   -- short shift (C)
            table.insert(t, "last")
            local a = table.remove(t, 1)
            local b = table.remove(t)
            local c = table.remove(t, #t - 2)
            local m = table.move(t, 1, 9000, 2)  -- overlapping, backwards
            local n = table.move(t, 1, 5000, 1, {})
            local ok = pcall(table.insert, t, 0, "x")
            local ok2 = pcall(table.insert, t, 1, 2, 3)
            return table.concat({ a, b, c, #t, t[1], t[2], t[9001], #n, n[5000], tostring(ok), tostring(ok2) }, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(result, "0,last,near,10000,1,1,9000,5000,4999,false,false");
}

#[test]
fn sandbox_holes_are_closed() {
    let engine = engine();
    load(
        &engine,
        r#"
            -- io is not loaded at all (it used to stay reachable through require)
            assert(io == nil and package.loaded.io == nil)
            assert(not pcall(require, "io"))
            assert(os.remove == nil and os.rename == nil and os.tmpname == nil)
            assert(os.execute == nil and os.setlocale == nil and loadfile == nil)
            -- modules are not looked up in the working directory
            assert(not package.path:find("%./"), package.path)
            -- binary chunks are not verified by Lua: refused
            local f, err = load(string.dump(function() return 1 end))
            assert(f == nil and err:find("binary"), err)
            assert(load("return 1")() == 1)
            local env = {}
            load("x = 1", "chunk", "t", env)()
            assert(env.x == 1 and x == nil)
            -- finalizers run with the time limit off
            assert(not pcall(setmetatable, {}, { __gc = function() end }))
            assert(not pcall(setmetatable, {}, { __gc = true }))
            assert(getmetatable(setmetatable({}, { __index = {} })) ~= nil)
            -- print/warn must not write to the terminal
            print("from a plugin", 1, 2.5, nil, {})
            warn("@on") warn("a ", "warning")
            "#,
    )
    .unwrap();
}

#[test]
fn memory_is_limited() {
    let engine = engine();
    let ok: bool = lua(&engine)
        .load(
            r#"
            local ok, err = pcall(string.rep, "x", 1 << 30)
            assert(not ok and tostring(err):find("memory"), tostring(err))
            -- still usable afterwards
            return #string.rep("y", 1000) == 1000
            "#,
        )
        .eval()
        .unwrap();
    assert!(ok);
}

#[test]
fn failing_handler_does_not_stop_the_others() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    load(
        &engine,
        r#"
            ran = 0
            helix.on("init", function() error("first fails") end)
            helix.on("init", function() ran = ran + 1 end)
            "#,
    )
    .unwrap();
    let event = PluginEvent {
        event_type: EventType::OnInit,
        data: EventData::None,
    };
    let err = engine.call_event_handlers(&mut editor, &event).unwrap_err();
    assert!(err.to_string().contains("first fails"), "{err}");
    let ran: i64 = lua(&engine).globals().get("ran").unwrap();
    assert_eq!(ran, 1);
}

#[test]
fn annotation_fields_are_type_checked() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local buf = helix.buffer.get_current()
        assert(pcall(buf.set_annotations, buf, { { char_idx = 0, text = "x", offset = 3, is_line = true } }))
        assert(not pcall(buf.set_annotations, buf, { { char_idx = 0, text = "x", offset = -1 } }))
        assert(not pcall(buf.set_annotations, buf, { { char_idx = 0, text = "x", fg = {} } }))
        assert(not pcall(helix.buffer.annotation, { char_idx = 0, text = "x", virt_line_idx = 70000 }))
        assert(not pcall(helix.buffer.annotation, { char_idx = 0 }))
        local a = helix.buffer.annotation({ char_idx = 2, text = "y", virt_line_idx = 1 })
        assert(a.virt_line_idx == 1 and a.offset == 0 and a.is_line == false)
        "#,
    )
    .unwrap();
}

#[test]
fn buffer_columns_clamp_to_the_line() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local buf = helix.buffer.get_current()
        buf:insert(1, 0, "abc\r\ndef")
        -- past the end of line 1: before its line ending, not on line 2
        buf:insert(1, 100, "X")
        assert(buf:get_text() == "abcX\r\ndef\n", buf:get_text())
        buf:delete(1, 2, 1, 100)
        assert(buf:get_text() == "ab\r\ndef\n", buf:get_text())
        -- buffers can be found again by id
        local same = helix.buffer.get_by_id(buf:id())
        assert(same ~= nil and same:get_text() == buf:get_text())
        assert(helix.buffer.get_by_id("DocumentId(424242)") == nil)
        "#,
    )
    .unwrap();
}

#[test]
fn move_cursor_moves_the_selection() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local buf = helix.buffer.get_current()
        buf:insert(1, 0, "hello\nworld")
        helix.editor.set_cursor(0, 0)
        helix.editor.move_cursor("right", 3)
        assert(buf:get_cursor() == 3, buf:get_cursor())
        helix.editor.move_cursor("down")
        local c = helix.editor.get_cursor()
        assert(c.row == 1 and c.col == 3, tostring(c))
        helix.editor.move_cursor("left", 1e12)
        assert(helix.editor.get_cursor().col == 0)
        assert(not pcall(helix.editor.move_cursor, "sideways"))
        assert(helix.editor.focus():get_buffer():id() == buf:id())
        "#,
    )
    .unwrap();
}

#[test]
fn plugins_load_with_modules_config_and_editor_access() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let dir = tempfile::TempDir::new().unwrap();
    let plugin = dir.path().join("my-plugin");
    std::fs::create_dir_all(plugin.join("lib")).unwrap();
    std::fs::write(
        plugin.join("init.lua"),
        r#"
        local util = require("my-plugin.lib.util")
        helix.ui.notify("loaded " .. util.name)  -- the editor is available while loading
        helix.register_command({
            name = "show-config",
            description = "shows the config",
            handler = function()
                -- at run time the config is still this plugin's
                local config = helix.get_config()
                helix.editor.set_status(config.greeting .. config.nested.list[2], "warning")
                helix.ui.prompt("?", nil, function() end)
            end,
        })
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("lib").join("util.lua"),
        "return { name = 'util' }",
    )
    .unwrap();
    let config = crate::types::PluginConfig {
        enabled: true,
        plugin_dirs: vec![dir.path().to_path_buf()],
        plugins: vec![crate::types::IndividualPluginConfig {
            name: "my-plugin".into(),
            enabled: true,
            config: serde_json::json!({ "greeting": "hi ", "nested": { "list": [1, 2] } }),
            permissions: [crate::permissions::Permission::Commands]
                .into_iter()
                .collect(),
        }],
    };
    let manager = crate::PluginManager::new(config).unwrap();
    manager.engine().write().set_ui_handler(Arc::new(NoopUi));
    manager.initialize(&mut editor).unwrap();
    assert_eq!(manager.engine().read().plugins().count(), 1);
    assert_eq!(
        editor.get_status().map(|(s, _)| s.to_string()),
        Some("loaded util".to_string())
    );
    let commands = manager.get_commands();
    assert_eq!(commands[0].doc, "shows the config");
    manager
        .execute_command(&mut editor, "show-config", vec![])
        .unwrap();
    assert_eq!(
        editor.get_status().map(|(s, _)| s.to_string()),
        Some("hi 2".to_string())
    );
    // the prompt's callback belongs to the plugin that opened it
    let engine = manager.engine();
    let engine = engine.read();
    let keys: Vec<_> = engine.ui_callbacks.read().keys().cloned().collect();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, "my-plugin");
}

#[test]
fn set_cursor_stays_on_its_line_and_mode_names_match() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        helix.buffer.get_current():insert(1, 0, "ab\ncd")
        helix.editor.set_cursor(0, 100)
        local c = helix.editor.get_cursor()
        assert(c.row == 0 and c.col == 2, tostring(c))
        helix.on("mode_change", function(e) seen = e.old_mode .. ">" .. e.new_mode end)
        "#,
    )
    .unwrap();
    let event = PluginEvent {
        event_type: EventType::OnModeChange,
        data: EventData::ModeChange {
            old_mode: "Normal".into(),
            new_mode: "Insert".into(),
        },
    };
    engine.call_event_handlers(&mut editor, &event).unwrap();
    let seen: String = lua(&engine).globals().get("seen").unwrap();
    assert_eq!(seen, "normal>insert");
}

/// Runs `code` and returns the global `result` (a string).
fn result_of(engine: &LuaEngine, editor: &mut Editor, code: &str) -> String {
    run(engine, editor, code).unwrap();
    lua(engine).globals().get("result").unwrap()
}

#[test]
fn any_open_buffer_can_be_edited_and_read() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local focused = helix.buffer.get_current()
        focused:set_text("one\ntwo\nthree\n")
        -- a buffer in no window
        local other = helix.buffer.create("alpha\nbeta\n")
        other:insert(2, 0, ">")
        other:replace(1, 0, 1, 5, "ALPHA")
        local lines = other:get_lines(1)
        focused:delete(2, 0, 3, 0)
        result = table.concat({
            table.concat(lines, ","), #lines,
            focused:get_range(1, 0, 2, 5), type(other:id()),
            tostring(helix.buffer.get_current():id() == focused:id()),
        }, "|")
        "#,
    );
    // (the empty line after the final line ending is not one)
    assert_eq!(result, "ALPHA,>beta|2|one\nthree|string|true");
    let texts: Vec<String> = editor.documents().map(|d| d.text().to_string()).collect();
    assert!(texts.contains(&"one\nthree\n".to_string()), "{texts:?}");
    assert!(texts.contains(&"ALPHA\n>beta\n".to_string()), "{texts:?}");
}

#[test]
fn buffer_info_and_language() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        local before = tostring(b:get_language())
        b:set_language("rust")
        local info = b:get_info()
        result = table.concat({
            before, b:get_language(), info.line_ending, info.encoding,
            type(info.indent_width), tostring(info.readonly), type(info.text_width),
        }, "|")
        "#,
    );
    let line_ending = if cfg!(windows) { "crlf" } else { "lf" };
    assert_eq!(
        result,
        format!("nil|rust|{line_ending}|UTF-8|number|false|number")
    );
}

#[test]
fn windows_split_move_their_cursor_scroll_and_close() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        local lines = {}
        for i = 1, 100 do lines[i] = "line " .. i end
        b:set_text(table.concat(lines, "\n"))
        local first = helix.window.get_current()
        local second = helix.window.split("horizontal")
        local found = helix.window.get_by_id(second:id())
        second:set_cursor(10, 3)
        local cursor = second:get_cursor()
        second:set_selections({ {anchor = 0, head = 2}, {anchor = 5, head = 7} }, 2)
        local ranges, primary = second:get_selections()
        second:scroll_to(40)
        local area = second:get_area()
        local out = {
            #helix.window.list(), tostring(found:id() == second:id()),
            tostring(second:is_focused()), tostring(first:is_focused()),
            cursor.row .. ":" .. cursor.col, #ranges .. "/" .. primary,
            second:get_top_line(), type(area.width),
        }
        second:close()
        out[#out + 1] = #helix.window.list()
        out[#out + 1] = tostring(second:is_valid())
        local ok, err = pcall(function() first:close() end)
        out[#out + 1] = tostring(err):match("last window") or tostring(err)
        result = table.concat(out, "|")
        "#,
    );
    assert_eq!(
        result,
        "2|true|true|false|10:3|2/2|40|number|1|false|last window"
    );
}

#[test]
fn window_set_buffer_shows_another_buffer_and_keeps_the_focus() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local scratch = helix.buffer.create("scratch text")
        local left = helix.window.get_current()
        local right = helix.window.split("vertical")
        left:focus()
        right:set_buffer(scratch)
        result = table.concat({
            right:get_buffer():get_text(), tostring(left:is_focused()),
        }, "|")
        "#,
    );
    assert_eq!(result, "scratch text|true");
}

#[test]
fn selections_keep_their_primary_and_check_it() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        helix.buffer.get_current():set_text("abcdef")
        helix.editor.set_selections({ {anchor = 0, head = 1}, {anchor = 3, head = 4} }, 2)
        local ranges, primary = helix.editor.get_selections()
        local ok, err = pcall(helix.editor.set_selections, { {anchor = 0, head = 1} }, 3)
        result = #ranges .. "|" .. primary .. "|" .. tostring(err):match("out of range")
        "#,
    );
    assert_eq!(result, "2|2|out of range");
}

#[test]
fn options_are_read_as_lua_values() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local width = helix.editor.get_option("text-width")
        local wrap = helix.editor.get_option("soft-wrap.enable")
        local scrolloff = helix.editor.get_option("scrolloff")
        local missing = helix.editor.get_option("no.such.option")
        result = table.concat({
            type(width), width, type(wrap), type(scrolloff), tostring(missing),
            type(helix.editor.get_cwd()),
        }, "|")
        "#,
    );
    // (`scrolloff` is a section of its own here: a table)
    assert_eq!(result, "number|80|boolean|table|nil|string");
}

#[test]
fn key_bindings_are_stored_for_the_application() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        helix.keymap.set({ "normal", "select" }, "<space>z", function() end, { desc = "zap" })
        helix.keymap.set("insert", "C-s", function() end)
        local bindings = helix.keymap.list()
        local removed = helix.keymap.del("select", "<space>z")
        local again = helix.keymap.del("select", "<space>z")
        local ok, err = pcall(helix.keymap.set, "visual-block", "x", function() end)
        local ok2, err2 = pcall(helix.keymap.set, "normal", "x", 42)
        result = table.concat({
            #bindings, bindings[1].mode, bindings[1].keys, bindings[1].desc,
            tostring(bindings[1].command:match("^:plugin%-key%-p%-") ~= nil),
            bindings[3].keys, tostring(removed), tostring(again),
            tostring(err):match("Invalid mode") or tostring(err),
            tostring(err2):match("of type integer") or tostring(err2),
            #helix.keymap.list(),
        }, "|")
        "#,
    );
    assert_eq!(
        result,
        "3|normal|<space>z|zap|true|<C-s>|true|false|Invalid mode|of type integer|2"
    );
    let version: u64 = editor.plugin_keymaps.version;
    assert_eq!(version, 4, "set (2 modes), set, del");
    // the functions' commands are hidden from the command palette
    assert!(engine.get_commands().is_empty());
}

#[test]
fn key_bindings_need_the_keymaps_permission() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = LuaEngine::new().unwrap();
    engine
        .add_state("p", crate::permissions::Permissions::DEFAULT)
        .unwrap();
    let error = engine
        .run_as(
            "p",
            &mut editor,
            r#"helix.keymap.set("normal", "x", function() end)"#,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("'keymaps' permission"), "{error}");
}

/// A UI handler that records the pickers opened and what is asked of them.
#[derive(Default)]
struct RecordingUi {
    pickers: parking_lot::Mutex<Vec<(crate::types::PluginPicker, u64)>>,
    added: parking_lot::Mutex<Vec<(u64, Vec<crate::types::PluginPickerItem>)>>,
    closed: parking_lot::Mutex<Vec<u64>>,
    quickfix: parking_lot::Mutex<Vec<bool>>,
}

impl UiHandler for RecordingUi {
    fn prompt(&self, _: &mut Editor, _: String, _: Option<String>, _: String, _: u64) {}
    fn confirm(&self, _: &mut Editor, _: String, _: String, _: u64) {}
    fn picker(&self, _: &mut Editor, _: Vec<String>, _: String, _: String, _: u64) {}
    fn open_picker(
        &self,
        _: &mut Editor,
        picker: crate::types::PluginPicker,
        _: String,
        callback_id: u64,
    ) {
        self.pickers.lock().push((picker, callback_id));
    }
    fn picker_add(&self, picker_id: u64, items: Vec<crate::types::PluginPickerItem>) {
        self.added.lock().push((picker_id, items));
    }
    fn picker_close(&self, picker_id: u64) {
        self.closed.lock().push(picker_id);
    }
    fn quickfix(&self, open: bool) {
        self.quickfix.lock().push(open);
    }
}

#[test]
fn pickers_take_structured_items_and_hand_them_back() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let ui = Arc::new(RecordingUi::default());
    lua(&engine).set_app_data(UiHandlerWrapper(ui.clone()));
    run(
        &engine,
        &mut editor,
        r#"
        handle = helix.ui.picker{
            title = "Tasks",
            columns = { "Name", "Where" },
            items = {
                { columns = { "build", "Makefile" }, path = "Makefile", line = 3, end_line = 5,
                  data = { id = 7 } },
                "plain",
            },
            on_select = function(item, action)
                chosen = (type(item) == "table" and item.data.id or item) .. "/" .. action
            end,
        }
        handle:add({ { text = "late", data = { id = 9 } } })
        handle:close()
        "#,
    )
    .unwrap();

    let (picker, callback_id) = ui.pickers.lock()[0].clone();
    assert_eq!(picker.title, "Tasks");
    assert_eq!(picker.columns, ["Name", "Where"]);
    assert!(picker.preview, "an item has a path");
    let first = &picker.items[0];
    let (index, columns, lines): (usize, Vec<String>, Option<(usize, usize)>) =
        (first.index, first.columns.clone(), first.lines);
    assert_eq!(
        (index, columns, lines),
        (1, vec!["build".into(), "Makefile".into()], Some((2, 4)))
    );
    assert_eq!(
        first.path.as_deref(),
        Some(std::path::Path::new("Makefile"))
    );
    // a string item fills the first column
    assert_eq!(picker.items[1].columns, ["plain", ""]);
    let added = ui.added.lock().clone();
    assert_eq!(added.len(), 1);
    assert_eq!((added[0].0, added[0].1[0].index), (picker.id, 3));
    assert_eq!(*ui.closed.lock(), [picker.id]);

    // the choice comes back as the plugin's own item, with the action
    engine
        .handle_ui_callback(
            &mut editor,
            "p".into(),
            callback_id,
            serde_json::json!({ "index": 1, "action": "vsplit" }),
        )
        .unwrap();
    let chosen: String = lua(&engine).globals().get("chosen").unwrap();
    assert_eq!(chosen, "7/vsplit");
}

#[test]
fn pickers_check_their_items() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let ui = Arc::new(RecordingUi::default());
    lua(&engine).set_app_data(UiHandlerWrapper(ui.clone()));
    let error = run(
        &engine,
        &mut editor,
        r#"helix.ui.picker{ items = { true }, on_select = function() end }"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not a boolean"), "{error}");
    let error = run(
        &engine,
        &mut editor,
        r#"helix.ui.picker{ columns = {1,2,3,4,5,6,7,8,9}, on_select = function() end }"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("at most 8 columns"), "{error}");
    assert!(ui.pickers.lock().is_empty());
}

#[test]
fn the_quicklist_is_set_read_and_shown() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let ui = Arc::new(RecordingUi::default());
    lua(&engine).set_app_data(UiHandlerWrapper(ui.clone()));
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local scratch = helix.buffer.get_current()
        helix.quickfix.set({
            { path = "/tmp/a.txt", line = 3, text = "first" },
            { buffer = scratch, line = 1, end_line = 2, text = "second" },
        }, true)
        helix.quickfix.add({ path = "/tmp/b.txt", text = "third" })
        local entries = helix.quickfix.get()
        local count = #entries
        helix.quickfix.close()
        local ok, err = pcall(helix.quickfix.add, { text = "nowhere" })
        helix.quickfix.clear()
        result = table.concat({
            count, entries[1].path, entries[1].line, entries[1].text,
            tostring(entries[2].buffer:id() == scratch:id()), entries[2].end_line,
            entries[3].line, #helix.quickfix.get(),
            tostring(err):match("needs a `path` or a `buffer`"),
        }, "|")
        "#,
    );
    assert_eq!(
        result,
        "3|/tmp/a.txt|3|first|true|2|1|0|needs a `path` or a `buffer`"
    );
    assert_eq!(*ui.quickfix.lock(), [true, false]);
}

#[test]
fn the_explorer_is_asked_and_decorated() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "a").unwrap();
    let code = format!(
        r##"
        local root = helix.explorer.get_root()
        helix.explorer.decorate({path:?}, {{ text = "★", fg = "#ff0000" }})
        helix.explorer.decorate({path:?} .. "-gone", {{ text = "x" }})
        helix.explorer.decorate({path:?} .. "-gone", nil)
        helix.explorer.refresh()
        local ok, err = pcall(helix.explorer.decorate, {path:?}, {{ fg = "red" }})
        result = tostring(root) .. "|" .. tostring(helix.explorer.is_open()) .. "|"
            .. tostring(err):match("rrggbb")
        "##,
        path = file.to_string_lossy()
    );
    let result = result_of(&engine, &mut editor, &code);
    assert_eq!(result, "nil|false|rrggbb");
    let decorations = editor.plugin_explorer.decorations.clone();
    assert_eq!(decorations.len(), 1);
    let decoration = &decorations[&helix_stdx::path::canonicalize(&file)];
    let (text, fg, plugin): (&str, Option<&str>, &str) = (
        &decoration.text,
        decoration.fg.as_deref(),
        &decoration.plugin,
    );
    assert_eq!((text, fg, plugin), ("★", Some("#ff0000"), "p"));
    assert_eq!(
        editor.plugin_explorer.requests,
        [helix_view::explorer::ExplorerRequest::Refresh]
    );
    run(&engine, &mut editor, "helix.explorer.clear_decorations()").unwrap();
    assert!(editor.plugin_explorer.decorations.is_empty());
}

#[test]
fn status_line_segments_are_set_changed_and_removed() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r##"
        helix.statusline.set("branch", { text = "main", position = "left", style = "ui.text" })
        helix.statusline.set("clock", { text = "12:00", fg = "#ff0000", priority = 2 })
        helix.statusline.set("clock", { text = "12:01", fg = "#ff0000", priority = 2 })
        helix.statusline.set("here", { text = "only here", buffer = helix.buffer.get_current() })
        removed = helix.statusline.remove("here")
        again = helix.statusline.remove("here")
        ok, err = pcall(helix.statusline.set, "bad", { text = "x", position = "top" })
        ok2, err2 = pcall(helix.statusline.set, "bad", { text = "two\nlines" })
        err, err2 = tostring(err), tostring(err2)
        "##,
    )
    .unwrap();
    let segments = editor.plugin_statusline.clone();
    let texts: Vec<(&str, &str)> = segments
        .iter()
        .map(|s| (s.id.as_str(), s.text.as_str()))
        .collect();
    assert_eq!(texts, [("branch", "main"), ("clock", "12:01")]);
    let positions: Vec<helix_view::editor::SegmentPosition> =
        segments.iter().map(|s| s.position).collect();
    assert_eq!(
        positions,
        [
            helix_view::editor::SegmentPosition::Left,
            helix_view::editor::SegmentPosition::Right
        ]
    );
    let (scope, fg): (Option<&str>, Option<helix_view::graphics::Color>) =
        (segments[0].scope.as_deref(), segments[1].style.fg);
    assert_eq!(
        (scope, fg),
        (
            Some("ui.text"),
            Some(helix_view::graphics::Color::Rgb(255, 0, 0))
        )
    );
    let globals = lua(&engine).globals();
    let (removed, again): (bool, bool) = (
        globals.get("removed").unwrap(),
        globals.get("again").unwrap(),
    );
    assert_eq!((removed, again), (true, false));
    let (err, err2): (String, String) = (globals.get("err").unwrap(), globals.get("err2").unwrap());
    assert!(err.contains("Invalid position 'top'"), "{err}");
    assert!(err2.contains("one line"), "{err2}");
    run(&engine, &mut editor, "helix.statusline.clear()").unwrap();
    assert!(editor.plugin_statusline.is_empty());
}

#[test]
fn signs_and_highlights_are_kept_per_plugin() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_text("one\ntwo\nthree\n")
        b:set_signs({
            { line = 3, text = "●", style = "error" },
            { line = 1, text = "wide ⚑", fg = "green" },
        })
        b:set_highlights({
            { from = 0, to = 3, style = "warning" },
            { start_line = 2, end_line = 2, style = "info" },
        })
        "#,
    )
    .unwrap();
    let doc = helix_view::doc!(editor);
    let signs: Vec<(usize, &str)> = doc
        .plugin_signs
        .iter()
        .map(|s| (s.line, s.text.as_str()))
        .collect();
    // sorted by line; one cell each
    assert_eq!(signs, [(0, "w"), (2, "●")]);
    let highlights: Vec<(usize, usize, &str)> = doc
        .plugin_highlights
        .iter()
        .map(|h| (h.from, h.to, h.scope.as_str()))
        .collect();
    // line 2 to its end (before the line ending)
    assert_eq!(highlights, [(0, 3, "warning"), (4, 7, "info")]);

    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_signs({})
        ok, err = pcall(b.set_highlights, b, { { from = 0, to = 1 } })
        err = tostring(err)
        "#,
    )
    .unwrap();
    assert!(helix_view::doc!(editor).plugin_signs.is_empty());
    let err: String = lua(&engine).globals().get("err").unwrap();
    assert!(err.contains("needs a `style`"), "{err}");
}

#[test]
fn theme_styles_set_by_plugins_stay_over_theme_changes() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r##"
        helix.ui.set_style("ui.statusline", { fg = "#010203", modifiers = { "bold" } })
        helix.ui.set_style("plugin.custom", { bg = "blue" })
        "##,
    )
    .unwrap();
    let statusline = editor.theme.get("ui.statusline");
    let expected = helix_view::graphics::Style::default()
        .fg(helix_view::graphics::Color::Rgb(1, 2, 3))
        .add_modifier(helix_view::graphics::Modifier::BOLD);
    assert_eq!(statusline, expected);
    // a scope new to the theme is a highlight too
    let custom = editor.theme.find_highlight_exact("plugin.custom");
    assert!(custom.is_some());

    // another theme: the plugin's styles stay
    let theme = editor.theme_loader.load("base16_default").unwrap();
    editor.set_theme(theme).unwrap();
    assert_eq!(editor.theme.get("ui.statusline"), expected);

    run(
        &engine,
        &mut editor,
        r#"helix.ui.reset_style("ui.statusline")"#,
    )
    .unwrap();
    let restored: helix_view::graphics::Style = editor.theme.get("ui.statusline");
    assert_ne!(restored, expected);
    let name: &str = editor.theme.name();
    assert_eq!(name, "base16_default");
    let overrides: Vec<&str> = editor
        .plugin_theme_overrides
        .iter()
        .map(|(scope, _)| scope.as_str())
        .collect();
    assert_eq!(overrides, ["plugin.custom"]);
}

/// The status line example of the book (`book/src/plugins.md`) does what it says.
#[test]
fn the_status_line_example_of_the_book_works() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let book = include_str!("../../../book/src/plugins.md");
    let start = book
        .find("local changes = 0")
        .expect("the example is in the book");
    let end = start + book[start..].find("```").expect("the example ends");
    run(&engine, &mut editor, &book[start..end]).unwrap();

    let fire = |editor: &mut Editor, event_type: EventType| {
        let event = PluginEvent {
            event_type,
            data: EventData::None,
        };
        engine.call_event_handlers(editor, &event).unwrap();
    };
    let text = |editor: &Editor| -> String {
        editor
            .plugin_statusline
            .iter()
            .find(|s| s.id == "changes")
            .map(|s| s.text.clone())
            .unwrap_or_default()
    };
    fire(&mut editor, EventType::OnBufferChanged);
    fire(&mut editor, EventType::OnBufferChanged);
    let changed: String = text(&editor);
    assert_eq!(changed, "Δ2");
    fire(&mut editor, EventType::OnBufferPostSave);
    let saved: String = text(&editor);
    assert!(
        saved.starts_with("saved ") && saved.len() == "saved 12:34".len(),
        "{saved:?}"
    );
}

/// A UI handler that records timers that are due and popups.
#[derive(Default)]
struct AsyncUi {
    timers: parking_lot::Mutex<Vec<(String, u64)>>,
    delivered: parking_lot::Mutex<Vec<(String, u64, serde_json::Value)>>,
    popups: parking_lot::Mutex<Vec<crate::types::PluginPopup>>,
    closed: parking_lot::Mutex<Vec<u64>>,
}

impl UiHandler for AsyncUi {
    fn prompt(&self, _: &mut Editor, _: String, _: Option<String>, _: String, _: u64) {}
    fn confirm(&self, _: &mut Editor, _: String, _: String, _: u64) {}
    fn picker(&self, _: &mut Editor, _: Vec<String>, _: String, _: String, _: u64) {}
    fn timer(&self, plugin: String, id: u64) {
        self.timers.lock().push((plugin, id));
    }
    fn deliver(&self, plugin: String, id: u64, value: serde_json::Value) {
        self.delivered.lock().push((plugin, id, value));
    }
    fn popup(&self, popup: crate::types::PluginPopup) {
        self.popups.lock().push(popup);
    }
    fn popup_close(&self, id: u64) {
        self.closed.lock().push(id);
    }
}

/// An engine whose plugin `p` has the UI handler `ui` (set before its state is made, as the
/// application does).
fn engine_with_ui(ui: Arc<AsyncUi>) -> LuaEngine {
    let mut engine = LuaEngine::new().unwrap();
    engine.set_ui_handler(ui);
    engine
        .add_state("p", crate::permissions::Permissions::all())
        .unwrap();
    engine
}

fn timer_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

#[test]
fn timers_fire_once_or_repeatedly_until_cancelled() {
    let rt = timer_runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let ui = Arc::new(AsyncUi::default());
    let engine = engine_with_ui(ui.clone());
    run(
        &engine,
        &mut editor,
        r#"
        fired = {}
        once = helix.defer(5, function() fired[#fired + 1] = "once" end)
        tick = helix.every(10, function() fired[#fired + 1] = "tick" end)
        "#,
    )
    .unwrap();
    let (once, tick): (u64, u64) = (
        lua(&engine).globals().get("once").unwrap(),
        lua(&engine).globals().get("tick").unwrap(),
    );
    // the timers run on the runtime; a repeating one waits for its firing to be run
    rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(60)));
    let due: Vec<u64> = ui.timers.lock().iter().map(|(_, id)| *id).collect();
    assert_eq!(
        due,
        [once, tick],
        "one firing of the repeating timer while it waits"
    );

    for (plugin, id) in std::mem::take(&mut *ui.timers.lock()) {
        engine.fire_timer(&mut editor, &plugin, id).unwrap();
    }
    rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(30)));
    let due: Vec<u64> = ui.timers.lock().iter().map(|(_, id)| *id).collect();
    assert_eq!(
        due,
        [tick],
        "the one-shot timer is done, the other fired again"
    );
    // the one-shot timer is gone: firing it again does nothing
    engine.fire_timer(&mut editor, "p", once).unwrap();

    run(
        &engine,
        &mut editor,
        "cancelled = helix.cancel(tick); again = helix.cancel(tick)",
    )
    .unwrap();
    let (cancelled, again): (bool, bool) = (
        lua(&engine).globals().get("cancelled").unwrap(),
        lua(&engine).globals().get("again").unwrap(),
    );
    assert_eq!((cancelled, again), (true, false));
    ui.timers.lock().clear();
    rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(30)));
    assert!(ui.timers.lock().is_empty(), "a cancelled timer stops");
    let fired: Vec<String> = lua(&engine).globals().get("fired").unwrap();
    assert_eq!(fired, ["once", "tick"]);

    let error = run(&engine, &mut editor, "helix.defer(-1, function() end)")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Invalid delay"), "{error}");
}

#[test]
fn programs_run_in_the_background_and_need_shell() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let ui = Arc::new(AsyncUi::default());
    let engine = engine_with_ui(ui.clone());
    run(
        &engine,
        &mut editor,
        r#"
        helix.process.run({ "sh", "-c", "printf %s \"$0\"", "from argv" }, function(result)
            got = result.stdout .. "|" .. tostring(result.success) .. "|" .. math.type(result.code)
        end)
        "#,
    )
    .unwrap();
    // the program runs on the runtime; its result is delivered
    for _ in 0..200 {
        if !ui.delivered.lock().is_empty() {
            break;
        }
        rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(10)));
    }
    let (plugin, id, value) = ui.delivered.lock().pop().expect("the result was delivered");
    engine
        .handle_ui_callback(&mut editor, plugin, id, value)
        .unwrap();
    let got: String = lua(&engine).globals().get("got").unwrap();
    assert_eq!(got, "from argv|true|integer");

    let mut limited = LuaEngine::new().unwrap();
    limited.set_ui_handler(ui.clone());
    limited
        .add_state("p", crate::permissions::Permissions::DEFAULT)
        .unwrap();
    let error = limited
        .run_as(
            "p",
            &mut editor,
            r#"helix.process.run({ "true" }, function() end)"#,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("'shell' permission"), "{error}");
}

#[test]
fn language_server_requests_are_checked_by_method() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = LuaEngine::new().unwrap();
    engine
        .add_state("p", crate::permissions::Permissions::DEFAULT)
        .unwrap();
    // reading is allowed (but there is no server here), anything else needs `shell`
    let hover = engine
        .run_as(
            "p",
            &mut editor,
            r#"helix.lsp.request("textDocument/hover", {}, function() end)"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        hover.contains("No language server for the buffer"),
        "{hover}"
    );
    let execute = engine
        .run_as(
            "p",
            &mut editor,
            r#"helix.lsp.request("workspace/executeCommand", {}, function() end)"#,
        )
        .unwrap_err()
        .to_string();
    assert!(execute.contains("'shell' permission"), "{execute}");
    let notify = engine
        .run_as("p", &mut editor, r#"helix.lsp.notify("$/note", {})"#)
        .unwrap_err()
        .to_string();
    assert!(notify.contains("'shell' permission"), "{notify}");
    let position = engine
        .run_as("p", &mut editor, "helix.lsp.position_params()")
        .unwrap_err()
        .to_string();
    assert!(position.contains("has no file"), "{position}");
}

#[test]
fn popups_are_read_from_their_options() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let ui = Arc::new(AsyncUi::default());
    let engine = engine_with_ui(ui.clone());
    run(
        &engine,
        &mut editor,
        r##"
        handle = helix.ui.popup{
            title = "Info",
            lines = { "plain", { { text = "red", fg = "red" }, " and ", { text = "kw", style = "keyword" } } },
            position = "center", max_width = 40, persistent = true,
        }
        handle:update{ markdown = "# Done", position = { row = 3, col = 4 } }
        handle:close()
        ok, err = pcall(helix.ui.popup, { text = "a", markdown = "b" })
        err = tostring(err)
        "##,
    )
    .unwrap();
    use crate::types::{PopupContent, PopupPosition};
    let popups = ui.popups.lock().clone();
    assert_eq!(popups.len(), 2);
    let first = &popups[0];
    let (title, position, max_width, persistent) = (
        first.title.as_deref(),
        first.position,
        first.max_width,
        first.persistent,
    );
    assert_eq!(
        (title, position, max_width, persistent),
        (Some("Info"), PopupPosition::Center, Some(40), true)
    );
    let PopupContent::Lines(lines) = &first.content else {
        panic!("lines expected: {:?}", first.content);
    };
    let texts: Vec<Vec<&str>> = lines
        .iter()
        .map(|line| line.iter().map(|(text, _, _)| text.as_str()).collect())
        .collect();
    assert_eq!(texts, [vec!["plain"], vec!["red", " and ", "kw"]]);
    let (red, keyword) = (lines[1][0].2.fg, lines[1][2].1.as_deref());
    assert_eq!(
        (red, keyword),
        (Some(helix_view::graphics::Color::Red), Some("keyword"))
    );
    assert_eq!(
        (popups[1].content.clone(), popups[1].position, popups[1].id),
        (
            PopupContent::Markdown("# Done".into()),
            PopupPosition::At { row: 3, col: 4 },
            first.id
        )
    );
    assert_eq!(*ui.closed.lock(), [first.id]);
    let err: String = lua(&engine).globals().get("err").unwrap();
    assert!(
        err.contains("one of `text`, `lines` and `markdown`"),
        "{err}"
    );
}

#[test]
fn decorations_follow_the_edits() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_text("aaa\nbbb\nccc\n")
        b:set_signs({ { line = 2, text = "S" } })
        b:set_highlights({ { from = 4, to = 7, style = "warning" }, { from = 8, to = 11, style = "info" } })
        b:set_annotations({ { char_idx = 8, text = "note" } })
        b:set_panel({ char_idx = 4, items = { { text = "panel" } } })
        -- a new line at the top, and the third line deleted
        b:insert(1, 0, "new\n")
        b:delete(4, 0, 5, 0)
        "#,
    )
    .unwrap();
    let (view, doc) = helix_view::current_ref!(editor);
    assert_eq!(doc.text().to_string(), "new\naaa\nbbb\n");
    let sign_lines: Vec<usize> = doc.plugin_signs.iter().map(|s| s.line).collect();
    assert_eq!(sign_lines, [2], "the sign stays on `bbb`");
    let highlights: Vec<(usize, usize, &str)> = doc
        .plugin_highlights
        .iter()
        .map(|h| (h.from, h.to, h.scope.as_str()))
        .collect();
    assert_eq!(
        highlights,
        [(8, 11, "warning")],
        "`bbb` moved; `ccc` was deleted"
    );
    let annotation: usize = doc.plugin_annotations[&view.id][0].char_idx;
    assert_eq!(
        annotation, 12,
        "its line went: it moved to where the deletion was"
    );
    let panel: usize = doc.plugin_panels[&view.id].char_idx;
    assert_eq!(panel, 8, "with `bbb`");
}

/// A ```lua example of the book that starts with `start`.
fn book_example(start: &str) -> &'static str {
    let book = include_str!("../../../book/src/plugins.md");
    let begin = book.find(start).expect("the example is in the book");
    let end = begin + book[begin..].find("```").expect("the example ends");
    &book[begin..end]
}

/// The examples of the book for programs and language server requests compile, and the
/// program one runs (in Helix's repository: git is there).
#[test]
fn the_background_examples_of_the_book_work() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let ui = Arc::new(AsyncUi::default());
    let engine = engine_with_ui(ui.clone());
    let hover = book_example(r#"helix.register_command{ name = "hover-status""#);
    let compiled = lua(&engine).load(hover).into_function();
    assert!(compiled.is_ok(), "{:?}", compiled.err());

    let git = book_example(r#"helix.register_command{ name = "git-branch""#);
    run(&engine, &mut editor, git).unwrap();
    engine
        .execute_command(&mut editor, "git-branch", vec![])
        .unwrap();
    for _ in 0..500 {
        if !ui.delivered.lock().is_empty() {
            break;
        }
        rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(10)));
    }
    let (plugin, id, value) = ui.delivered.lock().pop().expect("git ran");
    let success: bool = value["success"].as_bool().unwrap_or(false);
    // the example's own assertions hold
    engine
        .handle_ui_callback(&mut editor, plugin, id, value)
        .unwrap();
    if success {
        let branch: Option<&str> = editor
            .plugin_statusline
            .iter()
            .find(|s| s.id == "branch")
            .map(|s| s.text.as_str());
        assert!(branch.is_none_or(|b| !b.is_empty() && !b.ends_with('\n')));
    }
}
