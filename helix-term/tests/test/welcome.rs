//! The welcome screen: the logo as large as the view allows.

use super::*;
use helix_term::application::Application;

/// The welcome screen at `width` by `height`, without the notification of the startup.
async fn welcome(width: u16, height: u16) -> anyhow::Result<(Application, Vec<String>)> {
    let mut config = helpers::test_config();
    config.editor.notifications.enable = false;
    let mut app = helpers::AppBuilder::new().with_config(config).build()?;
    app.resize_terminal(width, height).await;
    let rows = app.rendered_rows();
    Ok((app, rows))
}

const WORDMARK: &str = "█  █ █▀▀▀ █    ▀█▀ ▀▄ ▄▀";

fn has(rows: &[String], text: &str) -> bool {
    rows.iter().any(|row| row.contains(text))
}

/// With room: the large mark (15 rows) beside the wordmark, the tagline and the help.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_view_shows_the_mark_and_the_wordmark() -> anyhow::Result<()> {
    let (_app, rows) = welcome(100, 30).await?;
    assert!(has(&rows, WORDMARK), "{rows:#?}");
    assert!(has(&rows, "A post-modern modal text editor"), "{rows:#?}");
    assert!(has(&rows, "learn helix"), "{rows:#?}");
    // the mark's first row, and the wordmark to the right of it, below its top
    let is_block = |c: char| "▀▄█".contains(c);
    let mark_row = rows
        .iter()
        .position(|row| row.chars().any(is_block))
        .unwrap();
    let wordmark_row = rows.iter().position(|row| row.contains(WORDMARK)).unwrap();
    assert!(wordmark_row > mark_row, "{rows:#?}");
    let mark_column = rows[mark_row].chars().position(is_block).unwrap();
    let wordmark_column = rows[wordmark_row].find(WORDMARK).unwrap();
    let wordmark_column = rows[wordmark_row][..wordmark_column].chars().count();
    assert_eq!(wordmark_column, mark_column + 19 + 4);
    // the 15 rows of the mark: half blocks from its first row on
    let blocks = rows[mark_row..mark_row + 15]
        .iter()
        .filter(|row| {
            row.chars()
                .take(mark_column + 19)
                .any(|c| "▀▄█".contains(c))
        })
        .count();
    assert_eq!(blocks, 15);
    Ok(())
}

/// Shorter: the small mark beside the help (no wordmark); shorter still, the help alone; and
/// without room for the help, nothing.
#[tokio::test(flavor = "multi_thread")]
async fn smaller_views_fall_back_to_the_small_mark_then_the_help() -> anyhow::Result<()> {
    // 15 rows: 14 for the view, 13 above its statusline: the small mark (12 rows)
    let (_app, rows) = welcome(100, 15).await?;
    assert!(!has(&rows, WORDMARK), "{rows:#?}");
    assert!(has(&rows, "helix "), "{rows:#?}");
    assert!(has(&rows, "learn helix"), "{rows:#?}");
    let mark_rows = rows
        .iter()
        .filter(|row| row.chars().any(|c| "▀▄█".contains(c)))
        .count();
    assert_eq!(mark_rows, 12, "{rows:#?}");

    // 13 rows: 11 above the statusline, the help alone
    let (_app, rows) = welcome(100, 13).await?;
    assert!(has(&rows, "learn helix"), "{rows:#?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.chars().any(|c| "▀▄█".contains(c))),
        "{rows:#?}"
    );

    // too narrow for the help: nothing
    let (_app, rows) = welcome(30, 30).await?;
    assert!(!has(&rows, "learn helix"), "{rows:#?}");
    Ok(())
}
