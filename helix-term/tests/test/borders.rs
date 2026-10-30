use super::*;

fn config(style: &str) -> Config {
    let raw: helix_term::config::ConfigRaw =
        toml::from_str(&format!("[editor]\nborder-style = \"{style}\"\n")).unwrap();
    let mut config = Config::default();
    config.editor = raw.editor.unwrap().try_into().unwrap();
    config
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
    let mut cfg = Config::default();
    cfg.editor = raw.editor.unwrap().try_into().unwrap();
    let screen = picker_screen(cfg).await?;
    assert!(screen.contains('╔') && !screen.contains('╭'), "{screen}");

    let raw: helix_term::config::ConfigRaw = toml::from_str("[editor]\nrounded-corners = true\n")?;
    let mut cfg = Config::default();
    cfg.editor = raw.editor.unwrap().try_into().unwrap();
    let screen = picker_screen(cfg).await?;
    assert!(screen.contains('╭'), "{screen}");
    Ok(())
}
