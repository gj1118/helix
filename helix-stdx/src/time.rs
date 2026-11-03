use std::time::SystemTime;

const SECOND: i64 = 1;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// Formats a timestamp into a human-readable relative time string.
///
/// # Arguments
///
/// * `timestamp` - A point in history. Seconds since UNIX epoch (UTC)
/// * `_timezone_offset` - Timezone offset in seconds (does not change the difference)
///
/// # Returns
///
/// A String representing the relative time (e.g., "4 years ago", "11 months from now")
#[inline]
pub fn format_relative_time(timestamp: i64, _timezone_offset: i32) -> String {
    // The wall clock, read each time: a clock derived from the monotonic one once at
    // startup falls behind by the time the system was suspended.
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // The timezone offset would shift both times equally, so it does not matter.
    // Saturating: the timestamp comes from outside (e.g. git) and may be absurd.
    let time_passed = now.saturating_sub(timestamp);

    let time_difference = time_passed.saturating_abs();

    let (value, unit) = if time_difference >= YEAR {
        let years = time_difference / YEAR;
        (years, if years == 1 { "year" } else { "years" })
    } else if time_difference >= MONTH {
        let months = time_difference / MONTH;
        (months, if months == 1 { "month" } else { "months" })
    } else if time_difference >= DAY {
        let days = time_difference / DAY;
        (days, if days == 1 { "day" } else { "days" })
    } else if time_difference >= HOUR {
        let hours = time_difference / HOUR;
        (hours, if hours == 1 { "hour" } else { "hours" })
    } else if time_difference >= MINUTE {
        let minutes = time_difference / MINUTE;
        (minutes, if minutes == 1 { "minute" } else { "minutes" })
    } else {
        let seconds = time_difference / SECOND;
        (seconds, if seconds == 1 { "second" } else { "seconds" })
    };
    let value = value.to_string();

    let label = if time_passed.is_positive() {
        "ago"
    } else {
        "from now"
    };

    crate::str_concat!(value, " ", unit, " ", label)
}

#[cfg(test)]
mod test {
    use super::*;

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn relative_times() {
        assert_eq!(format_relative_time(now() - 2 * HOUR, 3600), "2 hours ago");
        assert_eq!(format_relative_time(now() - YEAR, -7200), "1 year ago");
        assert!(format_relative_time(now() + 3 * DAY + 60, 0).ends_with("days from now"));
        // absurd timestamps must not overflow
        assert!(format_relative_time(i64::MIN, i32::MIN).ends_with("ago"));
        assert!(format_relative_time(i64::MAX, i32::MAX).ends_with("from now"));
    }
}
