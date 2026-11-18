use super::*;

/// Every panel, popup and bar of the editor drawn in terminals from 1×1 up: nothing panics
/// (debug builds panic on arithmetic overflow too), whatever is open when the terminal
/// shrinks.
#[tokio::test(flavor = "multi_thread")]
async fn tiny_terminals_draw_without_panicking() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("main.rs");
    std::fs::write(
        &file,
        "/// A doc comment.\nfn main() {\n    let wide = \"日本語 émoji 🎉\";\n    println!(\"{wide}\");\n}\n",
    )?;
    let raw: helix_term::config::ConfigRaw = toml::from_str(
        "[editor]\nbufferline = \"always\"\nrainbow-brackets = true\ncursorline = true\n\
         [editor.breadcrumb]\nenable = true\n[editor.indent-guides]\nrender = true\n\
         [editor.cmdline]\nstyle = \"popup\"\n[editor.gradient-borders]\nenable = true\n\
         [editor.notifications]\nenable = true\nstyle = \"popup\"\n",
    )?;
    let mut config = helpers::test_config();
    let mut editor: helix_view::editor::Config = raw.editor.unwrap().try_into()?;
    editor.lsp = config.editor.lsp.clone();
    editor.word_completion = config.editor.word_completion;
    config.editor = editor;
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_file(&file, None)
        .build()?;

    // what is open while the terminal shrinks and grows
    let states: [(&str, &str); 9] = [
        ("plain", ""),
        ("splits and tabs", "<C-w>v<C-w>s:tab-new<ret>"),
        ("command line", ":buffer"),
        ("picker", "<space>f"),
        ("buffer picker", "<space>b"),
        ("explorer", "<space>x"),
        ("quickfix panel", ":copen<ret>"),
        ("folds and a message", "jzf:echo a message to show<ret>"),
        ("insert mode", "ihello"),
    ];
    let sizes: [(u16, u16); 10] = [
        (1, 1),
        (2, 2),
        (3, 1),
        (1, 6),
        (6, 3),
        (12, 4),
        (24, 6),
        (80, 2),
        (4, 40),
        (80, 24),
    ];
    for (name, open) in states {
        send_keys(&mut app, open).await?;
        for (width, height) in sizes {
            app.resize_terminal(width, height).await;
            send_keys(&mut app, "").await?;
            let rows: Vec<String> = app.rendered_rows();
            assert_eq!(rows.len(), height as usize, "{name} at {width}x{height}");
        }
        // back to a plain editor for the next one
        send_keys(&mut app, "<esc><esc><esc>").await?;
        app.resize_terminal(80, 24).await;
        send_keys(&mut app, "").await?;
    }
    send_keys(&mut app, ":tab-only<ret><C-w>o:q!<ret>").await?;
    Ok(())
}
