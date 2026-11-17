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

/// A binding for some languages only, in a named menu; the same keys for all languages are
/// another binding.
#[test]
fn key_bindings_may_be_for_some_languages_in_a_menu() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        helix.keymap.set("normal", "<space>tn", function() end,
            { desc = "New task", language = "plaintasks", menu = "Tasks" })
        helix.keymap.set("normal", "C-d", function() end, { language = { "plaintasks", "markdown" } })
        helix.keymap.set("normal", "C-d", function() end)
        local bindings = helix.keymap.list()
        local ok, err = pcall(helix.keymap.set, "normal", "x", function() end, { language = 3 })
        assert(type(bindings[1].language) == "table" and bindings[1].language[1] == "plaintasks")
        assert(math.type(#bindings) == "integer")
        result = table.concat({
            #bindings, bindings[1].menu, table.concat(bindings[2].language, ","),
            tostring(bindings[3].language), tostring(ok),
            tostring(err):match("a language name or a list") or tostring(err),
        }, "|")
        "#,
    );
    assert_eq!(
        result,
        "3|Tasks|plaintasks,markdown|nil|false|a language name or a list"
    );
    let bindings = &editor.plugin_keymaps.bindings;
    let tasks: Option<Vec<String>> = bindings[0].languages.clone();
    assert_eq!(tasks, Some(vec!["plaintasks".to_string()]));
    let applies: Vec<bool> = [Some("plaintasks"), Some("rust"), None]
        .into_iter()
        .map(|language| bindings[0].applies_to(language, None))
        .collect();
    assert_eq!(applies, [true, false, false]);
    assert!(bindings[2].applies_to(Some("rust"), None));
    assert!(editor.plugin_keymaps.has_language_bindings());
}

/// `helix.fs` tells whether a path exists, and whether it is a directory.
#[test]
fn file_system_paths_are_checked() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    let code = format!(
        r#"
        local dir = {dir:?}
        local values = {{
            helix.fs.exists(dir .. "/a.txt"), helix.fs.is_dir(dir .. "/a.txt"),
            helix.fs.exists(dir), helix.fs.is_dir(dir), helix.fs.exists(dir .. "/none"),
        }}
        for _, value in ipairs(values) do
            assert(type(value) == "boolean")
        end
        result = table.concat({{
            tostring(values[1]), tostring(values[2]), tostring(values[3]),
            tostring(values[4]), tostring(values[5]),
        }}, "|")
        "#,
        dir = dir.path().to_string_lossy()
    );
    let result = result_of(&engine, &mut editor, &code);
    assert_eq!(result, "true|false|true|true|false");
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
    let decoration = &decorations[&helix_stdx::path::canonicalize(&file)][0];
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

/// Each plugin's annotations are its own: `set_annotations` replaces only the caller's, and
/// `clear_annotations` removes them in every window of the buffer.
#[test]
fn annotations_belong_to_their_plugin() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = engine();
    engine
        .add_state("q", crate::permissions::Permissions::all())
        .unwrap();
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_text("aaa\nbbb\n")
        b:set_annotations({ { char_idx = 4, text = "p1" }, { char_idx = 0, text = "p0" } })
        "#,
    )
    .unwrap();
    engine
        .run_as(
            "q",
            &mut editor,
            r#"helix.buffer.get_current():set_annotations({ { char_idx = 1, text = "q" } })"#,
        )
        .unwrap();
    let texts = |editor: &Editor| -> Vec<(String, usize, String)> {
        let (view, doc) = helix_view::current_ref!(editor);
        doc.plugin_annotations
            .get(&view.id)
            .map(|annots| {
                annots
                    .iter()
                    .map(|a| (a.plugin.clone(), a.char_idx, a.text.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let owned = |plugin: &str, char_idx: usize, text: &str| {
        (plugin.to_string(), char_idx, text.to_string())
    };
    // both plugins', sorted by position
    assert_eq!(
        texts(&editor),
        [owned("p", 0, "p0"), owned("q", 1, "q"), owned("p", 4, "p1")]
    );
    // `p` replaces its own
    run(
        &engine,
        &mut editor,
        r#"helix.buffer.get_current():set_annotations({ { char_idx = 5, text = "p2" } })"#,
    )
    .unwrap();
    assert_eq!(texts(&editor), [owned("q", 1, "q"), owned("p", 5, "p2")]);
    // `q` clears its own
    engine
        .run_as(
            "q",
            &mut editor,
            r#"helix.buffer.get_current():clear_annotations()"#,
        )
        .unwrap();
    assert_eq!(texts(&editor), [owned("p", 5, "p2")]);
    run(
        &engine,
        &mut editor,
        r#"helix.buffer.get_current():set_annotations({})"#,
    )
    .unwrap();
    let (view, doc) = helix_view::current_ref!(editor);
    assert!(!doc.plugin_annotations.contains_key(&view.id));
}

/// `find_all` finds a regex like Helix's search: char indices (`to` exclusive), smart case,
/// `case_insensitive`, `max`; an invalid pattern is an error. `last_search` is the pattern of
/// the last search, or nil.
#[test]
fn find_all_finds_like_the_search() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_text("héllo Hello\nhello\n")

        -- smart case: no uppercase letter, any case
        local found = b:find_all("h.llo")
        assert(type(found) == "table" and #found == 3, #found)
        assert(math.type(found[1].from) == "integer" and found[1].from == 0, found[1].from)
        -- char indices: "héllo" is 5 chars (6 bytes)
        assert(found[1].to == 5, found[1].to)
        assert(found[2].from == 6 and found[2].to == 11)
        assert(found[3].from == 12 and found[3].to == 17)

        -- an uppercase letter: the case matters
        found = b:find_all("H.llo")
        assert(#found == 1 and found[1].from == 6, #found)
        -- unless asked otherwise
        found = b:find_all("H.llo", { case_insensitive = true })
        assert(#found == 3, #found)
        found = b:find_all("h.llo", { case_insensitive = false })
        assert(#found == 2, #found)

        -- at most `max`
        found = b:find_all("l", { max = 2 })
        assert(#found == 2 and found[2].from == 3, #found)
        -- multi-line, like the search
        found = b:find_all("^hello$")
        assert(#found == 1 and found[1].from == 12, #found)
        found = b:find_all("nothing")
        assert(type(found) == "table" and #found == 0)

        local ok, err = pcall(b.find_all, b, "(unclosed")
        assert(not ok and tostring(err):find("Invalid pattern"), tostring(err))

        assert(helix.editor.last_search() == nil)
        helix.editor.set_register("/", { "wor.d" })
        local last = helix.editor.last_search()
        assert(type(last) == "string" and last == "wor.d", last)
        "#,
    )
    .unwrap();
}

/// The `find_all` example of the book runs, with its assertions.
#[test]
fn the_find_all_example_of_the_book_works() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let example =
        book_example("local buffer = helix.buffer.get_current()\n  buffer:set_text(\"one two");
    run(&engine, &mut editor, example).unwrap();
    let doc = helix_view::doc!(editor);
    assert_eq!(doc.text().to_string(), "one two\nTwo\n");
}

#[test]
fn explorer_decorations_of_plugins_are_kept_apart() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = engine();
    engine
        .add_state("q", crate::permissions::Permissions::all())
        .unwrap();
    let path = "/tmp/a.txt";
    run(
        &engine,
        &mut editor,
        &format!(
            r##"helix.explorer.decorate({path:?}, {{ text = "12 KB", style = "comment",
                name = {{ fg = "#00ff00" }}, after = {{ text = "(3)", fg = "#0000ff" }} }})"##
        ),
    )
    .unwrap();
    engine
        .run_as(
            "q",
            &mut editor,
            &format!(r#"helix.explorer.decorate({path:?}, {{ text = "★" }})"#),
        )
        .unwrap();
    let decorations: Vec<helix_view::explorer::ExplorerDecoration> =
        editor.plugin_explorer.decorations[&std::path::PathBuf::from(path)].clone();
    assert_eq!(decorations.len(), 2);
    let first = &decorations[0];
    let (plugin, text, style): (&str, &str, Option<&str>) =
        (&first.plugin, &first.text, first.style.as_deref());
    assert_eq!((plugin, text, style), ("p", "12 KB", Some("comment")));
    let name_fg: Option<&str> = first.name_fg.as_deref();
    assert_eq!(name_fg, Some("#00ff00"));
    let (after, after_fg): (&str, Option<&str>) = (&first.after, first.after_fg.as_deref());
    assert_eq!((after, after_fg), ("(3)", Some("#0000ff")));
    let second: (&str, &str) = (&decorations[1].plugin, &decorations[1].text);
    assert_eq!(second, ("q", "★"));

    // a plugin's `nil` and clear only take its own away
    run(
        &engine,
        &mut editor,
        &format!("helix.explorer.decorate({path:?}, nil)"),
    )
    .unwrap();
    let left: Vec<String> = editor.plugin_explorer.decorations[&std::path::PathBuf::from(path)]
        .iter()
        .map(|d| d.plugin.clone())
        .collect();
    assert_eq!(left, vec!["q".to_string()]);
    engine
        .run_as("q", &mut editor, "helix.explorer.clear_decorations()")
        .unwrap();
    let empty: bool = editor.plugin_explorer.decorations.is_empty();
    assert!(empty);

    // a bad color in `name` is refused
    let error: String = run(
        &engine,
        &mut editor,
        &format!(r#"helix.explorer.decorate({path:?}, {{ name = {{ fg = "green" }} }})"#),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("rrggbb"), "{error}");
}

/// The id of the action of `keys` in `place`.
fn action_id(editor: &Editor, place: helix_view::plugin_ui::ActionPlace, keys: &str) -> u64 {
    let keys = helix_view::input::parse_macro(keys).unwrap();
    match editor.plugin_actions.matching(place, &keys) {
        helix_view::plugin_ui::ActionMatch::Action(id) => id,
        other => panic!("no action of {keys:?}: {other:?}"),
    }
}

#[test]
fn explorer_actions_run_with_the_entry() {
    use helix_view::plugin_ui::{ActionContext, ActionPlace};
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        helix.explorer.add_action({ key = "y", desc = "Copy the path", run = function(entry)
            result = entry.path .. "|" .. tostring(entry.is_dir) .. "|" .. type(entry.is_dir)
        end })
        local list = helix.explorer.list_actions()
        listed = #list .. "|" .. list[1].key .. "|" .. list[1].desc .. "|" .. list[1].plugin
        "#,
    )
    .unwrap();
    let listed: String = lua(&engine).globals().get("listed").unwrap();
    assert_eq!(listed, "1|y|Copy the path|p");
    let description: String = editor
        .plugin_actions
        .of(ActionPlace::Explorer)
        .next()
        .unwrap()
        .description
        .clone();
    assert_eq!(description, "Copy the path");

    let id: u64 = action_id(&editor, ActionPlace::Explorer, "y");
    engine
        .run_action(
            &mut editor,
            "p",
            id,
            &ActionContext::Explorer {
                path: "/tmp/src".into(),
                is_dir: true,
            },
        )
        .unwrap();
    let result: String = lua(&engine).globals().get("result").unwrap();
    assert_eq!(result, "/tmp/src|true|boolean");

    // the panel's keys are one key; an action needs a function
    let error: String = run(
        &engine,
        &mut editor,
        r#"helix.explorer.add_action({ key = "<space>y", run = function() end })"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("one key"), "{error}");
    let error: String = run(
        &engine,
        &mut editor,
        r#"helix.explorer.add_action({ key = "z", run = "no" })"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("function `run`"), "{error}");

    // removed: the key is free again, the action gone
    run(
        &engine,
        &mut editor,
        r#"removed = helix.explorer.remove_action("y")"#,
    )
    .unwrap();
    let removed: bool = lua(&engine).globals().get("removed").unwrap();
    assert!(removed);
    let empty: bool = editor.plugin_actions.is_empty(ActionPlace::Explorer);
    assert!(empty);
    let gone: Option<LuaFunction> = crate::lua::api::actions::function(lua(&engine), id).unwrap();
    assert!(gone.is_none());
    // a gone action does nothing
    let ran: bool = engine
        .run_action(
            &mut editor,
            "p",
            id,
            &ActionContext::Explorer {
                path: "/tmp".into(),
                is_dir: true,
            },
        )
        .is_ok();
    assert!(ran);
}

#[test]
fn actions_need_their_permission() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = LuaEngine::new().unwrap();
    engine
        .add_state("p", crate::permissions::Permissions::DEFAULT)
        .unwrap();
    for (code, permission) in [
        (
            r#"helix.explorer.add_action({ key = "y", run = function() end })"#,
            "'explorer' permission",
        ),
        (
            r#"helix.quickfix.add_action({ key = "y", run = function() end })"#,
            "'quickfix' permission",
        ),
        (
            r#"helix.diff.add_action({ key = "y", run = function() end })"#,
            "'diff' permission",
        ),
        (
            r#"helix.quickfix.decorate({ path = "/a", line = 1, text = "x" })"#,
            "'quickfix' permission",
        ),
        ("helix.diff.get()", "'diff' permission"),
    ] {
        let error: String = engine
            .run_as("p", &mut editor, code)
            .unwrap_err()
            .to_string();
        assert!(error.contains(permission), "{code}: {error}");
    }
    // listing them is reading
    engine
        .run_as(
            "p",
            &mut editor,
            "count = #helix.explorer.list_actions() + #helix.quickfix.list_actions()",
        )
        .unwrap();
    let count: i64 = engine.state("p").unwrap().globals().get("count").unwrap();
    assert_eq!(count, 0);
}

#[test]
fn quickfix_marks_and_actions() {
    use helix_view::plugin_ui::{ActionContext, ActionPlace};
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r##"
        helix.quickfix.decorate({ path = "/tmp/a.rs", line = 3, text = "●", fg = "#ff0000" })
        helix.quickfix.decorate({ path = "/tmp/a.rs", line = 4, text = "x" })
        helix.quickfix.decorate({ path = "/tmp/a.rs", line = 4 })
        helix.quickfix.add_action({ key = "x", run = function(entry)
            result = tostring(entry.path) .. "|" .. tostring(entry.buffer ~= nil) .. "|"
                .. entry.line .. "|" .. entry.end_line .. "|" .. math.type(entry.line) .. "|"
                .. entry.text
        end })
        "##,
    )
    .unwrap();
    let path = std::path::Path::new("/tmp/a.rs");
    let marks: Vec<(String, Option<String>)> = editor
        .plugin_quickfix_marks
        .get(path, 2)
        .iter()
        .map(|m| (m.text.clone(), m.fg.clone()))
        .collect();
    assert_eq!(marks, vec![("●".to_string(), Some("#ff0000".to_string()))]);
    let removed: usize = editor.plugin_quickfix_marks.get(path, 3).len();
    assert_eq!(removed, 0);

    let id: u64 = action_id(&editor, ActionPlace::Quickfix, "x");
    engine
        .run_action(
            &mut editor,
            "p",
            id,
            &ActionContext::Quickfix {
                target: helix_view::quicklist::Target::Path(path.to_path_buf()),
                start_line: 2,
                end_line: 4,
                text: "a.rs:3 fn main".into(),
            },
        )
        .unwrap();
    let result: String = lua(&engine).globals().get("result").unwrap();
    assert_eq!(result, "/tmp/a.rs|false|3|5|integer|a.rs:3 fn main");

    // an entry of a buffer without a file
    let doc = editor.tree.get(editor.tree.focus).doc;
    engine
        .run_action(
            &mut editor,
            "p",
            id,
            &ActionContext::Quickfix {
                target: helix_view::quicklist::Target::Doc(doc),
                start_line: 0,
                end_line: 0,
                text: String::new(),
            },
        )
        .unwrap();
    let result: String = lua(&engine).globals().get("result").unwrap();
    assert_eq!(result, "nil|true|1|1|integer|");

    run(&engine, &mut editor, "helix.quickfix.clear_decorations()").unwrap();
    let empty: bool = editor.plugin_quickfix_marks.is_empty();
    assert!(empty);
}

#[test]
fn diffs_are_opened_read_marked_and_closed() {
    use helix_view::plugin_ui::{ActionContext, ActionPlace};
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    // the buffer: "a\nB\nc\nd\n", against "a\nb\nc\n": line 2 changed, line 4 added
    run(
        &engine,
        &mut editor,
        r#"
        local buffer = helix.buffer.get_current()
        buffer:set_text("a\nB\nc\nd\n")
        local diff = helix.diff.open({ buffer = buffer, text = "a\nb\nc\n", title = "suggested" })
        opened = diff.title .. "|" .. #diff.changes .. "|" .. tostring(diff.git) .. "|"
            .. tostring(diff.right:id() == buffer:id()) .. "|" .. diff.left:get_text()
        "#,
    )
    .unwrap();
    let opened: String = lua(&engine).globals().get("opened").unwrap();
    assert_eq!(opened, "suggested|2|nil|true|a\nb\nc\n");
    let sessions: usize = editor.diff_sessions.len();
    assert_eq!(sessions, 1);

    run(
        &engine,
        &mut editor,
        r#"
        local diff = helix.diff.get()
        local c = diff.changes
        changes = ""
        for _, change in ipairs(c) do
            changes = changes .. change.index .. ":" .. change.kind .. ":" .. change.left_line
                .. "+" .. change.left_count .. ":" .. change.right_line .. "+"
                .. change.right_count .. ":" .. math.type(change.right_line) .. ";"
        end
        listed = #helix.diff.list()
        "#,
    )
    .unwrap();
    let changes: String = lua(&engine).globals().get("changes").unwrap();
    assert_eq!(
        changes,
        "1:changed:2+1:2+1:integer;2:added:4+0:4+1:integer;"
    );
    let listed: i64 = lua(&engine).globals().get("listed").unwrap();
    assert_eq!(listed, 1);

    // the change under the cursor: on line 4 (the added one)
    let view = editor.tree.focus;
    let doc_id = editor.tree.get(view).doc;
    let doc = editor.documents.get_mut(&doc_id).unwrap();
    let line_start = doc.text().line_to_char(3);
    doc.set_selection(view, helix_core::Selection::point(line_start));
    run(
        &engine,
        &mut editor,
        r#"
        local change = helix.diff.change()
        under = change.index .. "|" .. change.kind
        helix.diff.mark(change.index, { text = "AI", style = "info" })
        local ok, err = pcall(helix.diff.mark, 9, { text = "x" })
        bad = tostring(err):match("no change 9") or tostring(err)
        "#,
    )
    .unwrap();
    let under: String = lua(&engine).globals().get("under").unwrap();
    assert_eq!(under, "2|added");
    let bad: String = lua(&engine).globals().get("bad").unwrap();
    assert_eq!(bad, "no change 9");
    let marks: Vec<(usize, String, Option<String>)> = editor.diff_sessions[0]
        .plugin_marks
        .iter()
        .map(|m| (m.change, m.mark.text.clone(), m.mark.style.clone()))
        .collect();
    assert_eq!(marks, vec![(1, "AI".to_string(), Some("info".to_string()))]);

    // an action of the diff views gets the diff and its change under the cursor
    run(
        &engine,
        &mut editor,
        r#"helix.diff.add_action({ key = "<space>e", run = function(diff)
            ran = diff.title .. "|" .. diff.change.index .. "|" .. diff.change.kind
        end })"#,
    )
    .unwrap();
    let id: u64 = action_id(&editor, ActionPlace::Diff, "<space>e");
    engine
        .run_action(&mut editor, "p", id, &ActionContext::Diff { view })
        .unwrap();
    let ran: String = lua(&engine).globals().get("ran").unwrap();
    assert_eq!(ran, "suggested|2|added");

    // a window id (of the events) works as the window
    run(
        &engine,
        &mut editor,
        r#"
        local id = helix.window.get_current():id()
        by_id = #helix.diff.get(id).changes
        helix.diff.clear_marks(id)
        closed = tostring(helix.diff.close()) .. "|" .. tostring(helix.diff.get())
        "#,
    )
    .unwrap();
    let by_id: i64 = lua(&engine).globals().get("by_id").unwrap();
    assert_eq!(by_id, 2);
    let closed: String = lua(&engine).globals().get("closed").unwrap();
    assert_eq!(closed, "true|nil");
    let sessions: usize = editor.diff_sessions.len();
    assert_eq!(sessions, 0);
}

#[test]
fn a_plugin_publishes_diagnostics_of_its_own() {
    use helix_core::diagnostic::{DiagnosticProvider, Severity};
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_text("let x = 1\n  todo here\nlast\n")
        count = b:set_diagnostics({
            { line = 2, message = "a todo", severity = "info", code = "T1" },
            { start_line = 1, start_col = 4, end_line = 1, end_col = 5, message = "unused x" },
            { from = 0, to = 3, message = "keyword", severity = "hint", source = "mine" },
        })
        "#,
    )
    .unwrap();
    let count: usize = lua(&engine).globals().get("count").unwrap();
    assert_eq!(count, 3_usize);
    let doc = helix_view::doc!(editor);
    type Found = (usize, usize, String, Option<Severity>, Option<String>);
    let mut found: Vec<Found> = doc
        .diagnostics()
        .iter()
        .map(|d| {
            (
                d.range.start,
                d.range.end,
                d.message.clone(),
                d.severity,
                d.source.clone(),
            )
        })
        .collect();
    found.sort_by_key(|d| d.0);
    let plugin = "p".to_string();
    assert_eq!(
        found,
        vec![
            (
                0,
                3,
                "keyword".to_string(),
                Some(Severity::Hint),
                Some("mine".to_string())
            ),
            (
                4,
                5,
                "unused x".to_string(),
                Some(Severity::Warning),
                Some(plugin.clone())
            ),
            // line 2 from its first non-blank char to its end
            (
                12,
                21,
                "a todo".to_string(),
                Some(Severity::Info),
                Some(plugin.clone())
            ),
        ]
    );
    assert!(doc.diagnostics().iter().all(|d| matches!(
        &d.provider,
        DiagnosticProvider::Plugin { name } if name.as_ref() == plugin
    )));

    // replaced whole; a bad severity is an error and changes nothing
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:set_diagnostics({ { line = 3, message = "only one" } })
        ok, err = pcall(b.set_diagnostics, b, { { line = 1, message = "x", severity = "fatal" } })
        err = tostring(err)
        "#,
    )
    .unwrap();
    let messages: Vec<String> = helix_view::doc!(editor)
        .diagnostics()
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(messages, vec!["only one".to_string()]);
    let err: String = lua(&engine).globals().get("err").unwrap();
    assert!(err.contains("Unknown severity 'fatal'"), "{err}");

    run(
        &engine,
        &mut editor,
        "helix.buffer.get_current():clear_diagnostics()",
    )
    .unwrap();
    assert!(helix_view::doc!(editor).diagnostics().is_empty());
}

#[test]
fn completion_sources_are_registered_asked_and_removed() {
    use helix_view::plugin_ui::PluginCompletionRequest;
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        helix.completion.register_source({
            name = "words",
            languages = { "markdown" },
            triggers = ":",
            complete = function(ctx)
                return { "plain", { label = "rich", insert = "RICH", kind = "k", documentation = "d" } }
            end,
        })
        -- the same name again: replaced
        helix.completion.register_source({
            name = "words",
            complete = function(ctx) return { ctx.prefix .. "-" .. ctx.line .. "-" .. ctx.col } end,
        })
        listed = helix.completion.list_sources()
        "#,
    )
    .unwrap();
    let listed: mlua::Table = lua(&engine).globals().get("listed").unwrap();
    assert_eq!(listed.raw_len(), 1_usize);
    assert_eq!(editor.plugin_completion_sources.len(), 1_usize);
    let source = editor.plugin_completion_sources[0].clone();
    assert!(source.languages.is_empty(), "the second registration's");

    let request = PluginCompletionRequest {
        document: helix_view::doc!(editor).id(),
        line: 2,
        col: 5,
        prefix: "wor".into(),
        before: "  wor".into(),
        manual: false,
    };
    let items = engine
        .complete(&mut editor, "p", source.id, &request)
        .unwrap();
    let labels: Vec<String> = items.into_iter().map(|item| item.label).collect();
    assert_eq!(
        labels,
        vec!["wor-3-5".to_string()],
        "lines are 1-based for the plugin"
    );

    // a bad item is an error; removing the source
    run(
        &engine,
        &mut editor,
        r#"
        helix.completion.register_source({ name = "bad", complete = function() return { 42 } end })
        "#,
    )
    .unwrap();
    let bad = editor
        .plugin_completion_sources
        .iter()
        .find(|s| s.name == "bad")
        .unwrap()
        .id;
    let err = engine
        .complete(&mut editor, "p", bad, &request)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("a completion item is a string or a table"),
        "{err}"
    );
    run(
        &engine,
        &mut editor,
        r#"removed = helix.completion.remove_source("bad"); again = helix.completion.remove_source("bad")"#,
    )
    .unwrap();
    let removed: bool = lua(&engine).globals().get("removed").unwrap();
    let again: bool = lua(&engine).globals().get("again").unwrap();
    assert_eq!((removed, again), (true, false));
    assert_eq!(editor.plugin_completion_sources.len(), 1_usize);
}

#[test]
fn the_syntax_tree_is_walked_and_queried() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        assert(b:has_syntax() == false)
        local ok, err = pcall(b.syntax_root, b)
        assert(not ok and tostring(err):find("no syntax tree", 1, true), tostring(err))
        b:set_language("rust")
        -- "é" is two bytes: the chars and the bytes of the nodes differ after it
        b:set_text("fn main() {\n    let é = add(1, 2);\n}\n")
        assert(b:has_syntax() == true)

        local root = b:syntax_root()
        assert(root.type == "source_file" and root.from == 0 and root.named == true)

        -- the identifier `add`: line 2, col 12 (chars)
        local at = b:line_to_char(1) + 12
        local node = b:syntax_node(at)
        assert(node.type == "identifier" and b:syntax_text(node) == "add", node.type)
        assert(math.type(node.from) == "integer" and node.from == at and node.to == at + 3)
        assert(node.start_line == 2 and node.start_col == 12 and node.end_col == 15)
        assert(node.start_byte == node.from + 1, "a byte more than chars after é")

        local call = b:syntax_parent(node)
        assert(call.type == "call_expression", call.type)
        local args = b:syntax_children(call, { named = true })
        assert(#args == 2 and args[2].type == "arguments", #args)
        local all = b:syntax_children(args[2])
        local kinds = {}
        for _, child in ipairs(all) do table.insert(kinds, child.type) end
        assert(table.concat(kinds, " ") == "( integer_literal , integer_literal )", table.concat(kinds, " "))

        -- an anonymous node with named = false; a range
        local paren = b:syntax_node(at + 3, { named = false })
        assert(paren.type == "(" and paren.named == false, paren.type)
        local spanning = b:syntax_node(at, { to = at + 9 })
        assert(spanning.type == "call_expression", spanning.type)

        -- queries: matches in the document's order, patterns 1-based, text predicates
        local matches = b:syntax_query([[
            (function_item name: (identifier) @name)
            ((integer_literal) @int (#eq? @int "2"))
        ]])
        assert(#matches == 2, #matches)
        assert(matches[1].pattern == 1 and matches[1].captures[1].name == "name")
        assert(b:syntax_text(matches[1].captures[1].node) == "main")
        assert(matches[2].pattern == 2 and b:syntax_text(matches[2].captures[1].node) == "2")
        local ranged = b:syntax_query("(integer_literal) @int", { from = 0, to = at + 5 })
        assert(#ranged == 1, #ranged)
        assert(#b:syntax_query("(integer_literal) @int", { max = 1 }) == 1)
        local bad_ok, bad = pcall(b.syntax_query, b, "(no_such_node) @x")
        assert(not bad_ok and tostring(bad):find("Invalid query", 1, true), tostring(bad))

        -- a node from before an edit that removed it is an error
        b:set_text("fn main() {}\n")
        local gone_ok, gone = pcall(b.syntax_parent, b, node)
        assert(not gone_ok and tostring(gone):find("no longer in the tree", 1, true), tostring(gone))
        result = table.concat({ root.type, node.type, call.type, tostring(#matches) }, "|")
        "#,
    );
    assert_eq!(
        result,
        "source_file|identifier|call_expression|2".to_string()
    );
}

#[test]
fn a_syntax_node_is_found_in_an_injected_language() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r##"
        local b = helix.buffer.get_current()
        b:set_language("markdown")
        b:set_text("# Title\n\n```rust\nlet value = 1;\n```\n")
        local node = b:syntax_node(b:line_to_char(3) + 5)
        assert(type(node) == "table" and math.type(node.start_line) == "integer")
        result = node.type .. "|" .. b:syntax_text(node) .. "|" .. node.start_line
        "##,
    );
    assert_eq!(result, "identifier|value|4".to_string());
}

#[test]
fn the_syntax_tree_example_of_the_book_runs() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
local buffer = helix.buffer.get_current()
buffer:set_language("rust") -- needs `edit`
buffer:set_text("fn main() {\n    let total = add(1, 2);\n}\n")
local add = buffer:syntax_node(buffer:line_to_char(1) + 16)
assert(type(add) == "table" and add.type == "identifier")
assert(math.type(add.start_line) == "integer" and add.start_line == 2)
assert(buffer:syntax_text(add) == "add")
local call = buffer:syntax_parent(add)
assert(call.type == "call_expression")
local matches = buffer:syntax_query("(function_item name: (identifier) @name)")
assert(#matches == 1 and matches[1].captures[1].name == "name")
assert(buffer:syntax_text(matches[1].captures[1].node) == "main")
result = "ran"
        "#,
    );
    assert_eq!(result, "ran".to_string());
}

#[test]
fn plugins_emit_events_of_their_own_and_hooks_can_say_no() {
    use crate::types::{EventType, EventValue, PluginEvent};
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let mut engine = engine();
    engine
        .add_state("q", crate::permissions::Permissions::all())
        .unwrap();
    // `q` listens, `p` emits
    engine
        .run_as(
            "q",
            &mut editor,
            r#"
            heard = {}
            helix.on("git:branch", function(event)
                assert(type(event.data) == "table" and math.type(event.data.ahead) == "integer")
                table.insert(heard, event.type .. "|" .. event.from .. "|" .. event.data.name
                    .. "|" .. event.data.ahead .. "|" .. tostring(event.data.tags[2]))
                -- a handler may emit: heard in the next round
                if event.data.name == "main" then helix.emit("git:again", "x") end
            end)
            helix.on("git:again", function(event) table.insert(heard, "again|" .. event.data) end)
            "#,
        )
        .unwrap();
    run(
        &engine,
        &mut editor,
        r#"
        helix.emit("git:branch", { name = "main", ahead = 2, tags = { "a", "b" } })
        helix.emit("git:nobody", nil)
        local ok, err = pcall(helix.emit, "nonamespace", 1)
        assert(not ok and tostring(err):find("namespace:name", 1, true), tostring(err))
        local ok2, err2 = pcall(helix.emit, "git:fn", function() end)
        assert(not ok2 and tostring(err2):find("The data of an event", 1, true), tostring(err2))
        local ok3, err3 = pcall(helix.on, "nonamespace", function() end)
        assert(not ok3 and tostring(err3):find("Invalid event type", 1, true), tostring(err3))
        "#,
    )
    .unwrap();
    // nothing is heard before the frame fires them
    let heard: mlua::Table = engine.state("q").unwrap().globals().get("heard").unwrap();
    assert_eq!(heard.raw_len(), 0_usize);
    assert!(engine.has_custom_events());
    engine.fire_custom_events(&mut editor);
    engine.fire_custom_events(&mut editor);
    assert!(!engine.has_custom_events());
    let heard: Vec<String> = heard.sequence_values().collect::<LuaResult<_>>().unwrap();
    assert_eq!(
        heard,
        vec!["git:branch|p|main|2|b".to_string(), "again|x".to_string()]
    );

    // hooks: handlers in order until one says `false`; an error does not stop them
    run(
        &engine,
        &mut editor,
        r#"
        calls = {}
        helix.on("command_pre", function(event)
            table.insert(calls, "first:" .. event.name .. ":" .. event.args)
            error("boom")
        end)
        helix.on("command_pre", function(event)
            table.insert(calls, "second")
            if event.name == "write" then return false end
            return nil
        end)
        helix.on("command_pre", function() table.insert(calls, "third") end)
        "#,
    )
    .unwrap();
    let hook = |name: &str| {
        PluginEvent::with_fields(
            EventType::OnCommandPre,
            vec![
                ("name", EventValue::from(name)),
                ("args", EventValue::from("a b")),
            ],
        )
    };
    assert!(!engine.call_hook(&mut editor, &hook("write")));
    assert!(engine.call_hook(&mut editor, &hook("quit")));
    let calls: mlua::Table = lua(&engine).globals().get("calls").unwrap();
    let calls: Vec<String> = calls.sequence_values().collect::<LuaResult<_>>().unwrap();
    assert_eq!(
        calls,
        vec![
            "first:write:a b",
            "second",
            "first:quit:a b",
            "second",
            "third"
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<String>>()
    );
}

#[test]
fn gutter_columns_and_window_bars_of_plugins() {
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
        b:set_gutter("marks", {
            width = 2,
            cells = { { line = 2, text = "●●●", style = "error" }, { line = 1, text = "a" } },
        })
        b:set_gutter("right", { position = "right", cells = { { line = 3, text = "r" } } })
        local ok, err = pcall(b.set_gutter, b, "wide", { width = 21 })
        assert(not ok and tostring(err):find("1 to 20 cells", 1, true), tostring(err))
        local ok2, err2 = pcall(b.set_gutter, b, "bad", { position = "top" })
        assert(not ok2 and tostring(err2):find("Invalid position", 1, true), tostring(err2))
        local ok3, err3 = pcall(b.set_gutter, b, "nl", { cells = { { line = 1, text = "a\nb" } } })
        assert(not ok3 and tostring(err3):find("one line", 1, true), tostring(err3))

        helix.winbar.set("file", { text = "{name}{modified}" })
        helix.winbar.set("lang", { text = "{language}", position = "right", priority = 2 })
        local listed = helix.winbar.list()
        assert(#listed == 2 and listed[1].position == "left" and math.type(listed[2].priority) == "integer")
        local ok4, err4 = pcall(helix.winbar.set, "x", { text = "a\nb" })
        assert(not ok4 and tostring(err4):find("one line", 1, true), tostring(err4))
        "#,
    )
    .unwrap();
    {
        let (view, doc) = helix_view::current_ref!(editor);
        assert_eq!(doc.plugin_gutters.len(), 2_usize);
        let marks = &doc.plugin_gutters[0];
        assert_eq!((marks.width, marks.left), (2_usize, true));
        // cut to the width, sorted by line
        let cells: Vec<(usize, &str)> = marks
            .cells
            .iter()
            .map(|cell| (cell.line, cell.text.as_str()))
            .collect();
        assert_eq!(cells, vec![(0_usize, "a"), (1, "●●")]);
        assert_eq!(doc.plugin_gutters_width(), 3_usize);
        // the bar takes the top row
        assert!(view.bar);
        assert_eq!(view.inner_area(doc).y, view.area.y + 1);
    }

    // a line inserted above: the cells move down with their lines
    run(
        &engine,
        &mut editor,
        r#"
        local b = helix.buffer.get_current()
        b:insert(1, 0, "zero\n")
        b:set_gutter("right", nil)
        assert(helix.winbar.remove("file") == true and helix.winbar.remove("file") == false)
        helix.winbar.clear()
        "#,
    )
    .unwrap();
    let (view, doc) = helix_view::current_ref!(editor);
    let lines: Vec<usize> = doc.plugin_gutters[0]
        .cells
        .iter()
        .map(|cell| cell.line)
        .collect();
    assert_eq!(lines, vec![1_usize, 2]);
    assert_eq!(doc.plugin_gutters.len(), 1_usize);
    assert!(!view.bar);
    assert_eq!(view.inner_area(doc).y, view.area.y);
}

#[test]
fn the_gutter_and_winbar_examples_of_the_book_run() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
helix.buffer.get_current():set_text("a\nb\nc\n")
local buffer = helix.buffer.get_current()
buffer:set_gutter("coverage", {
    width = 2,
    cells = { { line = 1, text = "✔", fg = "green" }, { line = 2, text = "✘", style = "error" } },
})
local columns = buffer:get_gutters()
assert(type(columns) == "table" and columns[1].id == "coverage")
assert(math.type(columns[1].width) == "integer" and columns[1].width == 2)
assert(columns[1].cells[2].line == 2 and columns[1].cells[2].text == "✘")
buffer:set_gutter("coverage", nil)
assert(#buffer:get_gutters() == 0)-- the file of each window, and the language on the right of the focused one's
helix.winbar.set("file", { text = "{path}{modified}", style = "ui.text" })
helix.winbar.set("language", {
    text = "{language}",
    position = "right",
    window = helix.window.get_current(),
})
local segments = helix.winbar.list()
assert(type(segments) == "table" and #segments >= 2)
assert(segments[1].position == "left" and math.type(segments[1].priority) == "integer")

result = "ran"
        "#,
    );
    assert_eq!(result, "ran".to_string());
}

#[test]
fn floats_docked_windows_and_buffer_keys_of_plugins() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    run(
        &engine,
        &mut editor,
        r#"
        local list = helix.buffer.create("one\ntwo\nthree\nfour\n")
        first = helix.ui.float({
            buffer = list, title = "Items", width = 20, height = 2, cursor = 3,
            keys = { ["<ret>"] = function(ctx) picked = ctx.line end },
        })
        assert(first:is_open() and first:is_focused() and first:get_cursor() == 3)
        assert(math.type(first:id()) == "integer")
        first:set_cursor(99)
        assert(first:get_cursor() == 5, first:get_cursor()) -- the last line (after the newline)
        -- a second focused float takes the focus
        second = helix.ui.float({ buffer = list, position = { row = 1, col = 2 }, focus = true })
        assert(second:is_focused() and not first:is_focused())
        first:focus()
        assert(first:is_focused() and not second:is_focused())
        second:update({ title = "Other", width = 5, position = "cursor" })
        assert(second:get_buffer():id() == list:id())
        assert(second:close() == true and second:close() == false and not second:is_open())
        local ok, err = pcall(second.get_cursor, second)
        assert(not ok and tostring(err):find("is closed", 1, true), tostring(err))
        local ok2, err2 = pcall(helix.ui.float, { buffer = list, keys = { gg = function() end } })
        assert(not ok2 and tostring(err2):find("one key", 1, true), tostring(err2))
        local ok3, err3 = pcall(helix.ui.float, { title = "x" })
        assert(not ok3 and tostring(err3):find("needs a `buffer`", 1, true), tostring(err3))

        -- a docked window of the buffer, and a key for that buffer only
        side = helix.window.dock(list, { side = "left", size = 25 })
        assert(side:get_buffer():id() == list:id() and not side:is_focused())
        helix.keymap.set("normal", "<space>z", function() end, { buffer = list })
        local bindings = helix.keymap.list()
        assert(#bindings == 1 and bindings[1].buffer == list:id(), tostring(bindings[1].buffer))
        local ok4, err4 = pcall(helix.window.dock, list, { side = "middle" })
        assert(not ok4 and tostring(err4):find("Invalid side", 1, true), tostring(err4))
        "#,
    )
    .unwrap();
    assert_eq!(editor.plugin_floats.len(), 1_usize);
    let float = &editor.plugin_floats[0];
    assert_eq!(
        (
            float.title.as_deref(),
            float.width,
            float.height,
            float.cursor_line,
            float.top_line
        ),
        (Some("Items"), 20_u16, 2_u16, 4_usize, 3_usize)
    );
    assert_eq!(float.keys.len(), 1_usize);
    // the key runs the plugin's function with the cursor line
    let (id, function) = (float.id, float.keys[0].1);
    engine.run_float(&mut editor, "p", id, function).unwrap();
    let picked: i64 = lua(&engine).globals().get("picked").unwrap();
    assert_eq!(picked, 5_i64);

    // the docked window: 25 columns on the left, the focus where it was
    let docked: Vec<_> = editor
        .tree
        .views()
        .filter(|(view, _)| view.area.x == 0 && view.area.width == 25)
        .collect();
    assert_eq!(docked.len(), 1_usize);
    assert!(!docked[0].1);
    let binding = &editor.plugin_keymaps.bindings[0];
    assert!(binding.applies_to(None, Some(docked[0].0.doc)));
    assert!(!binding.applies_to(None, Some(helix_view::doc!(editor).id())));
}

#[test]
fn the_float_and_dock_examples_of_the_book_run() {
    let rt = runtime();
    let _rt = rt.enter();
    let mut editor = test_editor();
    let engine = engine();
    let result = result_of(
        &engine,
        &mut editor,
        r#"
local list = helix.buffer.create("alpha\nbeta\ngamma\n") -- needs `edit`
local picked = nil
local float = helix.ui.float({
    buffer = list,
    title = "Pick one",
    width = 20,
    height = 3,
    keys = {
        ["<ret>"] = function(ctx)
            picked = ctx.buffer:get_line(ctx.line)
            ctx.float:close()
        end,
    },
})
assert(float:is_open() and float:is_focused())
float:set_cursor(2)
assert(math.type(float:get_cursor()) == "integer" and float:get_cursor() == 2)
assert(float:close() == true and not float:is_open())
local outline = helix.buffer.create("fn main\nfn helper\n") -- needs `edit`
local window = helix.window.dock(outline, { side = "right", size = 24 })
assert(window:get_buffer():id() == outline:id() and not window:is_focused())
local area = window:get_area()
assert(math.type(area.width) == "integer" and area.width == 24)
helix.keymap.set("normal", "<ret>", function()
    helix.ui.notify("jump to " .. outline:get_line(window:get_cursor().row + 1))
end, { buffer = outline }) -- needs `keymaps`
result = "ran"
        "#,
    );
    assert_eq!(result, "ran".to_string());
}
