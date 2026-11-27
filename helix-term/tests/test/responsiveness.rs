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

/// The highlights of the lines in view (the syntax's and the rainbow brackets') are kept from
/// one frame to the next while nothing they depend on changes. Whatever happens (moving,
/// scrolling, editing, undoing, another window on the same file, another theme), a frame
/// drawn with what was kept is the frame drawn without it.
#[tokio::test(flavor = "multi_thread")]
async fn the_kept_highlights_are_the_highlights() -> anyhow::Result<()> {
    use helix_term::config::{Config, ConfigLoadError};
    let mut text = String::new();
    for n in 0..120 {
        text.push_str(&format!(
            "fn f{n}(a: Vec<(u8, [u16; 2])>) -> Option<u8> {{ if a.is_empty() {{ None }} else {{ Some((a[0].0 + {n}) as u8) }} }}\n// comment {n} \"str\"\n"
        ));
    }
    let file = helpers::temp_file_with_contents(&text)?;
    let path = file.path().with_extension("rs");
    std::fs::copy(file.path(), &path)?;
    let config = Config::load(
        Ok(&"[editor]\nrainbow-brackets = true\ncursorline = true\n".to_owned()),
        Err(ConfigLoadError::default()),
    )
    .unwrap();
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .with_config(config)
        .build()?;
    app.resize_terminal(100, 30).await;

    // the same frame, with and without what was kept
    async fn check(
        app: &mut helix_term::application::Application,
        step: &str,
    ) -> anyhow::Result<()> {
        let kept = app.rendered_cells();
        app.render_without_caches().await;
        let fresh = app.rendered_cells();
        // the windows' rows: the status line has a spinner that moves from frame to frame
        let rows = 100 * 28;
        if let Some(cell) = kept[..rows]
            .iter()
            .zip(&fresh[..rows])
            .position(|(a, b)| a != b)
        {
            anyhow::bail!(
                "after {step:?}, cell {cell}: kept {:?}, fresh {:?}\n{:#?}",
                kept[cell],
                fresh[cell],
                app.rendered_rows()
            );
        }
        Ok(())
    }
    // highlighted at all: some cells have colors of the syntax
    let colors: std::collections::HashSet<_> =
        app.rendered_cells().iter().map(|cell| cell.fg).collect();
    assert!(colors.len() > 4, "{colors:?}");

    for step in [
        "j",
        "jjjj",
        "w",
        "e",
        "<C-d>",
        "<C-u>",
        "ge",
        "gg",
        "10j",
        "o{ [ ( x ) ] }<esc>",
        "u",
        "U",
        "ddp",
        "<C-w>v",
        "jj",
        "<C-w>w",
        "<C-d>",
        "ihello (<esc>",
        "<C-w>w",
        "%",
        "<gt>",
        "u",
        ":theme base16_default<ret>",
        "j",
        ":theme default<ret>",
        "gg",
        "x",
        "<C-w>q",
        "j",
    ] {
        helpers::send_keys(&mut app, step).await?;
        check(&mut app, step).await?;
    }
    std::fs::remove_file(&path)?;
    Ok(())
}
