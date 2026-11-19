use pretty_assertions::StrComparison;
use ropey::{Rope, RopeSlice};
use std::fmt::Write;
use std::fs;
use std::ops::{Bound, RangeBounds};
use std::path::Path;
use std::time::Duration;
use tree_sitter::Query;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::LanguageLoader;
use crate::highlighter::{Highlight, HighlightEvent, Highlighter};
use crate::query_iter::{QueryIter, QueryIterEvent};
use crate::{Language, Range, Syntax};

macro_rules! w {
    ($dst: expr$(, $($args: tt)*)?) => {{
        let _ = write!($dst$(, $($args)*)?);
    }};
}
macro_rules! wln {
    ($dst: expr$(, $($args: tt)*)?) => {{
        let _ = writeln!($dst$(, $($args)*)?);
    }};
}

pub fn check_fixture(path: impl AsRef<Path>, roundtrip: impl FnOnce(&str) -> String) {
    let path = path.as_ref();
    let snapshot = match fs::read_to_string(path) {
        Ok(content) => content.replace("\r\n", "\n"),
        Err(err) => panic!("Failed to read fixture {path:?}: {err}"),
    };
    let snapshot = snapshot.trim_end();
    let roundtrip = roundtrip(snapshot);
    if snapshot != roundtrip.trim_end() {
        if std::env::var_os("UPDATE_EXPECT").is_some_and(|it| it == "1") {
            println!("\x1b[1m\x1b[92mupdating\x1b[0m: {}", path.display());
            fs::write(path, roundtrip).unwrap();
        } else {
            println!(
                "\n
\x1b[1mCurrent\x1b[0m:
----
{}
----

\x1b[1mGenerated\x1b[0m:
----
{}
----

\x1b[1mDiff\x1b[0m:
----
{}
----
\x1b[1m\x1b[91merror\x1b[97m: fixture test failed\x1b[0m
   \x1b[1m\x1b[34m-->\x1b[0m {}

You can update all fixtures by running:

    env UPDATE_EXPECT=1 cargo test
",
                snapshot,
                roundtrip,
                StrComparison::new(snapshot, &roundtrip.trim_end()),
                path.display(),
            );
        }

        std::panic::resume_unwind(Box::new(()));
    }
}

pub fn strip_annotations(src: &str, comment_prefix: &str) -> Rope {
    let ident = " ".repeat(comment_prefix.width());
    let escape = src.lines().all(|line| {
        line.chars().all(|c| c.is_whitespace())
            || line.starts_with(&ident)
            || line.starts_with(comment_prefix)
    });
    if !escape {
        Rope::from_str(src);
    }
    let mut raw = String::new();
    for mut line in src.split_inclusive('\n') {
        if line.starts_with(comment_prefix) {
            continue;
        }
        line = line.strip_prefix(&ident).unwrap_or(line);
        raw.push_str(line);
    }
    Rope::from_str(&raw)
}

pub fn check_highlighter_fixture<R: RangeBounds<usize>>(
    path: impl AsRef<Path>,
    comment_prefix: &str,
    language: Language,
    loader: &impl LanguageLoader,
    get_highlight_name: impl Fn(Highlight) -> String,
    range: impl Fn(RopeSlice) -> R,
) {
    check_fixture(path, move |src| {
        roundtrip_highlighter_fixture(
            comment_prefix,
            language,
            loader,
            get_highlight_name,
            src,
            range,
        )
    })
}

pub fn check_injection_fixture<R: RangeBounds<usize>>(
    path: impl AsRef<Path>,
    comment_prefix: &str,
    language: Language,
    loader: &impl LanguageLoader,
    get_language_name: impl Fn(Language) -> String,
    range: impl Fn(RopeSlice) -> R,
) {
    check_fixture(path, move |src| {
        roundtrip_injection_fixture(
            comment_prefix,
            language,
            loader,
            get_language_name,
            src,
            range,
        )
    })
}

pub fn roundtrip_highlighter_fixture<R: RangeBounds<usize>>(
    comment_prefix: &str,
    language: Language,
    loader: &impl LanguageLoader,
    get_highlight_name: impl Fn(Highlight) -> String,
    src: &str,
    range: impl Fn(RopeSlice) -> R,
) -> String {
    let raw = strip_annotations(src, comment_prefix);
    let syntax = Syntax::new(raw.slice(..), language, Duration::from_secs(60), loader).unwrap();
    let range = range(raw.slice(..));
    highlighter_fixture(
        comment_prefix,
        loader,
        get_highlight_name,
        &syntax,
        raw.slice(..),
        range,
    )
}

pub fn roundtrip_injection_fixture<R: RangeBounds<usize>>(
    comment_prefix: &str,
    language: Language,
    loader: &impl LanguageLoader,
    get_language_name: impl Fn(Language) -> String,
    src: &str,
    range: impl Fn(RopeSlice) -> R,
) -> String {
    let raw = strip_annotations(src, comment_prefix);
    let syntax = Syntax::new(raw.slice(..), language, Duration::from_secs(60), loader).unwrap();
    let range = range(raw.slice(..));
    injections_fixture(
        comment_prefix,
        loader,
        get_language_name,
        &syntax,
        raw.slice(..),
        range,
    )
}

pub fn highlighter_fixture(
    comment_prefix: &str,
    loader: &impl LanguageLoader,
    get_highlight_name: impl Fn(Highlight) -> String,
    syntax: &Syntax,
    src: RopeSlice<'_>,
    range: impl RangeBounds<usize>,
) -> String {
    let start = match range.start_bound() {
        Bound::Included(&i) => i,
        Bound::Excluded(&i) => i + 1,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&i) => i - 1,
        Bound::Excluded(&i) => i,
        Bound::Unbounded => src.len_bytes(),
    };
    let ident = " ".repeat(comment_prefix.width());
    let mut highlighter = Highlighter::new(syntax, src, &loader, start as u32..);
    let mut pos = highlighter.next_event_offset();
    let mut highlight_stack = Vec::new();
    let mut line_idx = src.byte_to_line(pos as usize);
    let mut line_start = src.line_to_byte(line_idx) as u32;
    let mut line_end = src.line_to_byte(line_idx + 1) as u32;
    let mut line_highlights = Vec::new();
    let mut res = String::new();
    for line in src.byte_slice(..line_start as usize).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{ident}{line}")
        }
    }
    while pos < end as u32 {
        let (event, new_highlights) = highlighter.advance();
        if event == HighlightEvent::Refresh {
            highlight_stack.clear();
        }
        highlight_stack.extend(new_highlights.map(&get_highlight_name));
        let start = pos;
        pos = highlighter.next_event_offset();
        if pos == u32::MAX {
            pos = src.len_bytes() as u32
        }
        if pos <= start {
            panic!(
                "INVALID HIGHLIGHT RANGE: {start}..{pos} '{}' {:?}",
                src.byte_slice(pos as usize..start as usize),
                highlight_stack
            );
        }

        while start >= line_end {
            res.push_str(&ident);
            res.extend(
                src.byte_slice(line_start as usize..line_end as usize)
                    .chunks(),
            );
            annotate_line(
                comment_prefix,
                src,
                line_start,
                &mut line_highlights,
                &mut res,
                false,
            );
            line_highlights.clear();
            line_idx += 1;
            line_start = line_end;
            line_end = src
                .try_line_to_byte(line_idx + 1)
                .unwrap_or(src.len_bytes()) as u32;
        }
        if !highlight_stack.is_empty() {
            let range = start..pos.min(line_end);
            if !range.is_empty() {
                line_highlights.push((range, highlight_stack.clone()))
            }
        }
        while pos > line_end {
            res.push_str(&ident);
            res.extend(
                src.byte_slice(line_start as usize..line_end as usize)
                    .chunks(),
            );
            annotate_line(
                comment_prefix,
                src,
                line_start,
                &mut line_highlights,
                &mut res,
                !highlight_stack.is_empty(),
            );
            line_highlights.clear();
            line_idx += 1;
            line_start = line_end;
            line_end = src
                .try_line_to_byte(line_idx + 1)
                .unwrap_or(src.len_bytes()) as u32;
            line_highlights.is_empty();
            if pos > line_start && !highlight_stack.is_empty() {
                line_highlights.push((line_start..pos.min(line_end), Vec::new()))
            }
        }
    }
    if !line_highlights.is_empty() {
        res.push_str(&ident);
        res.extend(
            src.byte_slice(line_start as usize..line_end as usize)
                .chunks(),
        );
        if !res.ends_with('\n') {
            res.push('\n');
        }
        annotate_line(
            comment_prefix,
            src,
            line_start,
            &mut line_highlights,
            &mut res,
            false,
        );
        line_start = line_end;
    }
    for line in src.byte_slice(line_start as usize..).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{ident}{line}")
        }
    }
    res
}

pub fn injections_fixture(
    comment_prefix: &str,
    loader: &impl LanguageLoader,
    get_language_name: impl Fn(Language) -> String,
    syntax: &Syntax,
    src: RopeSlice<'_>,
    range: impl RangeBounds<usize>,
) -> String {
    let start = match range.start_bound() {
        Bound::Included(&i) => i,
        Bound::Excluded(&i) => i + 1,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&i) => i - 1,
        Bound::Excluded(&i) => i,
        Bound::Unbounded => src.len_bytes(),
    };
    let ident = " ".repeat(comment_prefix.width());
    let lang = syntax.layer(syntax.root).language;
    let language_config = loader.get_config(lang).unwrap();
    let query = Query::new(language_config.grammar, "", |_, _| unreachable!()).unwrap();
    let mut query_iter = QueryIter::<_, ()>::new(syntax, src, |_| Some(&query), start as u32..);
    let event = query_iter.next();
    let mut injection_stack = Vec::new();
    let mut pos = if let Some(QueryIterEvent::EnterInjection(injection)) = event {
        let language = syntax.layer(injection.layer).language;
        injection_stack.push(get_language_name(language));
        injection.range.start
    } else {
        end as u32
    };
    let mut line_idx = src.byte_to_line(pos as usize);
    let mut line_start = src.line_to_byte(line_idx) as u32;
    let mut line_end = src.line_to_byte(line_idx + 1) as u32;
    let mut line_labels = Vec::new();
    let mut res = String::new();
    for line in src.byte_slice(..line_start as usize).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{ident}{line}")
        }
    }
    let mut errors = String::new();
    while pos < end as u32 {
        let Some(event) = query_iter.next() else {
            break;
        };
        let mut start = pos;
        pos = event.start_byte();
        if pos == u32::MAX {
            pos = src.len_bytes() as u32
        }
        if pos <= start {
            wln!(
                errors,
                "INVALID RANGE: {start}..{pos} {:?} {:?}",
                src.byte_slice(pos as usize..start as usize),
                injection_stack
            );
            start = pos;
        }
        if !injection_stack.is_empty() {
            let range = start..pos.min(line_end);
            if !range.is_empty() {
                line_labels.push((range, injection_stack.clone()))
            }
        }

        if start != pos {
            while pos >= line_end {
                res.push_str(&ident);
                res.extend(
                    src.byte_slice(line_start as usize..line_end as usize)
                        .chunks(),
                );
                annotate_line(
                    comment_prefix,
                    src,
                    line_start,
                    &mut line_labels,
                    &mut res,
                    !injection_stack.is_empty() && pos > line_end,
                );
                line_labels.clear();
                line_idx += 1;
                line_start = line_end;
                line_end = src
                    .try_line_to_byte(line_idx + 1)
                    .unwrap_or(src.len_bytes()) as u32;
                if line_start == line_end {
                    break;
                }
                if pos > line_start && !injection_stack.is_empty() {
                    line_labels.push((line_start..pos.min(line_end), Vec::new()))
                }
            }
        }

        match event {
            QueryIterEvent::EnterInjection(injection) => {
                injection_stack.push(get_language_name(syntax.layer(injection.layer).language));
            }
            QueryIterEvent::ExitInjection { .. } => {
                injection_stack.pop();
            }
            QueryIterEvent::Match(_) => unreachable!(),
        }
    }
    if !line_labels.is_empty() {
        res.push_str(&ident);
        res.extend(
            src.byte_slice(line_start as usize..line_end as usize)
                .chunks(),
        );
        if !res.ends_with('\n') {
            res.push('\n');
        }
        annotate_line(
            comment_prefix,
            src,
            line_start,
            &mut line_labels,
            &mut res,
            false,
        );
        line_start = line_end;
    }
    for line in src.byte_slice(line_start as usize..).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{ident}{line}")
        }
    }
    res
}

fn annotate_line(
    comment_prefix: &str,
    src: RopeSlice<'_>,
    line_start: u32,
    annotations: &mut Vec<(Range, Vec<String>)>,
    dst: &mut String,
    continued: bool,
) {
    if annotations.is_empty() {
        return;
    }
    annotations.dedup_by(|(src_range, src_scopes), (dst_range, dst_scopes)| {
        if dst_scopes == src_scopes && dst_range.end == src_range.start {
            dst_range.end = src_range.end;
            true
        } else {
            false
        }
    });
    w!(dst, "{comment_prefix}");
    let mut prev_pos = line_start;
    let mut offsets = Vec::with_capacity(annotations.len());
    for (i, (range, labels)) in annotations.iter().enumerate() {
        let offset = src
            .byte_slice(prev_pos as usize..range.start as usize)
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        let mut width: usize = src
            .byte_slice(range.start as usize..range.end as usize)
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        width = width.saturating_sub(1);
        offsets.push((offset, width));
        let first_char = if labels.is_empty() {
            "━"
        } else if width == 0 {
            if i == annotations.len() - 1 {
                "╰"
            } else {
                "╿"
            }
        } else if i == annotations.len() - 1 {
            "┗"
        } else {
            "┡"
        };
        let last_char = if i == annotations.len() - 1 && !labels.is_empty() {
            "┹"
        } else if continued && i == annotations.len() - 1 {
            "━"
        } else {
            "┛"
        };
        if width == 0 {
            w!(dst, "{0:^offset$}{first_char}", "");
        } else {
            width -= 1;
            w!(dst, "{0:^offset$}{first_char}{0:━^width$}{last_char}", "");
        }
        prev_pos = range.end;
    }
    let Some(i) = annotations
        .iter()
        .position(|(_, scopes)| !scopes.is_empty())
    else {
        wln!(dst);
        return;
    };
    let highlights = &annotations[i..];
    let offset: usize = offsets
        .drain(..i)
        .map(|(offset, width)| offset + width + 1)
        .sum();
    offsets[0].0 += offset;
    w!(dst, "─");
    for highlight in &highlights.last().unwrap().1 {
        w!(dst, " {highlight}")
    }
    wln!(dst);
    for depth in (0..highlights.len().saturating_sub(1)).rev() {
        w!(dst, "{comment_prefix}");
        for &(offset, width) in &offsets[..depth] {
            w!(dst, "{0:^offset$}│{0:^width$}", "");
        }
        let offset = offsets[depth].0;
        w!(dst, "{:>offset$}╰─", "");
        for highlight in &highlights[depth].1 {
            w!(dst, " {highlight}")
        }
        wln!(dst);
    }
}
