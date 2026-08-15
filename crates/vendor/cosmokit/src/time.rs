//! Time constants, parsing, and formatting helpers (port of `src/time.ts`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use regex::Regex;

/// Milliseconds in one second.
pub const MILLISECOND: i64 = 1;
/// Milliseconds in one second.
pub const SECOND: i64 = 1000;
/// Milliseconds in one minute.
pub const MINUTE: i64 = SECOND * 60;
/// Milliseconds in one hour.
pub const HOUR: i64 = MINUTE * 60;
/// Milliseconds in one day.
pub const DAY: i64 = HOUR * 24;
/// Milliseconds in one week.
pub const WEEK: i64 = DAY * 7;

/// Module-level timezone offset (minutes to add to UTC for local time),
/// mirroring the TS mutable `timezoneOffset`. Negative east of UTC, matching
/// JS `Date#getTimezoneOffset()`.
static TIMEZONE_OFFSET: AtomicI32 = AtomicI32::new(i32::MIN);

fn default_timezone_offset() -> i32 {
    let seconds = Local::now().offset().local_minus_utc();
    -seconds / 60
}

/// Set the module timezone offset (TS `Time.setTimezoneOffset`).
pub fn set_timezone_offset(offset: i32) {
    TIMEZONE_OFFSET.store(offset, Ordering::Relaxed);
}

/// Get the module timezone offset (TS `Time.getTimezoneOffset`).
pub fn get_timezone_offset() -> i32 {
    let offset = TIMEZONE_OFFSET.load(Ordering::Relaxed);
    if offset == i32::MIN {
        let computed = default_timezone_offset();
        let _ = TIMEZONE_OFFSET.compare_exchange(
            i32::MIN,
            computed,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        computed
    } else {
        offset
    }
}

/// Day number for a timestamp under the given offset
/// (TS `Time.getDateNumber`, `date` in milliseconds since epoch).
///
/// Uses float division then a single `floor`, matching JS
/// `Math.floor((date.valueOf() / minute - offset) / 1440)`.
pub fn get_date_number(date_ms: i64, offset: Option<i32>) -> i64 {
    let offset = offset.unwrap_or_else(get_timezone_offset) as f64;
    (((date_ms as f64 / MINUTE as f64) - offset) / 1440.0).floor() as i64
}

/// Reconstruct a timestamp from a date number (TS `Time.fromDateNumber`).
pub fn from_date_number(value: i64, offset: Option<i32>) -> i64 {
    let offset = offset.unwrap_or_else(get_timezone_offset) as i64;
    value * DAY + offset * MINUTE
}

static TIME_REGEX: OnceLock<Regex> = OnceLock::new();

fn time_regex() -> &'static Regex {
    TIME_REGEX.get_or_init(|| {
        let numeric = r"\d+(?:\.\d+)?";
        let unit = |full: &str, _short: &str| format!(r"({numeric}{full})?");
        let pattern = format!(
            "^{}$",
            [
                unit("w(?:eek(?:s)?)?", "w"),
                unit("d(?:ay(?:s)?)?", "d"),
                unit("h(?:our(?:s)?)?", "h"),
                unit("m(?:in(?:ute)?(?:s)?)?", "m"),
                unit("s(?:ec(?:ond)?(?:s)?)?", "s"),
            ]
            .join("")
        );
        Regex::new(&pattern).expect("static time regex")
    })
}

/// JS `parseFloat` semantics: parse the leading numeric prefix, `NaN` when
/// there is none.
fn parse_float_prefix(source: &str) -> f64 {
    let end = source
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit() || **byte == b'.')
        .count();
    source[..end].parse::<f64>().unwrap_or(f64::NAN)
}

/// Parse a duration string like `1w2d3h4m5s` into milliseconds
/// (TS `Time.parseTime`; unmatched input yields 0).
pub fn parse_time(source: &str) -> i64 {
    let Some(captures) = time_regex().captures(source) else {
        return 0;
    };
    let number = |index: usize| -> f64 {
        captures
            .get(index)
            .map(|m| parse_float_prefix(m.as_str()))
            .unwrap_or(f64::NAN)
    };
    let ms = |value: f64, unit: i64| -> i64 {
        if value.is_nan() {
            0
        } else {
            (value * unit as f64) as i64
        }
    };
    ms(number(1), WEEK)
        + ms(number(2), DAY)
        + ms(number(3), HOUR)
        + ms(number(4), MINUTE)
        + ms(number(5), SECOND)
}

/// Parse a duration or date string into a UTC timestamp (TS `Time.parseDate`).
///
/// Deviation: JS `Date` parsing is implementation-defined; this port covers
/// the three documented branches (duration, `H:M(:S)`, `M-D-H:M(:S)` with the
/// current year) plus ISO-8601 fallback via chrono.
pub fn parse_date(date: &str) -> DateTime<Utc> {
    let parsed = parse_time(date);
    if parsed != 0 {
        return Utc::now() + chrono::Duration::milliseconds(parsed);
    }
    let time_only = Regex::new(r"^\d{1,2}(:\d{1,2}){1,2}$").expect("static regex");
    let date_time_no_year = Regex::new(r"^\d{1,2}-\d{1,2}-\d{1,2}(:\d{1,2}){1,2}$").expect("static regex");
    if time_only.is_match(date) {
        let now = Local::now();
        let parts: Vec<u32> = date.split(':').map(|p| p.parse().unwrap_or(0)).collect();
        let hour = parts[0];
        let minute = parts.get(1).copied().unwrap_or(0);
        let second = parts.get(2).copied().unwrap_or(0);
        let local = now
            .with_hour(hour)
            .and_then(|t| t.with_minute(minute))
            .and_then(|t| t.with_second(second))
            .unwrap_or(now);
        return local.with_timezone(&Utc);
    }
    if date_time_no_year.is_match(date) {
        let now = Local::now();
        let full = format!("{}-{}", now.year(), date);
        if let Ok(parsed) = DateTime::parse_from_str(&full, "%Y-%m-%d-%H:%M:%S")
            .or_else(|_| DateTime::parse_from_str(&full, "%Y-%m-%d-%H:%M"))
        {
            return parsed.with_timezone(&Utc);
        }
    }
    DateTime::parse_from_rfc3339(date)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Format a duration compactly (TS `Time.format`).
pub fn format(ms: i64) -> String {
    let abs = ms.abs();
    if abs >= DAY - HOUR / 2 {
        format!("{}d", (ms as f64 / DAY as f64).round() as i64)
    } else if abs >= HOUR - MINUTE / 2 {
        format!("{}h", (ms as f64 / HOUR as f64).round() as i64)
    } else if abs >= MINUTE - SECOND / 2 {
        format!("{}m", (ms as f64 / MINUTE as f64).round() as i64)
    } else if abs >= SECOND {
        format!("{}s", (ms as f64 / SECOND as f64).round() as i64)
    } else {
        format!("{ms}ms")
    }
}

/// Zero-pad a number to a fixed length (TS `Time.toDigits`).
pub fn to_digits(source: i64, length: usize) -> String {
    format!("{source:0length$}")
}

/// Fill a time template with `yyyy/yy/MM/dd/hh/mm/ss/SSS` fields
/// (TS `Time.template`, replacement order preserved).
pub fn template(template: &str, time: &DateTime<Local>) -> String {
    template
        .replace("yyyy", &time.year().to_string())
        .replace("yy", &(time.year() % 100).to_string())
        .replace("MM", &to_digits(time.month() as i64, 2))
        .replace("dd", &to_digits(time.day() as i64, 2))
        .replace("hh", &to_digits(time.hour() as i64, 2))
        .replace("mm", &to_digits(time.minute() as i64, 2))
        .replace("ss", &to_digits(time.second() as i64, 2))
        .replace("SSS", &to_digits(time.timestamp_subsec_millis() as i64, 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_durations() {
        assert_eq!(parse_time("10m"), 10 * MINUTE);
        assert_eq!(parse_time("1w2d3h4m5s"), WEEK + 2 * DAY + 3 * HOUR + 4 * MINUTE + 5 * SECOND);
        assert_eq!(parse_time("30s"), 30 * SECOND);
        assert_eq!(parse_time("1.5h"), (1.5 * HOUR as f64) as i64);
        assert_eq!(parse_time(""), 0);
        assert_eq!(parse_time("nope"), 0);
        assert_eq!(parse_time("10"), 0); // no unit → no match
    }

    #[test]
    fn formatting() {
        assert_eq!(format(0), "0ms");
        assert_eq!(format(500), "500ms");
        assert_eq!(format(1000), "1s");
        assert_eq!(format(90_000), "2m"); // rounds (90s >= 60s-0.5s)
        assert_eq!(format(3600_000), "1h");
        assert_eq!(format(86400_000), "1d");
        assert_eq!(format(-500), "-500ms");
        assert_eq!(to_digits(5, 2), "05");
        assert_eq!(to_digits(5, 3), "005");
    }

    #[test]
    fn date_numbers_round_trip() {
        // Use a fixed offset so the round trip is exact.
        let offset = 480; // UTC+8 expressed as JS-style minutes
        let now_ms = 1_752_000_000_000;
        let number = get_date_number(now_ms, Some(offset));
        let back = from_date_number(number, Some(offset));
        let number2 = get_date_number(back, Some(offset));
        assert_eq!(number, number2);
    }

    #[test]
    fn templates() {
        let time: DateTime<Local> = "2024-03-05T06:07:08.009+08:00".parse().unwrap();
        assert_eq!(template("yyyy-MM-dd hh:mm:ss.SSS", &time), "2024-03-05 06:07:08.009");
        assert_eq!(template("yy", &time), "24");
    }

    #[test]
    fn parse_dates() {
        let parsed = parse_date("2h");
        let lower = Utc::now() + chrono::Duration::hours(2) - chrono::Duration::minutes(1);
        let upper = Utc::now() + chrono::Duration::hours(2) + chrono::Duration::minutes(1);
        assert!(parsed >= lower && parsed <= upper, "2h should land near now+2h");
    }
}
