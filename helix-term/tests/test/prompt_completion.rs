//! Deferred prompt completions (helix-editor/helix#16065): completers doing blocking I/O run on
//! a background thread, so the prompt stays responsive while they work.

use super::*;

use helix_term::application::Application;
use helix_term::ui::{self, completers::CompletionResult, Prompt, PromptEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

type Keys = tokio::sync::mpsc::UnboundedSender<std::io::Result<Event>>;
type KeyStream = UnboundedReceiverStream<std::io::Result<Event>>;

/// A deferred completer taking `delay` per run, completing `input` to `done:<input>`, and
/// counting its runs in `runs`.
fn slow_completer(
    delay: Duration,
    runs: Arc<AtomicUsize>,
) -> impl FnMut(&helix_view::Editor, &str) -> CompletionResult + 'static {
    move |_editor, input| {
        let input = input.to_owned();
        let runs = runs.clone();
        CompletionResult::Deferred(Box::new(move |_handle| {
            runs.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            vec![(0.., format!("done:{input}").into())]
        }))
    }
}

fn ignore(_: &mut helix_term::compositor::Context, _: &str, _: PromptEvent) {}

fn texts(completions: &[ui::prompt::Completion]) -> Vec<String> {
    completions
        .iter()
        .map(|(_, span)| span.content.to_string())
        .collect()
}

/// The completions of the prompt on top: a bare one or the one in a cmdline popup.
fn completions(app: &mut Application) -> Vec<String> {
    if let Some(prompt) = app.compositor.find::<Prompt>() {
        return texts(prompt.completions());
    }
    let popup = app
        .compositor
        .find::<ui::CmdlinePopup>()
        .expect("a prompt or a cmdline popup");
    texts(popup.prompt_mut().completions())
}

fn send(keys_tx: &Keys, keys: &str) -> anyhow::Result<()> {
    for key in helix_view::input::parse_macro(keys)? {
        keys_tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    Ok(())
}

/// Runs the event loop (keys, job callbacks) until `done` holds or `limit` passes, returning
/// whether `done` held.
async fn run_until(
    app: &mut Application,
    keys: &mut KeyStream,
    limit: Duration,
    mut done: impl FnMut(&mut Application) -> bool,
) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done(app) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let step = (deadline - now).min(Duration::from_millis(50));
        let _ = tokio::time::timeout(step, app.event_loop_until_idle(keys)).await;
    }
}

/// Typing into a prompt with a slow completer does not wait for it: the keys are handled
/// right away, the completions arrive once computed, and a burst of keys runs the completer
/// once for the first key and once more for the final line (one task in flight at a time),
/// never showing the results for a stale line.
async fn slow_completion_arrives_later(popup: bool) -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    let runs = Arc::new(AtomicUsize::new(0));
    let completer = slow_completer(Duration::from_millis(400), runs.clone());
    if popup {
        app.compositor.push(Box::new(ui::CmdlinePopup::new(
            "Cmdline".into(),
            None,
            completer,
            ignore,
            helix_view::editor::CmdlineStyle::Popup,
        )));
    } else {
        app.compositor.push(Box::new(Prompt::new(
            "slow:".into(),
            None,
            completer,
            ignore,
        )));
    }

    let (keys_tx, keys_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut keys = UnboundedReceiverStream::new(keys_rx);
    send(&keys_tx, "abcd")?;

    // All four keys are handled well before a single completer run could finish.
    let started = Instant::now();
    let typed = run_until(&mut app, &mut keys, Duration::from_millis(350), |app| {
        let line = match app.compositor.find::<Prompt>() {
            Some(prompt) => prompt.line().clone(),
            None => app
                .compositor
                .find::<ui::CmdlinePopup>()
                .unwrap()
                .prompt_mut()
                .line()
                .clone(),
        };
        line == "abcd"
    })
    .await;
    assert!(typed, "the keys were not handled while completing");
    assert!(started.elapsed() < Duration::from_millis(350));
    assert_eq!(completions(&mut app), Vec::<String>::new());

    let mut seen = Vec::new();
    let arrived = run_until(&mut app, &mut keys, Duration::from_secs(5), |app| {
        let now = completions(app);
        if !now.is_empty() {
            seen.push(now.clone());
        }
        now == ["done:abcd"]
    })
    .await;
    assert!(arrived, "the completions never arrived: {seen:?}");
    assert!(
        seen.iter().all(|completions| completions == &["done:abcd"]),
        "stale completions were shown: {seen:?}"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);

    test_key_sequences(&mut app, vec![(Some("<esc>"), None)], false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_completion_arrives_later_in_prompt() -> anyhow::Result<()> {
    slow_completion_arrives_later(false).await
}

/// The cmdline popup wraps its prompt: the completions still reach it.
#[tokio::test(flavor = "multi_thread")]
async fn slow_completion_arrives_later_in_cmdline_popup() -> anyhow::Result<()> {
    slow_completion_arrives_later(true).await
}

/// A deferred completer that is fast shows its completions with the key, like an immediate
/// one (no flicker).
#[tokio::test(flavor = "multi_thread")]
async fn fast_deferred_completion_is_shown_at_once() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    let runs = Arc::new(AtomicUsize::new(0));
    let mut prompt = Prompt::new(
        "fast:".into(),
        None,
        slow_completer(Duration::ZERO, runs.clone()),
        ignore,
    );
    prompt.insert_str("x", &app.editor);
    assert_eq!(texts(prompt.completions()), ["done:x"]);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    app.compositor.push(Box::new(prompt));
    test_key_sequences(&mut app, vec![(Some("<esc>"), None)], false).await
}

/// `Ctrl-r` lists the registers in place of the completions; a slow completion finishing
/// afterwards does not replace that list.
#[tokio::test(flavor = "multi_thread")]
async fn register_list_is_not_replaced_by_a_late_completion() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    let runs = Arc::new(AtomicUsize::new(0));
    app.compositor.push(Box::new(Prompt::new(
        "slow:".into(),
        None,
        slow_completer(Duration::from_millis(300), runs.clone()),
        ignore,
    )));

    let (keys_tx, keys_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut keys = UnboundedReceiverStream::new(keys_rx);
    send(&keys_tx, "a<C-r>")?;
    let registers = run_until(&mut app, &mut keys, Duration::from_secs(1), |app| {
        !completions(app).is_empty()
    })
    .await;
    assert!(registers, "Ctrl-r listed no registers");
    let listed = completions(&mut app);

    // Well past the completer's run: the register list stays.
    let replaced = run_until(&mut app, &mut keys, Duration::from_millis(800), |app| {
        completions(app) != listed
    })
    .await;
    assert!(
        !replaced,
        "the register list was replaced: {:?}",
        completions(&mut app)
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    test_key_sequences(&mut app, vec![(Some("<esc><esc>"), None)], false).await
}
