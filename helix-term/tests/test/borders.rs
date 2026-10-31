use super::*;
use helix_term::application::Application;

fn config(style: &str) -> Config {
    let raw: helix_term::config::ConfigRaw =
        toml::from_str(&format!("[editor]\nborder-style = \"{style}\"\n")).unwrap();
    Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    }
}

/// The screen while the buffer picker is open.
async fn picker_screen(config: Config) -> anyhow::Result<String> {
    let file = tempfile::NamedTempFile::new()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;
    let screen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let out = screen.clone();
    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>b"), None),
            (Some("<C-n>"), None),
            (
                Some("<C-p>"),
                Some(&move |app| *out.lock().unwrap() = app.rendered_rows().join("\n")),
            ),
        ],
        false,
    )
    .await?;
    let screen = screen.lock().unwrap().clone();
    Ok(screen)
}

#[tokio::test(flavor = "multi_thread")]
async fn border_style_changes_the_picker_frame() -> anyhow::Result<()> {
    let plain = picker_screen(Config::default()).await?;
    assert!(plain.contains('┌') && plain.contains('┘'), "{plain}");

    for (style, corner, edge) in [
        ("plain", '┌', '│'),
        ("rounded", '╭', '│'),
        ("double", '╔', '║'),
        ("thick", '┏', '┃'),
        ("quadrant-inside", '▗', '▐'),
        ("quadrant-outside", '▛', '▌'),
    ] {
        let screen = picker_screen(config(style)).await?;
        assert!(
            screen.contains(corner),
            "{style}: corner {corner}\n{screen}"
        );
        assert!(screen.contains(edge), "{style}: edge {edge}\n{screen}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn border_style_none_hides_the_frame_but_keeps_the_separator() -> anyhow::Result<()> {
    let screen = picker_screen(config("none")).await?;
    for glyph in ['┌', '┐', '└', '┘', '╭', '╮', '╰', '╯', '╔', '▛'] {
        assert!(!screen.contains(glyph), "found {glyph}\n{screen}");
    }
    // the line under the prompt still divides the input from the results
    assert!(screen.contains("───"), "{screen}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rounded_corners_is_overridden_by_border_style() -> anyhow::Result<()> {
    let raw: helix_term::config::ConfigRaw =
        toml::from_str("[editor]\nrounded-corners = true\nborder-style = \"double\"\n")?;
    let cfg = Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    };
    let screen = picker_screen(cfg).await?;
    assert!(screen.contains('╔') && !screen.contains('╭'), "{screen}");

    let raw: helix_term::config::ConfigRaw = toml::from_str("[editor]\nrounded-corners = true\n")?;
    let cfg = Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    };
    let screen = picker_screen(cfg).await?;
    assert!(screen.contains('╭'), "{screen}");
    Ok(())
}

/// Where `text` is on the screen: (column, row), counting one column per character.
fn find(app: &Application, text: &str) -> Option<(u16, u16)> {
    app.rendered_rows()
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            let byte = line.find(text)?;
            Some((line[..byte].chars().count() as u16, row as u16))
        })
}

type Color = helix_view::graphics::Color;

/// The foreground of the top left and top right corner of the delete confirmation of the
/// explorer, and of the top left corner of the buffer picker.
async fn confirm_box_corners(config: Config) -> anyhow::Result<(Color, Color, Color)> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("to_delete.txt");
    std::fs::write(&file, "x")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(&file, None)
        .with_config(config)
        .build()?;
    let colors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = colors.clone();
    let picker_out = colors.clone();
    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>b"), None),
            (
                Some("<C-n>"),
                Some(&move |app| {
                    // the first line of the screen with a corner glyph
                    let corner = app
                        .rendered_rows()
                        .iter()
                        .enumerate()
                        .find_map(|(row, line)| {
                            let col = line.chars().position(|c| "┌╭╔┏▗▛".contains(c))?;
                            Some((col as u16, row as u16))
                        })
                        .expect("a picker frame");
                    picker_out
                        .lock()
                        .unwrap()
                        .push(app.rendered_fg(corner.0, corner.1));
                }),
            ),
            (Some("<esc>"), None),
            (Some("<space>x"), None),
            (Some("j"), None),
            (Some("d"), None),
            (
                Some("<C-n>"),
                Some(&move |app| {
                    let (x, y) = find(app, "Delete f").unwrap_or_else(|| {
                        panic!("no confirm box:\n{}", app.rendered_rows().join("\n"))
                    });
                    let left = app.rendered_fg(x - 2, y);
                    // the right corner is where the box ends: after the last frame glyph of the row
                    let row: Vec<char> = app.rendered_rows()[y as usize].chars().collect();
                    let right_x = row
                        .iter()
                        .rposition(|c| "┐╮╗┓▖▜".contains(*c))
                        .expect("the right corner") as u16;
                    out.lock().unwrap().push(app.rendered_fg(right_x, y));
                    out.lock().unwrap().push(left);
                }),
            ),
        ],
        false,
    )
    .await?;
    let colors = colors.lock().unwrap().clone();
    // picker corner, confirm box right corner, confirm box left corner
    Ok((colors[0], colors[2], colors[1]))
}

#[tokio::test(flavor = "multi_thread")]
async fn confirm_box_border_matches_the_pickers_by_default() -> anyhow::Result<()> {
    let (picker, left, right) = confirm_box_corners(Config::default()).await?;
    assert_eq!(left, picker, "confirm box vs picker");
    assert_eq!(right, picker, "confirm box vs picker");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirm_box_border_follows_the_gradient_borders() -> anyhow::Result<()> {
    let raw: helix_term::config::ConfigRaw = toml::from_str(
        r##"
        [editor.gradient-borders]
        enable = true
        start-color = "#ff0000"
        end-color = "#0000ff"
        "##,
    )?;
    let cfg = Config {
        editor: raw.editor.unwrap().try_into().unwrap(),
        ..Default::default()
    };
    let (_, left, right) = confirm_box_corners(cfg).await?;
    assert_ne!(left, right, "a gradient changes color along the border");
    assert!(
        matches!(left, Color::Rgb(r, _, b) if r > b),
        "starts red: {left:?}"
    );
    assert!(
        matches!(right, Color::Rgb(r, _, b) if b > r),
        "ends blue: {right:?}"
    );
    Ok(())
}
