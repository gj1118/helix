use super::*;

use helix_term::application::Application;

/// The background of the first cell of the text of the focused window.
fn first_text_cell_bg(app: &Application) -> helix_view::graphics::Color {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    app.rendered_cell(inner.x, inner.y).bg
}

/// A yank highlights what it took for `yank-highlight` milliseconds, over the selection; an
/// edit or the time running out ends it, and 0 turns it off.
#[tokio::test(flavor = "multi_thread")]
async fn yanked_text_is_highlighted_for_a_moment() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("hello world\nsecond\n")?;
    let mut config = helpers::test_config();
    config.editor.yank_highlight = 400;
    // a theme where the yank highlight and the selection differ
    config.theme = None;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;
    app.editor.theme = helix_view::Theme::from(toml::from_str::<toml::Value>(
        r##"
            "ui.selection" = { bg = "#101010" }
            "ui.selection.primary" = { bg = "#101010" }
            "ui.highlight.yank" = { bg = "#f0a000" }
            "##,
    )?);

    test_key_sequences(
        &mut app,
        vec![
            // `wy`: "hello " yanked
            (
                Some("wy"),
                Some(&|app| {
                    let flash = app.editor.yank_flash.as_ref().expect("a flash");
                    assert_eq!(flash.ranges, vec![0_usize..6_usize]);
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let ranges: Option<Vec<std::ops::Range<usize>>> =
                        app.editor.yank_flash_in(view, doc).map(<[_]>::to_vec);
                    assert_eq!(ranges, Some(vec![0_usize..6_usize]));
                    assert_eq!(
                        first_text_cell_bg(app),
                        helix_view::graphics::Color::Rgb(0xf0, 0xa0, 0x00)
                    );
                    // (the time runs out before the next keys)
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }),
            ),
            // over: the selection's own color again
            (
                Some("<esc>"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(app.editor.yank_flash_in(view, doc).is_none());
                    assert_ne!(
                        first_text_cell_bg(app),
                        helix_view::graphics::Color::Rgb(0xf0, 0xa0, 0x00)
                    );
                }),
            ),
            // an edit ends it at once
            (
                Some("xyix<esc>"),
                Some(&|app| {
                    assert!(app.editor.yank_flash.is_some());
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(app.editor.yank_flash_in(view, doc).is_none());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yank_highlight_of_0_is_off() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("hello\n")?;
    let mut config = helpers::test_config();
    config.editor.yank_highlight = 0;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;
    test_key_sequence(
        &mut app,
        Some("xy"),
        Some(&|app| assert!(app.editor.yank_flash.is_none())),
        false,
    )
    .await?;
    Ok(())
}
