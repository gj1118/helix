//! Resolution of snippet variables (`$TM_FILENAME`, `${CURRENT_YEAR}`, `$UUID`, ...).
//!
//! The renderer asks for a variable once per cursor, so everything that can differ between
//! cursors (selected text, line, word, cursor index) is captured per cursor in [`VariableEnv`]
//! up front. The values that must be stable within one insertion (the current time, random
//! values) are computed once per [`SnippetRenderCtx`].

use std::borrow::Cow;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use helix_core::chars::char_is_word;
use helix_core::snippets::{SnippetRenderCtx, VariableResolver};

use crate::document::Mode;
use crate::{Document, ViewId};

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// A broken-down point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    /// 1-12
    pub month: u32,
    /// 1-31
    pub day: u32,
    /// 0 is Sunday
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
    pub unix_seconds: i64,
    /// Offset from UTC in seconds, east positive.
    pub offset_seconds: i32,
    pub timezone_name: String,
}

impl DateTime {
    /// The current time in the local timezone. Falls back to UTC where the local timezone can't
    /// be determined.
    pub fn now() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = since_epoch.as_secs() as i64;
        let millis = since_epoch.subsec_millis();
        Self::local(secs, millis).unwrap_or_else(|| Self::utc(secs, millis))
    }

    #[cfg(unix)]
    fn local(secs: i64, millis: u32) -> Option<Self> {
        let time = secs as libc::time_t;
        // SAFETY: `tm` is plain old data, `localtime_r` only writes to the `tm` we pass and the
        // `tm_zone` pointer it fills in stays valid for as long as the timezone database isn't
        // reloaded, which we guard against by copying the string out immediately.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&time, &mut tm).is_null() {
                return None;
            }
            let timezone_name = if tm.tm_zone.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(tm.tm_zone)
                    .to_string_lossy()
                    .into_owned()
            };
            Some(DateTime {
                year: tm.tm_year as i64 + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                weekday: tm.tm_wday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
                second: tm.tm_sec as u32,
                millisecond: millis,
                unix_seconds: secs,
                offset_seconds: tm.tm_gmtoff as i32,
                timezone_name,
            })
        }
    }

    #[cfg(not(unix))]
    fn local(_secs: i64, _millis: u32) -> Option<Self> {
        None
    }

    /// Break down a unix timestamp in UTC.
    pub fn utc(secs: i64, millisecond: u32) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        // Civil-from-days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        DateTime {
            year,
            month,
            day,
            // 1970-01-01 was a Thursday.
            weekday: (days + 4).rem_euclid(7) as u32,
            hour: (rem / 3_600) as u32,
            minute: (rem % 3_600 / 60) as u32,
            second: (rem % 60) as u32,
            millisecond,
            unix_seconds: secs,
            offset_seconds: 0,
            timezone_name: "UTC".into(),
        }
    }

    fn variable(&self, name: &str) -> Option<String> {
        Some(match name {
            "CURRENT_YEAR" => self.year.to_string(),
            "CURRENT_YEAR_SHORT" => format!("{:02}", self.year.rem_euclid(100)),
            "CURRENT_MONTH" => format!("{:02}", self.month),
            "CURRENT_MONTH_NAME" => MONTH_NAMES[self.month as usize - 1].to_string(),
            "CURRENT_MONTH_NAME_SHORT" => MONTH_NAMES[self.month as usize - 1][..3].to_string(),
            "CURRENT_DATE" => format!("{:02}", self.day),
            "CURRENT_DAY_NAME" => DAY_NAMES[self.weekday as usize].to_string(),
            "CURRENT_DAY_NAME_SHORT" => DAY_NAMES[self.weekday as usize][..3].to_string(),
            "CURRENT_HOUR" => format!("{:02}", self.hour),
            "CURRENT_MINUTE" => format!("{:02}", self.minute),
            "CURRENT_SECOND" => format!("{:02}", self.second),
            "CURRENT_MILLISECOND" => format!("{:03}", self.millisecond),
            "CURRENT_SECONDS_UNIX" => self.unix_seconds.to_string(),
            "CURRENT_MILLISECONDS_UNIX" => {
                (self.unix_seconds * 1000 + i64::from(self.millisecond)).to_string()
            }
            "CURRENT_TIMEZONE_OFFSET" => {
                let sign = if self.offset_seconds < 0 { '-' } else { '+' };
                let abs = self.offset_seconds.unsigned_abs();
                format!("{sign}{:02}:{:02}", abs / 3_600, abs % 3_600 / 60)
            }
            "CURRENT_TIMEZONE_NAME" => self.timezone_name.clone(),
            _ => return None,
        })
    }
}

/// A 64 bit random number. This is only used for placeholder values (random numbers and UUIDs in
/// snippets), never for anything security sensitive.
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // `RandomState` is seeded randomly by the OS for every new instance.
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    hasher.finish()
}

fn random_digits() -> String {
    format!("{:06}", random_u64() % 1_000_000)
}

fn random_hex() -> String {
    format!("{:06x}", random_u64() % 0x100_0000)
}

fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&random_u64().to_le_bytes());
    bytes[8..].copy_from_slice(&random_u64().to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Values of the variables that differ for every cursor.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CursorVars {
    /// The selected text, empty when nothing is selected.
    pub selected: String,
    /// The text of the line the cursor is on, without the line ending.
    pub line: String,
    /// The word around the cursor, empty if the cursor isn't on a word.
    pub word: String,
    /// Zero-based line index of the cursor.
    pub line_index: usize,
}

/// Everything the snippet variables are resolved from.
#[derive(Debug, Default, Clone)]
pub struct VariableEnv {
    pub cursors: Vec<CursorVars>,
    pub file_path: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub clipboard: Option<String>,
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
}

fn non_empty(text: String) -> Option<Cow<'static, str>> {
    (!text.is_empty()).then_some(Cow::Owned(text))
}

fn path_string(path: &Path) -> Cow<'static, str> {
    Cow::Owned(path.to_string_lossy().into_owned())
}

impl VariableEnv {
    /// Capture the environment of `doc` as seen from `view_id`.
    ///
    /// A range counts as a selection when it is longer than one character, or, in select mode,
    /// when it covers at least one character. A single character in normal mode is just the
    /// cursor.
    pub fn from_document(
        doc: &Document,
        view_id: ViewId,
        mode: Mode,
        clipboard: Option<String>,
    ) -> Self {
        let text = doc.text().slice(..);
        let cursors = doc
            .selection(view_id)
            .iter()
            .map(|range| {
                let len = range.len();
                let selected = if len > 1 || (mode == Mode::Select && len >= 1) {
                    range.slice(text).to_string()
                } else {
                    String::new()
                };
                let cursor = range.cursor(text);
                let line_index = text.char_to_line(cursor);
                let line = text
                    .line(line_index)
                    .to_string()
                    .trim_end_matches(['\n', '\r'])
                    .to_owned();
                let mut start = cursor;
                while start > 0 && char_is_word(text.char(start - 1)) {
                    start -= 1;
                }
                let mut end = cursor;
                while end < text.len_chars() && char_is_word(text.char(end)) {
                    end += 1;
                }
                CursorVars {
                    selected,
                    line,
                    word: text.slice(start..end).to_string(),
                    line_index,
                }
            })
            .collect();

        let language = doc.language_config();
        let line_comment = language
            .and_then(|config| config.comment_tokens.as_ref())
            .and_then(|tokens| tokens.first().cloned());
        let block_comment = language
            .and_then(|config| config.block_comment_tokens.as_ref())
            .and_then(|tokens| tokens.first())
            .map(|token| (token.start.clone(), token.end.clone()));

        VariableEnv {
            cursors,
            file_path: doc.path().map(helix_stdx::path::canonicalize),
            workspace: Some(helix_loader::find_workspace().0),
            clipboard,
            line_comment,
            block_comment,
        }
    }

    fn file_variable(&self, name: &str) -> Option<Cow<'static, str>> {
        let path = self.file_path.as_deref()?;
        match name {
            "TM_FILENAME" => Some(path.file_name()?.to_string_lossy().into_owned().into()),
            "TM_FILENAME_BASE" => Some(path.file_stem()?.to_string_lossy().into_owned().into()),
            "TM_DIRECTORY" => Some(path_string(path.parent()?)),
            "TM_FILEPATH" => Some(path_string(path)),
            "RELATIVE_FILEPATH" => {
                let workspace = self.workspace.as_deref()?;
                Some(path_string(path.strip_prefix(workspace).unwrap_or(path)))
            }
            _ => None,
        }
    }

    /// Turn this environment into a variable resolver. Time is sampled once here so all
    /// references and all cursors see the same instant.
    pub fn into_resolver(self) -> Box<VariableResolver> {
        self.into_resolver_at(DateTime::now())
    }

    fn into_resolver_at(self, now: DateTime) -> Box<VariableResolver> {
        // Random values are cached per cursor and name so that `$UUID ... $UUID` agree
        // within a cursor but every cursor gets its own value.
        let mut random: HashMap<(usize, String), String> = HashMap::new();
        Box::new(move |name: &str, cursor: usize| {
            let cursor_vars = self.cursors.get(cursor);
            match name {
                "TM_SELECTED_TEXT" | "SELECTION" => non_empty(cursor_vars?.selected.clone()),
                "TM_CURRENT_LINE" => Some(Cow::Owned(cursor_vars?.line.clone())),
                "TM_CURRENT_WORD" => non_empty(cursor_vars?.word.clone()),
                "TM_LINE_INDEX" => Some(cursor_vars?.line_index.to_string().into()),
                "TM_LINE_NUMBER" => Some((cursor_vars?.line_index + 1).to_string().into()),
                "CURSOR_INDEX" => Some(cursor.to_string().into()),
                "CURSOR_NUMBER" => Some((cursor + 1).to_string().into()),
                "TM_FILENAME" | "TM_FILENAME_BASE" | "TM_DIRECTORY" | "TM_FILEPATH"
                | "RELATIVE_FILEPATH" => self.file_variable(name),
                "WORKSPACE_NAME" => Some(
                    self.workspace
                        .as_deref()?
                        .file_name()?
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                ),
                "WORKSPACE_FOLDER" => Some(path_string(self.workspace.as_deref()?)),
                "CLIPBOARD" => non_empty(self.clipboard.clone()?),
                "LINE_COMMENT" => self.line_comment.clone().map(Cow::Owned),
                "BLOCK_COMMENT_START" => self.block_comment.as_ref().map(|(s, _)| s.clone().into()),
                "BLOCK_COMMENT_END" => self.block_comment.as_ref().map(|(_, e)| e.clone().into()),
                "RANDOM" | "RANDOM_HEX" | "UUID" => {
                    let value =
                        random
                            .entry((cursor, name.to_owned()))
                            .or_insert_with(|| match name {
                                "RANDOM" => random_digits(),
                                "RANDOM_HEX" => random_hex(),
                                _ => uuid_v4(),
                            });
                    Some(Cow::Owned(value.clone()))
                }
                _ => now.variable(name).map(Cow::Owned),
            }
        })
    }
}

/// Build the render context used to insert snippets into `doc`.
pub fn render_ctx(
    doc: &Document,
    view_id: ViewId,
    mode: Mode,
    clipboard: Option<String>,
) -> SnippetRenderCtx {
    SnippetRenderCtx {
        resolve_var: VariableEnv::from_document(doc, view_id, mode, clipboard).into_resolver(),
        tab_width: doc.tab_width(),
        indent_style: doc.indent_style,
        line_ending: doc.line_ending.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(env: VariableEnv) -> Box<VariableResolver> {
        // 2024-02-29 13:05:09.042 UTC, a Thursday in a leap year.
        let mut now = DateTime::utc(1_709_211_909, 42);
        now.offset_seconds = -(5 * 3600 + 30 * 60);
        now.timezone_name = "TEST".into();
        env.into_resolver_at(now)
    }

    #[test]
    fn utc_breakdown() {
        let t = DateTime::utc(1_709_211_909, 42);
        assert_eq!(
            (t.year, t.month, t.day, t.weekday),
            (2024, 2, 29, 4),
            "{t:?}"
        );
        assert_eq!((t.hour, t.minute, t.second), (13, 5, 9));
        let epoch = DateTime::utc(0, 0);
        assert_eq!(
            (epoch.year, epoch.month, epoch.day, epoch.weekday),
            (1970, 1, 1, 4)
        );
        let before = DateTime::utc(-1, 0);
        assert_eq!(
            (
                before.year,
                before.month,
                before.day,
                before.hour,
                before.second
            ),
            (1969, 12, 31, 23, 59)
        );
        // End of a century year that is not a leap year.
        let t = DateTime::utc(4_107_542_400, 0);
        assert_eq!((t.year, t.month, t.day), (2100, 3, 1));
    }

    #[test]
    fn date_variables() {
        let mut resolve = resolver(VariableEnv::default());
        let mut get = |name: &str| resolve(name, 0).map(|v| v.into_owned());
        assert_eq!(get("CURRENT_YEAR").as_deref(), Some("2024"));
        assert_eq!(get("CURRENT_YEAR_SHORT").as_deref(), Some("24"));
        assert_eq!(get("CURRENT_MONTH").as_deref(), Some("02"));
        assert_eq!(get("CURRENT_MONTH_NAME").as_deref(), Some("February"));
        assert_eq!(get("CURRENT_MONTH_NAME_SHORT").as_deref(), Some("Feb"));
        assert_eq!(get("CURRENT_DATE").as_deref(), Some("29"));
        assert_eq!(get("CURRENT_DAY_NAME").as_deref(), Some("Thursday"));
        assert_eq!(get("CURRENT_DAY_NAME_SHORT").as_deref(), Some("Thu"));
        assert_eq!(get("CURRENT_HOUR").as_deref(), Some("13"));
        assert_eq!(get("CURRENT_MINUTE").as_deref(), Some("05"));
        assert_eq!(get("CURRENT_SECOND").as_deref(), Some("09"));
        assert_eq!(get("CURRENT_MILLISECOND").as_deref(), Some("042"));
        assert_eq!(get("CURRENT_SECONDS_UNIX").as_deref(), Some("1709211909"));
        assert_eq!(
            get("CURRENT_MILLISECONDS_UNIX").as_deref(),
            Some("1709211909042")
        );
        assert_eq!(get("CURRENT_TIMEZONE_OFFSET").as_deref(), Some("-05:30"));
        assert_eq!(get("CURRENT_TIMEZONE_NAME").as_deref(), Some("TEST"));
        assert_eq!(get("NOT_A_VARIABLE"), None);
    }

    #[test]
    fn cursor_variables_differ_per_cursor() {
        let env = VariableEnv {
            cursors: vec![
                CursorVars {
                    selected: "foo".into(),
                    line: "let foo = 1;".into(),
                    word: "foo".into(),
                    line_index: 0,
                },
                CursorVars {
                    selected: String::new(),
                    line: "bar".into(),
                    word: String::new(),
                    line_index: 4,
                },
            ],
            ..Default::default()
        };
        let mut resolve = resolver(env);
        assert_eq!(resolve("TM_SELECTED_TEXT", 0).as_deref(), Some("foo"));
        assert_eq!(resolve("SELECTION", 0).as_deref(), Some("foo"));
        // Nothing selected: unresolved so that a default can apply.
        assert_eq!(resolve("TM_SELECTED_TEXT", 1), None);
        assert_eq!(resolve("TM_CURRENT_WORD", 1), None);
        assert_eq!(resolve("TM_CURRENT_LINE", 1).as_deref(), Some("bar"));
        assert_eq!(resolve("TM_LINE_INDEX", 1).as_deref(), Some("4"));
        assert_eq!(resolve("TM_LINE_NUMBER", 1).as_deref(), Some("5"));
        assert_eq!(resolve("CURSOR_INDEX", 1).as_deref(), Some("1"));
        assert_eq!(resolve("CURSOR_NUMBER", 1).as_deref(), Some("2"));
    }

    #[test]
    fn file_and_workspace_variables() {
        let env = VariableEnv {
            file_path: Some("/work/proj/src/main.test.rs".into()),
            workspace: Some("/work/proj".into()),
            line_comment: Some("//".into()),
            block_comment: Some(("/*".into(), "*/".into())),
            clipboard: Some("clip".into()),
            ..Default::default()
        };
        let mut resolve = resolver(env);
        assert_eq!(resolve("TM_FILENAME", 0).as_deref(), Some("main.test.rs"));
        assert_eq!(resolve("TM_FILENAME_BASE", 0).as_deref(), Some("main.test"));
        assert_eq!(
            resolve("TM_DIRECTORY", 0).as_deref(),
            Some("/work/proj/src")
        );
        assert_eq!(
            resolve("TM_FILEPATH", 0).as_deref(),
            Some("/work/proj/src/main.test.rs")
        );
        assert_eq!(
            resolve("RELATIVE_FILEPATH", 0).as_deref(),
            Some("src/main.test.rs")
        );
        assert_eq!(resolve("WORKSPACE_NAME", 0).as_deref(), Some("proj"));
        assert_eq!(
            resolve("WORKSPACE_FOLDER", 0).as_deref(),
            Some("/work/proj")
        );
        assert_eq!(resolve("LINE_COMMENT", 0).as_deref(), Some("//"));
        assert_eq!(resolve("BLOCK_COMMENT_START", 0).as_deref(), Some("/*"));
        assert_eq!(resolve("BLOCK_COMMENT_END", 0).as_deref(), Some("*/"));
        assert_eq!(resolve("CLIPBOARD", 0).as_deref(), Some("clip"));

        // A buffer without a file has no file variables.
        let mut resolve = resolver(VariableEnv::default());
        assert_eq!(resolve("TM_FILENAME", 0), None);
        assert_eq!(resolve("CLIPBOARD", 0), None);
    }

    #[test]
    fn random_values() {
        let mut resolve = resolver(VariableEnv::default());
        let first = resolve("UUID", 0).unwrap().into_owned();
        assert_eq!(first, resolve("UUID", 0).unwrap(), "stable within a cursor");
        let other = resolve("UUID", 1).unwrap().into_owned();
        assert_ne!(first, other, "different per cursor");

        let bytes = first.as_bytes();
        assert_eq!(first.len(), 36);
        assert_eq!(
            [bytes[8], bytes[13], bytes[18], bytes[23]],
            [b'-'; 4],
            "{first}"
        );
        assert_eq!(bytes[14], b'4', "version nibble in {first}");
        assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'), "{first}");

        let random = resolve("RANDOM", 0).unwrap();
        assert!(random.len() == 6 && random.chars().all(|c| c.is_ascii_digit()));
        let hex = resolve("RANDOM_HEX", 0).unwrap();
        assert!(hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn local_time_is_plausible() {
        let now = DateTime::now();
        assert!((2024..3000).contains(&now.year));
        assert!((1..=12).contains(&now.month));
        assert!(now.hour < 24 && now.weekday < 7);
    }
}
