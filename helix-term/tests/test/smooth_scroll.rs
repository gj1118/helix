use super::*;

use helix_term::application::Application;
use helix_view::view::ViewPosition;

fn config(smooth_scroll: &str) -> Config {
    let raw: helix_term::config::ConfigRaw =
        toml::from_str(&format!("[editor.smooth-scroll]\n{smooth_scroll}\n")).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// The view's position and the cursor of the focused view.
fn state(app: &Application) -> (ViewPosition, usize) {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    (
        doc.view_offset(view.id),
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    )
}

/// The number of the first line on the screen ("line N" rows).
fn top_line(app: &Application) -> usize {
    app.rendered_rows()
        .iter()
        .find_map(|row| {
            let at = row.find("line ")?;
            row[at + 5..]
                .split_whitespace()
                .next()?
                .parse::<usize>()
                .ok()
        })
        .expect("a line on the screen")
}

async fn app_with(
    smooth_scroll: &str,
    file: &tempfile::NamedTempFile,
) -> anyhow::Result<Application> {
    helpers::AppBuilder::new()
        .with_config(config(smooth_scroll))
        .with_file(file.path(), None)
        .build()
}

/// `[editor.smooth-scroll]`: `C-d` glides there. The screen shows the view on its way while
/// the cursor and the view are already where `C-d` puts them without the animation, so the
/// next key acts on the final position.
#[tokio::test(flavor = "multi_thread")]
async fn smooth_scroll_animates_only_the_drawing() -> anyhow::Result<()> {
    let text: String = (1..=600).map(|i| format!("line {i}\n")).collect();
    let file = helpers::temp_file_with_contents(&text)?;

    // without it: where `C-d` goes
    let mut plain = app_with("enable = false", &file).await?;
    send_keys(&mut plain, "<C-d>").await?;
    let expected = state(&plain);
    let expected_top = top_line(&plain);
    assert!(plain.editor.scroll_animation.is_none());
    assert!(expected_top > 1_usize, "{expected_top}");
    quit(plain).await?;

    // with it: a long, even scroll, caught on its way
    let mut app = app_with(
        "enable = true\nduration = 60000\neasing = \"linear\"",
        &file,
    )
    .await?;
    send_keys(&mut app, "<C-d>").await?;
    assert_eq!(state(&app), expected);
    let animation = app
        .editor
        .scroll_animation
        .clone()
        .expect("a smooth scroll");
    assert_eq!(animation.to, expected.0);
    assert_eq!(animation.from, ViewPosition::default());
    assert_eq!(animation.rows, (expected_top - 1) as isize);
    assert_eq!(animation.easing, helix_view::editor::ScrollEasing::Linear);
    let shown = top_line(&app);
    assert!(shown < expected_top, "shown {shown}, final {expected_top}");

    // another `C-d` on its way: from what the screen shows, to two half pages down
    send_keys(&mut app, "<C-d>").await?;
    let second = app
        .editor
        .scroll_animation
        .clone()
        .expect("a smooth scroll");
    let (offset, _) = state(&app);
    assert_eq!(second.to, offset);
    assert!(second.from.anchor < animation.to.anchor, "{second:?}");
    assert!(offset.anchor > animation.to.anchor, "{second:?}");

    // a jump ends it: the screen shows the view where it is
    send_keys(&mut app, "gg").await?;
    assert!(app.editor.scroll_animation.is_none());
    assert_eq!(top_line(&app), 1_usize);

    // `zt` glides too: the line of the cursor to the top (less the scrolloff)
    send_keys(&mut app, "40jzt").await?;
    let align = app
        .editor
        .scroll_animation
        .clone()
        .expect("a smooth scroll");
    assert_eq!(align.from, ViewPosition::default());
    assert_eq!(align.rows, 35_isize);
    let shown = top_line(&app);
    assert!(shown < 36_usize, "shown {shown}");
    quit(app).await?;

    // a short animation ends on the final position
    let mut app = app_with("enable = true\nduration = 30", &file).await?;
    send_keys(&mut app, "<C-d>").await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    send_keys(&mut app, "").await?;
    assert!(app.editor.scroll_animation.is_none());
    assert_eq!(state(&app), expected);
    assert_eq!(top_line(&app), expected_top);
    // one row at a time (`z j`) does not animate
    send_keys(&mut app, "zj").await?;
    assert!(app.editor.scroll_animation.is_none());
    quit(app).await?;
    Ok(())
}

/// The setting: off by default, with its duration and easing.
#[test]
fn smooth_scroll_setting() {
    use helix_view::editor::{ScrollEasing, SmoothScrollConfig};
    let default = SmoothScrollConfig::default();
    assert!(!default.enable);
    assert_eq!(default.duration, 150_u64);
    assert_eq!(default.easing, ScrollEasing::EaseOut);
    let config = config("enable = true\nduration = 250\neasing = \"ease-in-out\"");
    let smooth = config.editor.smooth_scroll;
    assert!(smooth.enable);
    assert_eq!(smooth.duration, 250_u64);
    assert_eq!(smooth.easing, ScrollEasing::EaseInOut);
    // an unknown easing is refused
    let raw: helix_term::config::ConfigRaw =
        toml::from_str("[editor.smooth-scroll]\neasing = \"bounce\"\n").unwrap();
    let editor: Result<helix_view::editor::Config, _> = raw.editor.unwrap().try_into();
    assert!(editor.is_err());
}
