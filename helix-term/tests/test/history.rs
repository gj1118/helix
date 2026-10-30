use super::*;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Ada",
            "-c",
            "user.email=ada@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn has_git() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok()
}

/// A repository whose `a.txt` was committed twice.
fn repo() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    git(&root, &["init", "-q"]);
    let file = root.join("a.txt");
    std::fs::write(&file, "one\ntwo\nthree\n")?;
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-q", "-m", "Add numbers"]);
    std::fs::write(&file, "one\nTWO\nthree\n")?;
    git(&root, &["commit", "-q", "-am", "Shout two"]);
    Ok((dir, file))
}

fn scratch_text(app: &helix_term::application::Application, needle: &str) -> bool {
    app.editor
        .documents()
        .any(|doc| doc.title.is_some() && doc.text().to_string().contains(needle))
}

#[tokio::test(flavor = "multi_thread")]
async fn file_history_opens_the_commit_diff() -> anyhow::Result<()> {
    if !has_git() {
        return Ok(());
    }
    let (_dir, file) = repo()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some(":file-history<ret>"), None),
            // idle rounds while git runs
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (
                Some("<C-n>"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("Shout two"), "{rows}");
                    assert!(rows.contains("Add numbers"), "{rows}");
                }),
            ),
            // back to the newest commit; Enter opens its change to the file
            (Some("<C-p>"), None),
            (Some("<ret>"), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(scratch_text(app, "+TWO"), "diff buffer missing");
                    let doc = app.editor.documents().find(|d| d.title.is_some()).unwrap();
                    assert!(
                        doc.display_name().contains("a.txt @"),
                        "{}",
                        doc.display_name()
                    );
                    assert!(doc.readonly);
                    assert!(!doc.is_modified());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_history_opens_an_older_version() -> anyhow::Result<()> {
    if !has_git() {
        return Ok(());
    }
    let (_dir, file) = repo()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some(":file-history<ret>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            // move to the older commit and open the version at that commit
            (Some("<down>"), None),
            (Some("<A-o>"), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(scratch_text(app, "one\ntwo\nthree"), "old version missing");
                    assert!(!scratch_text(app, "TWO"));
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_history_browses_a_commit_then_a_file() -> anyhow::Result<()> {
    if !has_git() {
        return Ok(());
    }
    let (_dir, file) = repo()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some(":git-history<ret>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            // the commit opens the files it changed ...
            (Some("<ret>"), None),
            (Some("<C-n>"), None),
            (Some("<C-p>"), None),
            (
                Some("<C-n>"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("a.txt"), "{rows}");
                    assert!(rows.contains("modified"), "{rows}");
                }),
            ),
            // ... and the file its diff
            (Some("<ret>"), None),
            (Some("<esc>"), None),
            (
                Some("<esc>"),
                Some(&|app| assert!(scratch_text(app, "+TWO"))),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_change_puts_back_the_head_version() -> anyhow::Result<()> {
    if !has_git() {
        return Ok(());
    }
    let (_dir, file) = repo()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            // change the first and the last line, which are separate hunks
            (Some("iX<esc>jjiY<esc>"), None),
            (Some("l"), None),
            (
                Some("h"),
                Some(&|app| {
                    let doc = app.editor.documents().next().unwrap();
                    assert_eq!(doc.text().to_string(), "Xone\nTWO\ntYhree\n");
                }),
            ),
            // restore the change under the cursor only
            (Some(":restore-change<ret>"), None),
            (Some("l"), None),
            (
                Some("h"),
                Some(&|app| {
                    let doc = app.editor.documents().next().unwrap();
                    assert_eq!(doc.text().to_string(), "Xone\nTWO\nthree\n");
                    assert!(doc.is_modified());
                }),
            ),
            // select everything and restore all of it
            (Some("%:restore-change<ret>"), None),
            (Some("l"), None),
            (
                Some("h"),
                Some(&|app| {
                    let doc = app.editor.documents().next().unwrap();
                    assert_eq!(doc.text().to_string(), "one\nTWO\nthree\n");
                }),
            ),
            // nothing left to restore
            (Some(":restore-change<ret>"), None),
            (
                Some("l"),
                Some(&|app| {
                    let doc = app.editor.documents().next().unwrap();
                    assert_eq!(doc.text().to_string(), "one\nTWO\nthree\n");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
