use super::*;

static HEAD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Ada Lovelace",
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

fn repo_with_commit() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    git(&root, &["init", "-q"]);
    let file = root.join("a.txt");
    std::fs::write(&file, "one\ntwo\n")?;
    git(&root, &["add", "a.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "Add the numbers\n\nBecause counting matters.",
        ],
    );
    Ok((dir, file))
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_popup_shows_commit_details() -> anyhow::Result<()> {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return Ok(());
    }
    let (_dir, file) = repo_with_commit()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            // The first request computes the file blame, then the popup opens.
            (Some(":blame<ret>"), None),
            (Some("l"), None),
            (Some("h"), None),
            (
                Some("l"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(rows.contains("Ada Lovelace <ada@example.com>"), "{rows}");
                    assert!(rows.contains("Add the numbers"), "{rows}");
                    assert!(rows.contains("Because counting matters."), "{rows}");
                    assert!(rows.contains("y copy commit id"), "{rows}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_popup_copies_commit_id() -> anyhow::Result<()> {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return Ok(());
    }
    let (dir, file) = repo_with_commit()?;
    let mut app = helpers::AppBuilder::new().with_file(&file, None).build()?;

    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "HEAD"])
        .output()?;
    let head = String::from_utf8(head.stdout)?.trim().to_owned();
    HEAD.set(head).unwrap();

    test_key_sequences(
        &mut app,
        vec![
            (Some(":blame<ret>"), None),
            (Some("l"), None),
            (Some("h"), None),
            (Some("l"), None),
            (
                Some("y"),
                Some(&|app| {
                    let rows = app.rendered_rows().join("\n");
                    assert!(
                        !rows.contains("y copy commit id"),
                        "popup should close:\n{rows}"
                    );
                    let copied = app
                        .editor
                        .registers
                        .read('+', &app.editor)
                        .and_then(|mut values| values.next().map(|v| v.to_string()))
                        .unwrap_or_default();
                    assert_eq!(copied.trim(), HEAD.get().unwrap().as_str());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
