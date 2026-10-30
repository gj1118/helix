use super::*;

use std::path::{Path, PathBuf};

use helix_term::application::Application;

const MOCK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/test/fixtures/mock_dap.py"
);

fn lang_conf(log: &Path) -> String {
    let log = log.display();
    format!(
        r##"
        [[language]]
        name = "mock"
        scope = "source.mock"
        file-types = ["mock"]
        comment-token = "#"

        [language.debugger]
        name = "mockdap"
        transport = "stdio"
        command = "python3"
        args = ["{MOCK}", "{log}"]

        [[language.debugger.templates]]
        name = "run"
        request = "launch"
        completion = [ {{ name = "program", completion = "filename" }} ]
        args = {{ program = "{{0}}", stops = [3, 5], label = "run" }}

        [[language.debugger.templates]]
        name = "second"
        request = "launch"
        completion = [ {{ name = "program", completion = "filename" }} ]
        args = {{ program = "{{0}}", stops = [4], label = "second" }}
        "##
    )
}

/// A buffer of the mock language with five lines and an adapter log.
struct Fixture {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    pub log: PathBuf,
}

fn fixture() -> anyhow::Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("prog.mock");
    std::fs::write(
        &path,
        "answer = 42\nname = \"mock\"\nobj = point(1, 2)\nitems = range(250)\nprint(answer, name)\n",
    )?;
    let log = dir.path().join("dap.log");
    Ok(Fixture {
        _dir: dir,
        path,
        log,
    })
}

fn app(fixture: &Fixture) -> anyhow::Result<Application> {
    helpers::AppBuilder::new()
        .with_file(&fixture.path, None)
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf(&fixture.log))))
        .build()
}

fn requests(fixture: &Fixture, command: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(&fixture.log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["command"] == command)
        .map(|entry| entry["arguments"].clone())
        .collect()
}

fn cursor_line(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.cursor_line(view.id)
}

fn start(path: &Path) -> String {
    format!(":debug-start run {}<ret>", path.display())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_starts_and_stops_at_the_first_stop() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    // idle rounds so the adapter can answer
    test_key_sequences(
        &mut app,
        vec![
            (Some(start(&fixture.path).as_str()), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(app.editor.debug_adapters.get_active_client().is_some());
                    // the mock stops at (one-based) line 3
                    assert_eq!(cursor_line(app), 2);
                }),
            ),
        ],
        false,
    )
    .await
}

fn rows(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_variable_browser_opens_levels_and_goes_back_with_the_filter_kept() -> anyhow::Result<()>
{
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<left>"), None),
            (Some("<left>"), None),
            (Some("<space>Gv"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(
                        rows.contains("▸ Locals") && rows.contains("▸ Globals"),
                        "{rows}"
                    );
                    // the root has no way back
                    assert!(!rows.contains(".. (back)"), "{rows}");
                }),
            ),
            // open Locals
            (Some("<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(rows.contains("▸ obj") && rows.contains(" int"), "{rows}");
                    assert!(rows.contains("▸ obj") && rows.contains("▸ items"), "{rows}");
                    assert!(rows.contains(".. (back)"), "{rows}");
                }),
            ),
            // filter, open `obj`
            (Some("obj<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(rows.contains(" x ") && rows.contains(" y "), "{rows}");
                    assert!(!rows.contains("▸ items"), "{rows}");
                }),
            ),
            // `.. (back)` is the first row; the filter of the parent level is restored
            (Some("<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(rows.contains("▸ obj"), "{rows}");
                    assert!(!rows.contains("▸ items"), "filter was lost:\n{rows}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn large_arrays_are_paged_with_load_more() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<left>"), None),
            (Some("<left>"), None),
            (Some("<space>Gv"), None),
            (Some("<left>"), None),
            (Some("<ret>"), None),
            (Some("<left>"), None),
            // open `items`
            (Some("items<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(rows.contains("[0]"), "{rows}");
                }),
            ),
            // select "Load more…" by filtering for it
            (Some("Load<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    // 100 of 250 were loaded, now 200: 50 are left
                    assert!(
                        rows.contains("Load more…") && rows.contains("50 more"),
                        "{rows}"
                    );
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await?;

    let pages: Vec<(Option<u64>, Option<u64>)> = requests(&fixture, "variables")
        .iter()
        .filter(|args| args["variablesReference"] == 4)
        .map(|args| (args["start"].as_u64(), args["count"].as_u64()))
        .collect();
    assert_eq!(pages, [(Some(0), Some(100)), (Some(100), Some(100))]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_eval_reports_and_opens_expandable_results() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<left>"), None),
            (Some("<left>"), None),
            (Some(":debug-eval answer<ret>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.contains("answer = 42"), "{status}");
                }),
            ),
            // a result with children opens the browser
            (Some(":debug-eval obj<ret>"), None),
            (Some("<left>"), None),
            (Some("<left>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(rows.contains(" x ") && rows.contains(" y "), "{rows}");
                }),
            ),
            (Some("<esc>"), None),
            // errors are reported
            (
                Some(":debug-eval boom<ret>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(
                        rows.contains("cannot evaluate 'boom'"),
                        "{:?}\n{rows}",
                        app.editor.get_status()
                    );
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn deep_stacks_are_paged_in_the_frame_picker() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<esc>"), None),
            (Some("<esc>"), None),
            (Some("<space>Gsf"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    assert!(
                        rows.contains("frame0") && rows.contains("frame19"),
                        "{rows}"
                    );
                    // 20 frames and the "Load more…" row
                    assert!(rows.contains("21/21"), "{rows}");
                }),
            ),
            (Some("Load<ret>"), None),
            (
                Some("<left>"),
                Some(&|app| {
                    let rows = rows(app);
                    // 40 of 120 frames and the "Load more…" row
                    assert!(rows.contains("41/41"), "{rows}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await?;

    let pages: Vec<(Option<u64>, Option<u64>)> = requests(&fixture, "stackTrace")
        .iter()
        .map(|args| (args["startFrame"].as_u64(), args["levels"].as_u64()))
        .collect();
    assert!(pages.contains(&(Some(0), Some(20))), "{pages:?}");
    assert!(pages.contains(&(Some(20), Some(20))), "{pages:?}");
    Ok(())
}

fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle:?}:\n{}", rows.join("\n")))
}

#[tokio::test(flavor = "multi_thread")]
async fn variable_values_are_shown_at_the_end_of_the_lines_and_hidden_when_modified(
) -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let line = row_with(&rows, "answer = 42");
                    assert!(line.contains("answer = 42  answer = 42"), "{line}");
                    let line = row_with(&rows, "obj = point(1, 2)");
                    assert!(line.contains("point(1, 2)  obj = "), "{line}");
                    // names the line doesn't use are not shown
                    assert!(!line.contains("answer ="), "{line}");
                    // lines after the stop (line 3) have no values yet
                    let line = row_with(&rows, "print(answer, name)");
                    assert!(!line.contains("= 42"), "{line}");
                }),
            ),
            // an unsaved change hides the values: they belong to the saved file
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let line = row_with(&rows, "answer = 42");
                    assert!(!line.contains("answer = 42  answer = 42"), "{line}");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_values_can_be_turned_off() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut config = helpers::test_config();
    config.editor.debugger.inline_values = false;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(&fixture.path, None)
        .with_lang_loader(helpers::test_syntax_loader(Some(lang_conf(&fixture.log))))
        .build()?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert_eq!(cursor_line(app), 2);
                    let rows = app.rendered_rows();
                    let line = row_with(&rows, "answer = 42");
                    assert!(!line.contains("answer = 42  answer = 42"), "{line}");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn several_sessions_can_run_and_be_switched() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let first = start(&fixture.path);
    let second = format!(":debug-start second {}<ret>", fixture.path.display());
    test_key_sequences(
        &mut app,
        vec![
            (Some(first.as_str()), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert_eq!(app.editor.debug_adapters.len(), 1);
                    assert_eq!(cursor_line(app), 2);
                }),
            ),
            (Some(second.as_str()), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    // both sessions exist; the newest one is selected and shows its stop (line 4)
                    assert_eq!(app.editor.debug_adapters.len(), 2);
                    let selected = app.editor.debug_adapters.get_active_client().unwrap();
                    assert_eq!(selected.display_name(), "second");
                    assert_eq!(selected.state_label(), "stopped");
                    assert_eq!(cursor_line(app), 3);
                    let rows = app.rendered_rows();
                    let status = row_with(&rows, "dbg ");
                    assert!(
                        status.contains("dbg second stopped (2 sessions)"),
                        "{status}"
                    );
                }),
            ),
            // the picker marks the selected session; choose the first one
            (
                Some("<space>Gss"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("Debug Sessions"), "{rows}");
                    assert!(
                        rows.contains("* second") || rows.contains("*  second"),
                        "{rows}"
                    );
                }),
            ),
            (Some("run<ret>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    let selected = app.editor.debug_adapters.get_active_client().unwrap();
                    assert_eq!(selected.display_name(), "run");
                    assert_eq!(cursor_line(app), 2);
                }),
            ),
            // ending the selected session selects the other one
            (Some("<space>Gt"), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert_eq!(app.editor.debug_adapters.len(), 1);
                    let selected = app.editor.debug_adapters.get_active_client().unwrap();
                    assert_eq!(selected.display_name(), "second");
                    assert_eq!(cursor_line(app), 3);
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn sources_generated_by_the_adapter_open_read_only_and_take_breakpoints() -> anyhow::Result<()>
{
    let fixture = fixture()?;
    let mut app = app(&fixture)?;
    let start_keys = start(&fixture.path);
    test_key_sequences(
        &mut app,
        vec![
            (Some(start_keys.as_str()), None),
            (Some("<esc>"), None),
            (Some("<esc>"), None),
            // frame 2 of the mock lives in a source only the adapter has
            (Some("<space>Gsf"), None),
            (Some("frame2<ret>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("generated line 1"), "{rows}");
                    assert!(rows.contains("generated.txt"), "{rows}");
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.readonly);
                    assert!(!doc.is_modified());
                    assert_eq!(cursor_line(app), 1);
                }),
            ),
            // a breakpoint is set by the source's reference, not by a path
            (Some("<space>Gb"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    let path = doc.path().unwrap().to_path_buf();
                    assert_eq!(app.editor.breakpoints.get(&path).map(Vec::len), Some(1));
                }),
            ),
        ],
        false,
    )
    .await?;

    let sources = requests(&fixture, "source");
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0]["sourceReference"], 7);
    let with_reference: Vec<_> = requests(&fixture, "setBreakpoints")
        .into_iter()
        .filter(|args| args["source"]["sourceReference"] == 7)
        .collect();
    assert_eq!(with_reference.len(), 1, "{with_reference:?}");
    assert!(with_reference[0]["source"].get("path").is_none());
    Ok(())
}
