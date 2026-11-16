use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

/// Job callbacks (what handlers and background tasks hand to the editor) run during a burst of
/// input, not only once it is over: left waiting, their queue filled up, the handlers blocked
/// on it, and each key then waited on the handlers.
#[tokio::test(flavor = "multi_thread")]
async fn job_callbacks_run_during_a_burst_of_input() -> anyhow::Result<()> {
    const KEYS: usize = 2000;
    static RAN_AT: AtomicUsize = AtomicUsize::new(usize::MAX);

    let mut app = helpers::AppBuilder::new()
        .with_input_text(format!("#[x|]#{}\n", "x".repeat(2 * KEYS)))
        .build()?;
    // The callback notes where the cursor is when it runs.
    helix_term::job::dispatch_blocking(|editor, _| {
        let (view, doc) = helix_view::current_ref!(editor);
        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        RAN_AT.store(cursor, Ordering::SeqCst);
    });
    // The input is all there already: at every turn of the event loop a key is ready.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(&"l".repeat(KEYS))? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;

    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(cursor, KEYS, "every key was handled");
    let ran_at = RAN_AT.load(Ordering::SeqCst);
    assert_eq!(ran_at, 0usize, "the callback ran before the first key");
    Ok(())
}

/// Keys typed ahead of a callback queued before them are handled after it: the callback that
/// closes the prompts when a document loses focus must not close the prompt a key typed
/// right after opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_callback_queued_before_a_key_does_not_act_on_what_the_key_opens() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new().build()?;
    helix_term::job::dispatch_blocking(|_, compositor| {
        compositor.remove_type::<helix_term::ui::Prompt>();
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in helix_view::input::parse_macro(":")? {
        tx.send(Ok(termina::Event::Key(key.into())))?;
    }
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut stream).await;
    // the editor and the prompt
    assert_eq!(app.compositor.layer_count(), 2usize);
    Ok(())
}
