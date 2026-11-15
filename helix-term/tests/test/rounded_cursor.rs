use super::*;

use helix_term::application::Application;
use helix_term::config::{Config, ConfigLoadError};
use std::collections::HashMap;

use helix_view::graphics::{Color, CursorKind, Modifier};

fn config(toml: &str) -> Config {
    // the image previews' protocol is set: the terminal running the tests doesn't count
    let toml = format!("[editor]\n{toml}\n[editor.image-preview]\nprotocol = \"halfblocks\"\n");
    Config::load(Ok(&toml), Err(ConfigLoadError::default())).unwrap()
}

/// A theme in hex colors (an image can't be drawn in a terminal's palette colors), with the
/// cursor reversed like the default theme's.
const THEME: &str = r##""ui.background" = { bg = "#101010" }
"ui.text" = { fg = "#c0c0c0" }
"ui.selection" = { bg = "#303030" }
"ui.cursor" = { modifiers = ["reversed"] }"##;

fn app(toml: &str, text: &str, kitty: bool) -> anyhow::Result<Application> {
    app_with_theme(toml, text, kitty, THEME)
}

fn app_with_theme(toml: &str, text: &str, kitty: bool, theme: &str) -> anyhow::Result<Application> {
    let mut app = helpers::AppBuilder::new()
        .with_config(config(toml))
        .with_input_text(text)
        .build()?;
    app.editor.set_theme(toml::from_str(theme)?)?;
    app.set_kitty_graphics(kitty);
    app.set_cell_size(Some((10, 20)));
    Ok(app)
}

/// The kitty graphics commands sent so far whose control data starts with `action`, with
/// where they were sent and their keys.
fn kitty_commands(app: &Application, action: &str) -> Vec<(u16, u16, HashMap<String, u32>)> {
    app.raw_output()
        .iter()
        .filter_map(|(x, y, bytes)| {
            let text = String::from_utf8_lossy(bytes);
            let control = text.strip_prefix("\x1b_G")?.split([';', '\x1b']).next()?;
            control.starts_with(action).then(|| {
                let keys = control
                    .split(',')
                    .filter_map(|pair| {
                        let (key, value) = pair.split_once('=')?;
                        Some((key.to_string(), value.parse().ok()?))
                    })
                    .collect();
                (*x, *y, keys)
            })
        })
        .collect()
}

/// The cursor images' placements shown so far: where, the image id and the placement id.
fn placements(app: &Application) -> Vec<(u16, u16, u32, u32)> {
    kitty_commands(app, "a=p,")
        .into_iter()
        .map(|(x, y, keys)| (x, y, keys["i"], keys["p"]))
        .collect()
}

/// The ids of the cursor images sent so far.
fn transmitted(app: &Application) -> Vec<u32> {
    kitty_commands(app, "a=t,")
        .into_iter()
        .map(|(_, _, keys)| keys["i"])
        .collect()
}

/// The placements taken away so far: image id and placement id.
fn unplaced(app: &Application) -> Vec<(u32, u32)> {
    kitty_commands(app, "a=d,d=i,")
        .into_iter()
        .map(|(_, _, keys)| (keys["i"], keys["p"]))
        .collect()
}

fn cursor_kind(app: &Application) -> CursorKind {
    app.compositor.cursor(app.compositor.size(), &app.editor).1
}

fn symbol(app: &Application, x: u16, y: u16) -> String {
    app.rendered_cell(x, y).symbol.to_string()
}

/// The primary cursor is a rounded image under its character, which keeps the cursor's text
/// color; the terminal's own cursor is hidden. The image is sent once: moving the cursor moves
/// its placement, without taking it away first (no flicker).
#[tokio::test(flavor = "multi_thread")]
async fn a_rounded_cursor_is_an_image_under_its_character() -> anyhow::Result<()> {
    let mut app = app("cursor-roundness = 25", "#[h|]#ello\nworld\n", true)?;
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    let placements = placements(app);
                    assert_eq!(placements.len(), 1usize, "{placements:?}");
                    let (x, y, image, placement) = placements[0];
                    assert_eq!(transmitted(app), vec![image]);
                    assert_eq!(placement, 1u32);
                    assert_eq!(symbol(app, x, y), "h");
                    // the default theme's cursor is reversed: the text in the background's
                    // color, the cell's own background under the image
                    let cell = app.rendered_cell(x, y);
                    let background = app.editor.theme.get("ui.background").bg.unwrap();
                    assert!(matches!(background, Color::Rgb(..)), "{background:?}");
                    assert_eq!(cell.fg, background);
                    assert_eq!(cell.bg, background);
                    assert!(!cell.modifier.contains(Modifier::REVERSED));
                    assert_eq!(cursor_kind(app), CursorKind::Hidden);
                }),
            ),
            (
                Some("lj"),
                Some(&|app| {
                    let placements = placements(app);
                    let (first_x, first_y, image, _) = placements[0];
                    let &(x, y, moved_image, placement) = placements.last().unwrap();
                    assert_eq!((x, y), (first_x + 1, first_y + 1));
                    assert_eq!((moved_image, placement), (image, 1u32));
                    assert_eq!(symbol(app, x, y), "o");
                    // sent once, never taken away
                    assert_eq!(transmitted(app), vec![image]);
                    assert_eq!(unplaced(app), Vec::<(u32, u32)>::new());
                }),
            ),
        ],
        false,
    )
    .await
}

/// Each cursor in view is its own placement of the image.
#[tokio::test(flavor = "multi_thread")]
async fn every_cursor_is_rounded() -> anyhow::Result<()> {
    let mut app = app("cursor-roundness = 50", "#[h|]#ello\nworld\n", true)?;
    test_key_sequences(
        &mut app,
        vec![(
            Some("C"),
            Some(&|app| {
                let placements = placements(app);
                assert!(placements.len() >= 2usize, "{placements:?}");
                let last_two = &placements[placements.len() - 2..];
                let mut ids: Vec<u32> = last_two.iter().map(|placement| placement.3).collect();
                ids.sort();
                assert_eq!(ids, vec![1u32, 2]);
                let mut symbols: Vec<String> = last_two
                    .iter()
                    .map(|&(x, y, _, _)| symbol(app, x, y))
                    .collect();
                symbols.sort();
                assert_eq!(symbols, vec!["h".to_string(), "w".to_string()]);
                // one image for both (the secondary cursor's style is the primary's here)
                assert_eq!(transmitted(app).len(), 1usize);
            }),
        )],
        false,
    )
    .await
}

/// `cursor-roundness = 0` (the default) leaves the cursor as it was: the terminal's own.
#[tokio::test(flavor = "multi_thread")]
async fn no_roundness_leaves_the_cursor_as_it_was() -> anyhow::Result<()> {
    let mut app = app("cursor-roundness = 0", "#[h|]#ello\n", true)?;
    test_key_sequences(
        &mut app,
        vec![(
            Some("l"),
            Some(&|app| {
                assert_eq!(placements(app), Vec::<(u16, u16, u32, u32)>::new());
                assert_eq!(cursor_kind(app), CursorKind::Block);
            }),
        )],
        false,
    )
    .await
}

/// A terminal without the kitty graphics protocol keeps the square cursor.
#[tokio::test(flavor = "multi_thread")]
async fn without_kitty_graphics_the_cursor_stays_square() -> anyhow::Result<()> {
    let mut app = app("cursor-roundness = 25", "#[h|]#ello\n", false)?;
    test_key_sequences(
        &mut app,
        vec![(
            Some("l"),
            Some(&|app| {
                assert_eq!(placements(app), Vec::<(u16, u16, u32, u32)>::new());
                assert_eq!(cursor_kind(app), CursorKind::Block);
            }),
        )],
        false,
    )
    .await
}

/// A bar cursor (insert mode here) isn't rounded: the placement goes when insert mode starts,
/// and the terminal draws the bar.
#[tokio::test(flavor = "multi_thread")]
async fn a_bar_cursor_is_not_rounded() -> anyhow::Result<()> {
    let mut app = app(
        "cursor-roundness = 25\n[editor.cursor-shape]\ninsert = \"bar\"",
        "#[h|]#ello\n",
        true,
    )?;
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    assert_eq!(placements(app).len(), 1usize);
                    assert_eq!(cursor_kind(app), CursorKind::Hidden);
                }),
            ),
            (
                Some("i"),
                Some(&|app| {
                    let (_, _, image, placement) = placements(app)[0];
                    assert_eq!(placements(app).len(), 1usize);
                    assert_eq!(unplaced(app), vec![(image, placement)]);
                    assert_eq!(cursor_kind(app), CursorKind::Bar);
                }),
            ),
        ],
        false,
    )
    .await
}

/// A cursor in its own colors (not reversed): the image in its background color, the text in
/// its foreground color.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_in_its_own_colors() -> anyhow::Result<()> {
    let theme =
        format!("{THEME}\n\"ui.cursor.primary\" = {{ fg = \"#000000\", bg = \"#e0a040\" }}");
    let mut app = app_with_theme("cursor-roundness = 25", "#[h|]#ello\n", true, &theme)?;
    test_key_sequences(
        &mut app,
        vec![(
            None,
            Some(&|app| {
                let placements = placements(app);
                assert_eq!(placements.len(), 1usize, "{placements:?}");
                let (x, y, _, _) = placements[0];
                let cell = app.rendered_cell(x, y);
                assert_eq!(cell.fg, Color::Rgb(0, 0, 0));
                assert_eq!(cell.bg, Color::Rgb(0x10, 0x10, 0x10));
            }),
        )],
        false,
    )
    .await
}

/// A cursor in a terminal palette color can't be an image: the editor draws it square, as
/// before (the terminal's cursor stays hidden, the editor draws all of them alike).
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_in_palette_colors_is_square() -> anyhow::Result<()> {
    let theme = r#""ui.background" = { bg = "black" }
"ui.text" = { fg = "white" }
"ui.selection" = { bg = "blue" }
"ui.cursor" = { modifiers = ["reversed"] }"#;
    let mut app = app_with_theme("cursor-roundness = 25", "#[h|]#ello\n", true, theme)?;
    test_key_sequences(
        &mut app,
        vec![(
            None,
            Some(&|app| {
                assert_eq!(placements(app), Vec::<(u16, u16, u32, u32)>::new());
                let (x, y) = (0..app.compositor.size().width)
                    .flat_map(|x| (0..3u16).map(move |y| (x, y)))
                    .find(|&(x, y)| {
                        symbol(app, x, y) == "h"
                            && app
                                .rendered_cell(x, y)
                                .modifier
                                .contains(Modifier::REVERSED)
                    })
                    .expect("the square cursor on `h`");
                assert_eq!(symbol(app, x, y), "h");
            }),
        )],
        false,
    )
    .await
}
