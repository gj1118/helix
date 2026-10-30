use super::*;

use helix_term::application::Application;
use helix_term::config::{Config, ConfigLoadError};
use helix_view::quicklist::{Entry, Position, Target};

fn config(toml: &str) -> Config {
    Config::load(Ok(&toml.to_owned()), Err(ConfigLoadError::default())).unwrap()
}

/// A PNG of `width` x `height` pixels: red on the top half and blue below.
fn write_png(path: &std::path::Path, width: u32, height: u32) {
    let image = image::RgbaImage::from_fn(width, height, |_, y| {
        if y < height / 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        }
    });
    image.save(path).unwrap();
}

/// An app with a text file open, `files` in the quicklist, and the quicklist picker open
/// (its entries are known right away, unlike those of the file picker).
fn app_with_quicklist(toml: &str, files: &[&std::path::Path]) -> anyhow::Result<Application> {
    let text = tempfile::NamedTempFile::new()?;
    let mut app = helpers::AppBuilder::new()
        .with_config(config(toml))
        .with_file(text.path(), None)
        .build()?;
    // keep the file around for as long as the app
    std::mem::forget(text);
    for file in files {
        app.editor.quicklist.push(Entry::new(
            Target::Path(file.to_path_buf()),
            Position::Lines { start: 0, end: 0 },
            0,
            "",
        ));
    }
    Ok(app)
}

fn screen(app: &Application) -> String {
    app.rendered_rows().join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preview_of_an_image_is_made_of_half_blocks() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let png = dir.path().join("pic.png");
    write_png(&png, 64, 64);

    let mut app = app_with_quicklist(
        "[editor.image-preview]\nprotocol = \"halfblocks\"\n",
        &[&png],
    )?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(screen.contains('▀'), "no image:\n{screen}");
                    assert!(!screen.contains("<Binary file>"), "{screen}");
                    assert!(app.raw_output().is_empty());
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn half_blocks_keep_the_proportions_of_the_image() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let png = dir.path().join("wide.png");
    // 8 times as wide as high
    write_png(&png, 800, 100);

    let mut app = app_with_quicklist(
        "[editor.image-preview]\nprotocol = \"halfblocks\"\n",
        &[&png],
    )?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let rows = app.rendered_rows();
                    let image_rows: Vec<_> = rows.iter().filter(|row| row.contains('▀')).collect();
                    assert!(!image_rows.is_empty(), "no image:\n{}", screen(app));
                    // a cell is 1 x 2 pixels: 8 times as wide as high is 16 columns per row
                    let width = image_rows[0].chars().filter(|c| *c == '▀').count();
                    assert!(
                        (image_rows.len() * 16).abs_diff(width) <= 16,
                        "{} rows for {width} columns\n{}",
                        image_rows.len(),
                        screen(app)
                    );
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_images_go_to_the_terminal_once() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let png = dir.path().join("pic.png");
    write_png(&png, 64, 64);

    let mut app = app_with_quicklist("[editor.image-preview]\nprotocol = \"kitty\"\n", &[&png])?;
    app.set_cell_size(Some((10, 20)));
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let raw = app.raw_output();
                    let summary: Vec<_> = raw
                        .iter()
                        .map(|(x, y, bytes)| {
                            (
                                *x,
                                *y,
                                String::from_utf8_lossy(&bytes[..bytes.len().min(60)]).into_owned(),
                            )
                        })
                        .collect();
                    assert_eq!(raw.len(), 1, "{summary:?}");
                    let (_, _, bytes) = &raw[0];
                    let text = String::from_utf8_lossy(bytes);
                    assert!(text.starts_with("\x1b_Ga=T,f=100,t=d,i="), "{text}");
                    // no half blocks as well
                    assert!(!screen(app).contains('▀'));
                }),
            ),
            // a frame without changes does not send it again
            (
                Some("<left>"),
                Some(&|app| assert_eq!(app.raw_output().len(), 1)),
            ),
            // closing the picker deletes the image from the terminal
            (
                Some("<esc>"),
                Some(&|app| {
                    let raw = app.raw_output();
                    assert_eq!(raw.len(), 2, "{}", screen(app));
                    let text = String::from_utf8_lossy(&raw[1].2).into_owned();
                    assert!(text.starts_with("\x1b_Ga=d,d=I,i="), "{text}");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn images_are_binary_files_when_the_preview_is_disabled() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let png = dir.path().join("pic.png");
    write_png(&png, 16, 16);

    let mut app = app_with_quicklist("[editor.image-preview]\nenable = false\n", &[&png])?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| assert!(screen(app).contains("<Binary file>"), "{}", screen(app))),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_image_says_why() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let png = dir.path().join("broken.png");
    // a PNG signature and nothing else
    std::fs::write(&png, b"\x89PNG\r\n\x1a\nnot an image at all")?;

    let mut app = app_with_quicklist(
        "[editor.image-preview]\nprotocol = \"halfblocks\"\n",
        &[&png],
    )?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>q"),
                Some(&|app| {
                    let screen = screen(app);
                    assert!(!screen.contains('▀'), "{screen}");
                    assert!(screen.contains("│ <"), "no message:\n{screen}");
                }),
            ),
            (Some("<esc>"), None),
        ],
        false,
    )
    .await
}
