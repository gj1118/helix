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
