use super::*;

fn subword_config() -> Config {
    let mut config = helpers::test_config();
    config.editor.subword_motions = true;
    config
}

/// `[editor] subword-motions`: `w`, `b`, `e` stop at the parts of `camelCase` and
/// `snake_case` names (and select them, as words are); off, they go by whole words.
#[tokio::test(flavor = "multi_thread")]
async fn word_motions_by_subwords() -> anyhow::Result<()> {
    let cases: [(&str, &str, &str); 6] = [
        ("#[p|]#arseHttpResponse x", "w", "#[parse|]#HttpResponse x"),
        ("#[p|]#arseHttpResponse x", "ww", "parse#[Http|]#Response x"),
        ("#[m|]#ax_line_width x", "w", "#[max_|]#line_width x"),
        ("#[m|]#ax_line_width x", "e", "#[max|]#_line_width x"),
        ("parseHttp#[R|]#esponse x", "b", "parse#[|Http]#Response x"),
        ("parseHttpResponse #[x|]#", "bb", "parse#[|Http]#Response x"),
    ];
    for case in cases {
        test_with_config(AppBuilder::new().with_config(subword_config()), case).await?;
    }
    // off (the default): whole words
    test(("#[p|]#arseHttpResponse x", "w", "#[parseHttpResponse |]#x")).await?;
    // select mode extends by sub-words
    test_with_config(
        AppBuilder::new().with_config(subword_config()),
        (
            "#[p|]#arseHttpResponse x",
            "vww",
            "#[parseHttp|]#Response x",
        ),
    )
    .await?;
    Ok(())
}

/// `mii` / `mai`: the indentation block at each cursor, with the line it hangs from and its
/// closing line around; a count steps out.
#[tokio::test(flavor = "multi_thread")]
async fn indentation_text_object() -> anyhow::Result<()> {
    test((
        "fn f() {\n    let a = 1;\n    #[|a]#\n}\n",
        "mii",
        "fn f() {\n#[    let a = 1;\n    a\n|]#}\n",
    ))
    .await?;
    test((
        "fn f() {\n    let a = 1;\n    #[|a]#\n}\n",
        "mai",
        "#[fn f() {\n    let a = 1;\n    a\n}\n|]#",
    ))
    .await?;
    test((
        "def f():\n    if a:\n        #[|b]#()\n    c()\n",
        "2mii",
        "def f():\n#[    if a:\n        b()\n    c()\n|]#",
    ))
    .await?;
    // at each cursor
    test((
        "a:\n  #[|b]#\nc:\n  #(|d)#\n",
        "mii",
        "a:\n#[  b\n|]#c:\n#(  d\n|)#",
    ))
    .await?;
    Ok(())
}

/// `A-Z` saves the selections (all of them), `A-z` restores them, mapped through the edits
/// made since; a register names them (`"a`); `A-m` adds them to the selections.
#[tokio::test(flavor = "multi_thread")]
async fn saved_selections() -> anyhow::Result<()> {
    // two cursors saved, then gone, then back where their text moved to
    test((
        "#[|one]# two #(|three)#\n",
        "<A-Z>,ggiNEW <esc><A-z>",
        "NEW #[|one]# two #(|three)#\n",
    ))
    .await?;
    // in a register of its own: the default one keeps its own
    test((
        "#[|one]# two three\n",
        "<A-Z>ww\"a<A-Z>gg\"a<A-z>",
        "one #[two |]#three\n",
    ))
    .await?;
    // added to the selections
    test((
        "#[|one]# two three\n",
        "<A-Z>ww<A-m>",
        // (the current selection stays the primary one)
        "#(|one)# #[two |]#three\n",
    ))
    .await?;

    let mut app = helpers::AppBuilder::new().build()?;
    test_key_sequences(
        &mut app,
        vec![
            // nothing saved
            (
                Some("\"q<A-z>"),
                Some(&|app| {
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(status.as_deref(), Some("no selections saved in q"));
                }),
            ),
            // in another buffer: restored there
            (
                Some("ihello<esc>b<A-Z>:new<ret>iother<esc><A-z>"),
                Some(&|app| {
                    let doc = helix_view::doc!(app.editor);
                    assert_eq!(doc.text().to_string(), "hello\n");
                    let selection = doc.selection(app.editor.tree.focus).primary();
                    assert_eq!((selection.from(), selection.to()), (0_usize, 5_usize));
                }),
            ),
            // a closed buffer takes its saved selections with it
            (
                Some(":bc!<ret><A-z>"),
                Some(&|app| {
                    assert!(app.editor.saved_selections.is_empty());
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(status.as_deref(), Some("no selections saved in ^"));
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// `g;` / `g,` walk the places of the last changes, newest first; typing a line is one place.
#[tokio::test(flavor = "multi_thread")]
async fn changelist() -> anyhow::Result<()> {
    let line = |app: &helix_term::application::Application| {
        let doc = helix_view::doc!(app.editor);
        let text = doc.text().slice(..);
        text.char_to_line(doc.selection(app.editor.tree.focus).primary().cursor(text))
    };
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[|a]#\nb\nc\nd\ne\n")
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            // changes on lines 0, 2 and 4 (several keys on line 4: one place)
            (Some("ix<esc>2jiy<esc>2jizz<esc>gg"), None),
            (Some("g;"), Some(&|app| assert_eq!(line(app), 4_usize))),
            (Some("g;"), Some(&|app| assert_eq!(line(app), 2_usize))),
            (Some("g;"), Some(&|app| assert_eq!(line(app), 0_usize))),
            (
                Some("g;"),
                Some(&|app| {
                    assert_eq!(line(app), 0_usize);
                    let status = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert_eq!(status.as_deref(), Some("at the oldest change"));
                }),
            ),
            (Some("g,"), Some(&|app| assert_eq!(line(app), 2_usize))),
            // with a count
            (Some("g;2g,"), Some(&|app| assert_eq!(line(app), 4_usize))),
            // the places follow the text: a line added above moves them down
            (
                Some("ggOnew<esc>g;"),
                Some(&|app| assert_eq!(line(app), 0_usize)),
            ),
            (Some("g;"), Some(&|app| assert_eq!(line(app), 5_usize))),
        ],
        false,
    )
    .await?;
    Ok(())
}
