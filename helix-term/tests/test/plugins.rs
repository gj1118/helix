use super::*;

use helix_term::application::Application;

/// The tests never load plugins: the default plugin directories are the user's own, and
/// every test would otherwise run with (and load again) whatever plugins are installed.
#[tokio::test(flavor = "multi_thread")]
async fn tests_do_not_load_the_users_plugins() -> anyhow::Result<()> {
    let app = helpers::AppBuilder::new().build()?;
    assert!(!app.plugins_enabled());
    Ok(())
}

/// Sends `keys` to the application and runs its event loop until it is idle.
async fn send_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::{Event, KeyEvent};
    #[cfg(not(windows))]
    use termina::event::{Event, KeyEvent};

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut rx).await;
    Ok(())
}

/// What the probe plugin recorded in register `z`.
fn recorded(app: &Application) -> Vec<String> {
    app.editor
        .registers
        .read('z', &app.editor)
        .and_then(|mut values| values.next().map(|value| value.into_owned()))
        .map(|events| events.split(',').map(String::from).collect())
        .unwrap_or_default()
}

/// A test's own `[plugins]` table loads plugins (from the given directory only), hands them
/// their configuration and fires the editor events.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_get_their_config_and_the_editor_events() -> anyhow::Result<()> {
    let plugins = tempfile::tempdir()?;
    let probe = plugins.path().join("probe");
    std::fs::create_dir(&probe)?;
    std::fs::write(
        probe.join("init.lua"),
        r#"
local seen = {}
local function record(name)
    table.insert(seen, name)
    helix.editor.set_register("z", { table.concat(seen, ",") })
end
for _, name in ipairs({ "ready", "buffer_changed", "buffer_pre_save", "buffer_post_save", "buffer_close" }) do
    helix.on(name, function() record(name) end)
end
helix.on("ready", function()
    record("greeting=" .. tostring((helix.get_config() or {}).greeting))
end)
-- runs before the text is written: the edit is saved too
helix.on("buffer_pre_save", function()
    helix.buffer.get_current():insert(1, 0, "PRE ")
end)
"#,
    )?;

    let mut config = helpers::test_config();
    config.plugins = Some(toml::from_str(&format!(
        r#"
        plugin_dirs = [{:?}]
        [[plugins]]
        name = "probe"
        config = {{ greeting = "hi" }}
        permissions = ["edit"]
        "#,
        plugins.path()
    ))?);

    let mut file = helpers::temp_file_with_contents("abc\n")?;
    let other = helpers::temp_file_with_contents("other\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    assert!(app.plugins_enabled());
    assert_eq!(recorded(&app), ["ready", "greeting=hi"]);

    // A macro run 20 times edits the buffer 20 times before the next frame: one event.
    send_keys(&mut app, "Qix<esc>Q").await?;
    let changes = |app: &Application| {
        recorded(app)
            .iter()
            .filter(|event| *event == "buffer_changed")
            .count()
    };
    let before = changes(&app);
    assert!(before >= 1);
    send_keys(&mut app, "20q").await?;
    // (the replay ends with a redraw request: draw a frame for sure)
    send_keys(&mut app, "<esc>").await?;
    assert_eq!(changes(&app), before + 1);

    send_keys(&mut app, ":w<ret>").await?;
    helpers::assert_file_has_content(
        &mut file,
        &LineFeedHandling::Native.apply(&format!("PRE {}abc\n", "x".repeat(21))),
    )?;
    let events = recorded(&app);
    let pre = events.iter().position(|e| e == "buffer_pre_save");
    let post = events.iter().position(|e| e == "buffer_post_save");
    assert!(pre.is_some() && pre < post, "{events:?}");
    // The pre-save edit is part of the saved revision: nothing is left unsaved.
    assert!(!helix_view::doc!(app.editor).is_modified());
    assert!(app.editor.documents().all(|doc| !doc.is_modified()));

    send_keys(
        &mut app,
        &format!(":o {}<ret>:bc<ret>", other.path().to_string_lossy()),
    )
    .await?;
    assert_eq!(
        recorded(&app).last().map(String::as_str),
        Some("buffer_close")
    );
    assert!(!app.editor.should_close());

    // `:wq` writes (running the hook again) and quits: the hook's edit doesn't block it.
    send_keys(&mut app, ":wq<ret>").await?;
    assert!(app.editor.should_close(), "{:?}", app.editor.get_status());
    helpers::assert_file_has_content(
        &mut file,
        &LineFeedHandling::Native.apply(&format!("PRE PRE {}abc\n", "x".repeat(21))),
    )?;
    Ok(())
}

/// A directory of plugins: `(name, init.lua, plugin.toml)`.
fn plugin_dir(plugins: &[(&str, &str, Option<&str>)]) -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    for (name, init, manifest) in plugins {
        let plugin = dir.path().join(name);
        std::fs::create_dir(&plugin)?;
        std::fs::write(plugin.join("init.lua"), init)?;
        if let Some(manifest) = manifest {
            std::fs::write(plugin.join("plugin.toml"), manifest)?;
        }
    }
    Ok(dir)
}

/// The test config with the plugins of `dir` and the `[[plugins.plugins]]` entries
/// `entries` (TOML).
fn plugin_config(dir: &std::path::Path, entries: &str) -> anyhow::Result<Config> {
    let mut config = helpers::test_config();
    config.editor.notifications.enable = false;
    config.plugins = Some(toml::from_str(&format!(
        "plugin_dirs = [{:?}]\n{entries}",
        dir
    ))?);
    Ok(config)
}

/// What a register holds, split at `|`.
fn register(app: &Application, name: char) -> Vec<String> {
    app.editor
        .registers
        .read(name, &app.editor)
        .and_then(|mut values| values.next().map(|value| value.into_owned()))
        .map(|text| text.split('|').map(String::from).collect())
        .unwrap_or_default()
}

/// Tries each call and reports `name=ok` or `name=<the permissions it lacks>`.
const TRY_ALL: &str = r#"
local function try(results, name, f)
    local ok, err = pcall(f)
    if ok then
        results[#results + 1] = name .. "=ok"
    else
        local missing = tostring(err):match("needs the (.-) permission")
        results[#results + 1] = name .. "=" .. (missing or tostring(err))
    end
end
local function checks()
    local results = {}
    try(results, "read", function() return helix.buffer.get_current():get_text() end)
    try(results, "insert", function() helix.buffer.get_current():insert(1, 0, "X") end)
    try(results, "save", function() helix.editor.save() end)
    try(results, "sh", function() helix.editor.execute_command("sh", { "true" }) end)
    try(results, "set", function() helix.editor.execute_command("set", { "scrolloff", "2" }) end)
    try(results, "vsplit", function() helix.editor.execute_command("vsplit") end)
    try(results, "open", function() helix.editor.open("x.txt") end)
    try(results, "command", function()
        helix.register_command{ name = "c" .. #results, handler = function() end }
    end)
    return table.concat(results, "|")
end
"#;

/// A plugin gets `read` and `ui`, and what its entry grants; every call checks.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_may_only_do_what_they_are_granted() -> anyhow::Result<()> {
    let limited = format!(
        r#"{TRY_ALL}
        helix.on("ready", function() helix.ui.set_status(checks()) end)"#
    );
    let granted = format!(
        r#"{TRY_ALL}
        helix.on("ready", function() helix.editor.set_register("y", {{ checks() }}) end)"#
    );
    let dir = plugin_dir(&[("limited", &limited, None), ("granted", &granted, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "granted"
        permissions = ["edit", "commands", "windows"]
        "#,
    )?;
    let file = helpers::temp_file_with_contents("abc\n")?;
    let app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let status: String = app
        .editor
        .get_status()
        .map(|(status, _)| status.to_string())
        .unwrap_or_default();
    let limited: Vec<&str> = status.split('|').collect();
    assert_eq!(
        limited,
        [
            "read=ok",
            "insert='edit'",
            "save='files'",
            "sh='commands' and 'shell'",
            "set='commands' and 'config'",
            "vsplit='commands' and 'windows'",
            "open='windows'",
            "command='commands'",
        ]
    );

    let granted: Vec<String> = register(&app, 'y');
    assert_eq!(
        granted,
        [
            "read=ok",
            "insert=ok",
            "save='files'",
            "sh='shell'",
            "set='config'",
            "vsplit=ok",
            "open=ok",
            "command=ok",
        ]
    );
    // the granted plugin's edit happened, the limited plugin's did not
    let text: String = helix_view::doc!(app.editor).text().to_string();
    assert_eq!(text.matches('X').count(), 1, "{text:?}");
    Ok(())
}

/// A manifest that lists permissions narrows the grants, and what it asks for that is not
/// granted is reported when the plugin loads.
#[tokio::test(flavor = "multi_thread")]
async fn a_manifest_narrows_and_ungranted_requests_are_reported() -> anyhow::Result<()> {
    let init = r#"
        helix.on("ready", function()
            helix.editor.set_register("y", { table.concat(helix.permissions(), ",") })
        end)
    "#;
    let manifest = r#"
        name = "narrow"
        version = "1.0.0"
        permissions = ["read", "edit", "files"]
    "#;
    let dir = plugin_dir(&[("narrow", init, Some(manifest))])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "narrow"
        permissions = ["edit", "shell"]
        "#,
    )?;
    let app = helpers::AppBuilder::new().with_config(config).build()?;

    // `ui` (a default) and `shell` (granted) are not asked for; `files` is not granted
    let permissions: Vec<String> = register(&app, 'y');
    assert_eq!(permissions, ["read,edit"]);
    let status: Option<String> = app.editor.get_status().map(|(s, _)| s.to_string());
    assert_eq!(
        status.as_deref(),
        Some("Plugin 'narrow' asks for permissions not granted: files (see :log-open)")
    );
    Ok(())
}

/// An unknown permission in `config.toml` is a config error.
#[test]
fn unknown_granted_permissions_are_config_errors() {
    let error = toml::from_str::<helix_plugin::PluginConfig>(
        r#"
        [[plugins]]
        name = "p"
        permissions = ["edit", "network"]
        "#,
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(
        error.contains("unknown plugin permission 'network'"),
        "{error}"
    );
}

/// Sends terminal events (not keys) and runs the event loop until it is idle.
async fn send_events(app: &mut Application, events: Vec<termina::Event>) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for event in events {
        tx.send(Ok(event))?;
    }
    let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.editor.reset_idle_timer();
    app.event_loop_until_idle(&mut rx).await;
    Ok(())
}

/// The events that had no hook for plugins before: each reaches the handlers, with its
/// fields (and their types).
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn plugins_hear_of_every_kind_of_event() -> anyhow::Result<()> {
    let init = r#"
        local seen = {}
        local function record(entry)
            seen[#seen + 1] = entry
            helix.editor.set_register("z", { table.concat(seen, "|") })
        end
        local function base(path)
            return path and path:match("[^/\\]+$") or "nil"
        end
        helix.on("window_open", function(e)
            record("window_open:" .. type(e.window_id) .. ":" .. type(e.document_id))
        end)
        helix.on("window_close", function(e) record("window_close:" .. type(e.window_id)) end)
        helix.on("window_focus", function(e)
            record("window_focus:" .. tostring(e.window_id ~= e.previous_window_id))
        end)
        helix.on("insert_char", function(e) record("char:" .. e.char) end)
        helix.on("command", function(e)
            if e.name == "insert_mode" or e.name == "normal_mode" then
                record("command:" .. e.name)
            end
        end)
        helix.on("language_change", function(e)
            record("language:" .. tostring(e.old_language) .. ">" .. tostring(e.language))
        end)
        helix.on("buffer_rename", function(e)
            record("rename:" .. base(e.old_path) .. ">" .. base(e.path))
        end)
        helix.on("buffer_save_failed", function(e)
            record("save_failed:" .. type(e.document_id) .. ":" .. base(e.path))
        end)
        helix.on("config_change", function() record("config_change") end)
        helix.on("theme_change", function(e) record("theme:" .. e.theme) end)
        helix.on("scroll", function(e) record("scroll:" .. type(e.top_char)) end)
        helix.on("paste", function(e) record("paste:" .. e.text) end)
        helix.on("focus_lost", function() record("focus_lost") end)
        helix.on("focus_gained", function() record("focus_gained") end)
        helix.on("mouse", function(e)
            record("mouse:" .. e.kind .. ":" .. tostring(e.button) .. ":" .. e.row .. ":"
                .. e.col .. ":" .. e.modifiers)
        end)
        helix.on("idle", function() idle_count = (idle_count or 0) + 1 end)
        helix.on("buffer_focus_lost", function(e) record("focus_lost:" .. base(e.path)) end)
    "#;
    let dir = plugin_dir(&[("events", init, None)])?;
    let mut config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "events"
        permissions = ["edit"]
        "#,
    )?;
    config.editor.true_color = true;
    let lines: String = (0..300).map(|n| format!("line {n}\n")).collect();
    let file = helpers::temp_file_with_contents(&lines)?;
    let other = helpers::temp_file_with_contents("other\n")?;
    let renamed = tempfile::tempdir()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    // The events recorded since the last call.
    let mut seen = 0;
    let mut since = |app: &Application| -> Vec<String> {
        let all = register(app, 'z');
        let new = all[seen.min(all.len())..].to_vec();
        seen = all.len();
        new
    };

    // (the view the split leaves loses the focus: of its buffer too)
    send_keys(&mut app, ":vsplit<ret>").await?;
    let name = file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let events: Vec<String> = since(&app);
    assert_eq!(
        events,
        [
            format!("focus_lost:{name}"),
            "window_open:string:string".to_string(),
            "window_focus:true".to_string(),
        ]
    );

    send_keys(&mut app, ":q<ret>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["window_close:string", "window_focus:true"]);

    send_keys(&mut app, "ihi<esc>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(
        events,
        [
            "command:insert_mode",
            "char:h",
            "char:i",
            "command:normal_mode"
        ]
    );

    send_keys(&mut app, ":lang rust<ret>").await?;
    let events: Vec<String> = since(&app);
    // (a file without a language: nil)
    assert_eq!(events, ["language:nil>rust"]);

    let new_path = renamed.path().join("renamed.rs");
    send_keys(&mut app, &format!(":w {}<ret>", new_path.display())).await?;
    let saved = std::fs::read_to_string(&new_path).unwrap_or_default();
    assert!(saved.starts_with("hiline 0"), "{saved:?}");
    let events: Vec<String> = since(&app);
    assert_eq!(events, [format!("rename:{name}>renamed.rs")]);

    let missing = renamed.path().join("no-such-dir").join("x.rs");
    send_keys(&mut app, &format!(":w {}<ret>", missing.display())).await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["save_failed:string:x.rs"]);

    send_keys(&mut app, ":set scrolloff 2<ret>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["config_change"]);

    // (the tests have the built-in themes only; true color is on: `default` at start)
    send_keys(&mut app, ":theme base16_default<ret>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["theme:base16_default"]);

    send_keys(&mut app, "<C-d>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["scroll:number"]);

    send_keys(&mut app, &format!(":open {}<ret>", other.path().display())).await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["focus_lost:renamed.rs"]);

    use termina::event::{Modifiers as KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    send_events(
        &mut app,
        vec![
            termina::Event::Paste("pasted".to_string()),
            termina::Event::FocusOut,
            termina::Event::FocusIn,
            termina::Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 3,
                row: 1,
                modifiers: KeyModifiers::CONTROL,
            }),
            termina::Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 4,
                row: 1,
                modifiers: KeyModifiers::NONE,
            }),
        ],
    )
    .await?;
    let terminal: Vec<String> = since(&app);
    assert_eq!(
        terminal,
        [
            "paste:pasted",
            "focus_lost",
            "focus_gained",
            // a move without a button is no event
            "mouse:down:left:1:3:C",
        ]
    );
    Ok(())
}

/// `idle` (like Vim's `CursorHold`) and `exit` run right away, not before a frame.
#[tokio::test(flavor = "multi_thread")]
async fn idle_and_exit_reach_the_plugins() -> anyhow::Result<()> {
    let init = r#"
        local seen = {}
        local function record(entry)
            seen[#seen + 1] = entry
            helix.editor.set_register("z", { table.concat(seen, "|") })
        end
        local idle = 0
        helix.on("idle", function()
            idle = idle + 1
            if idle == 1 then record("idle:" .. helix.editor.mode()) end
        end)
        helix.on("exit", function() record("exit") end)
    "#;
    let dir = plugin_dir(&[("hold", init, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "hold"
        permissions = ["edit"]
        "#,
    )?;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    let before: Vec<String> = register(&app, 'z');
    assert_eq!(before, Vec::<String>::new());

    send_keys(&mut app, "i").await?;
    let idle: Vec<String> = register(&app, 'z');
    assert_eq!(idle, ["idle:insert"]);

    let errors = app.close().await;
    assert!(errors.is_empty(), "{errors:?}");
    let exit: Vec<String> = register(&app, 'z');
    assert_eq!(exit, ["idle:insert", "exit"]);
    Ok(())
}

/// Plugins bind keys to commands and Lua functions; a binding needs what its command needs,
/// and removing it brings the configured one back.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_bind_keys() -> anyhow::Result<()> {
    let init = r#"
        local function try(f)
            local ok, err = pcall(f)
            return ok and "ok" or (tostring(err):match("needs the (.-) permission")
                or tostring(err):match("No command '[^']*'") or tostring(err))
        end
        helix.keymap.set("normal", "<space>z", function()
            helix.editor.set_register("z", { "zapped " .. helix.editor.mode() })
        end, { desc = "zap" })
        -- `X` moves the cursor down (the default `X` selects the line)
        helix.keymap.set("normal", "X", "move_line_down")
        helix.on("ready", function()
            helix.editor.set_register("y", {
                try(function() helix.keymap.set("normal", "Q", ":sh echo hi") end) .. "," ..
                try(function() helix.keymap.set("normal", "Q", "@ihi") end) .. "," ..
                try(function() helix.keymap.set("normal", "Q", "nope") end) .. "," ..
                try(function() helix.keymap.set("normal", "Q", ":write") end)
            })
        end)
        helix.register_command{ name = "restore-x", handler = function()
            helix.keymap.del("normal", "X")
        end }
    "#;
    let dir = plugin_dir(&[("keys", init, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "keys"
        permissions = ["edit", "keymaps", "commands"]
        "#,
    )?;
    let file = helpers::temp_file_with_contents("one\ntwo\nthree\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    // `:sh` needs `shell`, a macro all of them (it has edit, commands and keymaps), `:write`
    // `files`; unknown commands fail
    let refused: Vec<String> = register(&app, 'y');
    assert_eq!(
        refused,
        [concat!(
            "'shell',",
            "'windows' and 'files' and 'config' and 'explorer' and 'shell',",
            "No command 'nope',",
            "'files'"
        )]
    );

    send_keys(&mut app, "<space>z").await?;
    let zapped: Vec<String> = register(&app, 'z');
    assert_eq!(zapped, ["zapped normal"]);

    send_keys(&mut app, "X").await?;
    let cursor_line = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let text = doc.text().slice(..);
        text.char_to_line(doc.selection(view.id).primary().cursor(text))
    };
    let moved: usize = cursor_line(&app);
    assert_eq!(moved, 1);

    // removed: `X` selects the line again
    send_keys(&mut app, ":restore-x<ret>X").await?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let selected: String = doc
        .selection(view.id)
        .primary()
        .fragment(doc.text().slice(..))
        .into_owned();
    assert_eq!(selected, "two\n");
    Ok(())
}

/// `helix.editor.run` runs the commands of key bindings, pickers included; settings, the
/// working directory and buffers that are not focused can be changed.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_run_commands_and_change_settings_and_buffers() -> anyhow::Result<()> {
    let init = r#"
        helix.register_command{ name = "probe", handler = function()
            helix.editor.run("goto_line_end")
            helix.editor.run("move_line_down", 2)
            local ok, err = pcall(helix.editor.run, "find_next_char")
            local ok2, err2 = pcall(helix.editor.run, "nope")
            helix.editor.set_option("text-width", 60)
            helix.editor.set_option("auto-pairs", false)
            helix.editor.run("file_picker")
            helix.editor.set_register("z", {
                tostring(err):match("waits for a key") .. "," .. tostring(err2):match("No command 'nope'")
            })
        end }
        helix.register_command{ name = "other-buffers", handler = function(args)
            local other = helix.editor.open(args[1])
            other:insert(1, 0, "edited ")
            other:save()
            helix.editor.set_register("y", { tostring(other:is_modified()) })
        end }
    "#;
    let dir = plugin_dir(&[("runner", init, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "runner"
        permissions = ["edit", "commands", "config", "files", "windows", "ui"]
        "#,
    )?;
    let file = helpers::temp_file_with_contents("one\ntwo\nthree\nfour\n")?;
    let other = helpers::temp_file_with_contents("other\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    send_keys(&mut app, ":probe<ret>").await?;
    let errors: Vec<String> = register(&app, 'z');
    assert_eq!(errors, ["waits for a key,No command 'nope'"]);
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let (line, col): (usize, usize) = (
        text.char_to_line(cursor),
        cursor - text.line_to_char(text.char_to_line(cursor)),
    );
    assert_eq!((line, col), (2, 2));
    let width: usize = app.editor.config().text_width;
    assert_eq!(width, 60);
    let auto_pairs: bool = app.editor.auto_pairs.is_some();
    assert!(!auto_pairs);
    // the picker the command opened is on screen
    let picker_open: bool = app.compositor.has_id(helix_term::ui::picker::ID);
    assert!(picker_open);

    send_keys(&mut app, "<esc>").await?;
    send_keys(
        &mut app,
        &format!(":other-buffers {}<ret>", other.path().display()),
    )
    .await?;
    let saved = std::fs::read_to_string(other.path())?;
    assert_eq!(saved, LineFeedHandling::Native.apply("edited other\n"));
    // the edit was in the background: the focused buffer is unchanged
    let text: String = helix_view::doc!(app.editor).text().to_string();
    assert_eq!(text, "one\ntwo\nthree\nfour\n");
    Ok(())
}

/// A plugin's picker has columns and its own items, gets items added while it is open, and
/// hands the item chosen back with the action; a handle closes it.
#[tokio::test(flavor = "multi_thread")]
async fn plugin_pickers_show_columns_take_items_and_close() -> anyhow::Result<()> {
    let init = r#"
        local function open()
            return helix.ui.picker{
                title = "Tasks",
                columns = { "Name", "Step" },
                items = {
                    { columns = { "build", "one" }, data = { id = 7 } },
                    { columns = { "test", "two" }, data = { id = 8 } },
                },
                on_select = function(item, action)
                    helix.editor.set_register("z", { item.data.id .. "/" .. action })
                end,
            }
        end
        helix.register_command{ name = "tasks", handler = function()
            local handle = open()
            handle:add({ { columns = { "deploy", "three" }, data = { id = 9 } } })
        end }
        helix.register_command{ name = "tasks-closed", handler = function()
            open():close()
        end }
    "#;
    let dir = plugin_dir(&[("tasks", init, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "tasks"
        permissions = ["edit", "commands"]
        "#,
    )?;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;

    send_keys(&mut app, ":tasks<ret>").await?;
    let open: bool = app.compositor.has_id(helix_term::ui::picker::ID);
    assert!(open);
    let rows = app.rendered_rows();
    let shown = |text: &str| rows.iter().any(|row| row.contains(text));
    assert!(
        shown("Tasks") && shown("Name") && shown("Step"),
        "{rows:#?}"
    );
    assert!(
        shown("build") && shown("test") && shown("deploy"),
        "{rows:#?}"
    );

    // the added item is chosen like the others
    send_keys(&mut app, "deploy<C-v>").await?;
    let chosen: Vec<String> = register(&app, 'z');
    assert_eq!(chosen, ["9/vsplit"]);
    assert!(!app.compositor.has_id(helix_term::ui::picker::ID));

    send_keys(&mut app, ":tasks-closed<ret>").await?;
    let closed: bool = !app.compositor.has_id(helix_term::ui::picker::ID);
    assert!(closed);
    Ok(())
}

/// A plugin fills the quicklist and shows it in the quickfix panel; a picker can be docked
/// there too.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_fill_and_show_the_quickfix_panel() -> anyhow::Result<()> {
    let a = helpers::temp_file_with_contents("alpha\nbeta\n")?;
    let b = helpers::temp_file_with_contents("gamma\n")?;
    let init = format!(
        r#"
        helix.register_command{{ name = "qf", handler = function()
            helix.quickfix.set({{
                {{ path = {a:?}, line = 2, text = "beta line" }},
                {{ path = {b:?}, line = 1, text = "gamma line" }},
            }}, true)
            local entries = helix.quickfix.get()
            helix.editor.set_register("z", {{ #entries .. ":" .. entries[1].line }})
        end }}
        helix.register_command{{ name = "docked", handler = function()
            helix.ui.picker{{ title = "Docked list", dock = true,
                items = {{ {{ text = "in the panel", path = {a:?}, line = 1 }} }},
                on_select = function() end }}
        end }}
        "#,
        a = a.path().to_string_lossy(),
        b = b.path().to_string_lossy(),
    );
    let dir = plugin_dir(&[("qf", &init, None)])?;
    let config = plugin_config(
        dir.path(),
        r#"
        [[plugins]]
        name = "qf"
        permissions = ["edit", "commands"]
        "#,
    )?;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;

    send_keys(&mut app, ":qf<ret>").await?;
    let read: Vec<String> = register(&app, 'z');
    assert_eq!(read, ["2:2"]);
    let (visible, panel): (bool, bool) = (
        app.editor.quickfix_panel.visible,
        app.compositor.has_id(helix_term::ui::quickfix::ID),
    );
    assert_eq!((visible, panel), (true, true));
    let rows = app.rendered_rows();
    assert!(
        rows.iter().any(|row| row.contains("beta line"))
            && rows.iter().any(|row| row.contains("gamma line")),
        "{rows:#?}"
    );

    // the panel has the keys: close it, then dock a picker in it
    send_keys(&mut app, "q:docked<ret>").await?;
    let rows = app.rendered_rows();
    assert!(
        rows.iter().any(|row| row.contains("Docked list"))
            && rows.iter().any(|row| row.contains("in the panel")),
        "{rows:#?}"
    );
    assert!(app.editor.quickfix_panel.visible);
    Ok(())
}

/// Plugins see the explorer's root and cursor, hear of its moves and of the files it creates
/// and renames, ask it to reveal a path, and put text on its rows.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_follow_drive_and_decorate_the_explorer() -> anyhow::Result<()> {
    use helix_term::ui::explorer::{Explorer, EXPLORER_ID};
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    std::fs::create_dir(root.join("sub"))?;
    std::fs::write(root.join("a.txt"), "a\n")?;
    std::fs::write(root.join("sub/deep.txt"), "d\n")?;
    let init = format!(
        r##"
        local seen = {{}}
        local function record(entry)
            seen[#seen + 1] = entry
            helix.editor.set_register("z", {{ table.concat(seen, "|") }})
        end
        local function base(path) return path:match("[^/\\]+$") end
        helix.on("explorer_select", function(e) record("select:" .. base(e.path)) end)
        helix.on("file_create", function(e)
            record("create:" .. base(e.path) .. ":" .. tostring(e.is_dir))
        end)
        helix.on("file_rename", function(e)
            record("rename:" .. base(e.old_path) .. ">" .. base(e.path))
        end)
        helix.register_command{{ name = "look", handler = function()
            record("root:" .. tostring(helix.explorer.get_root() == {root:?})
                .. ":" .. base(helix.explorer.get_selected()))
            helix.explorer.decorate({root:?} .. "/a.txt", {{ text = "TODO 3", fg = "#ff0000" }})
            helix.explorer.reveal({root:?} .. "/sub/deep.txt")
        end }}
        "##,
        root = root.to_string_lossy()
    );
    let plugins = plugin_dir(&[("tree", &init, None)])?;
    let config = plugin_config(
        plugins.path(),
        r#"
        [[plugins]]
        name = "tree"
        permissions = ["edit", "commands", "explorer"]
        "#,
    )?;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    app.editor.explorer_panel.visible = true;
    app.editor.explorer_panel.focused = true;
    let explorer = Explorer::new(&root, &app.editor)?;
    app.compositor.push(Box::new(explorer));
    if let Some(explorer) = app.compositor.find_id::<Explorer>(EXPLORER_ID) {
        explorer.wait_for_scan(&app.editor);
    }
    let mut seen = 0;
    let mut since = |app: &Application| -> Vec<String> {
        let all = register(app, 'z');
        let new = all[seen.min(all.len())..].to_vec();
        seen = all.len();
        new
    };

    // (the tree: the root, sub/, a.txt; shown, the cursor is on the root)
    send_keys(&mut app, "j").await?;
    let root_name = root.file_name().unwrap().to_string_lossy().into_owned();
    let events: Vec<String> = since(&app);
    assert_eq!(
        events,
        [format!("select:{root_name}"), "select:sub".to_string()]
    );

    send_keys(&mut app, "<C-l>:look<ret>").await?;
    let events: Vec<String> = since(&app);
    assert_eq!(events, ["root:true:sub", "select:deep.txt"]);
    let rows = app.rendered_rows();
    assert!(
        rows.iter()
            .any(|row| row.contains("a.txt") && row.contains("TODO 3")),
        "{rows:#?}"
    );

    // back in the tree: create a file, then rename it
    send_keys(&mut app, "<C-h>anew.txt<ret>").await?;
    assert!(root.join("sub/new.txt").exists());
    send_keys(&mut app, "r<C-u>renamed.txt<ret>").await?;
    assert!(root.join("sub/renamed.txt").exists());
    let events: Vec<String> = since(&app)
        .into_iter()
        .filter(|e| !e.starts_with("select:"))
        .collect();
    assert_eq!(
        events,
        ["create:new.txt:false", "rename:new.txt>renamed.txt"]
    );
    Ok(())
}
