//! Recurrence rules for scheduled tasks and their next UTC occurrence.
//!
//! One-shot (`at`) and fixed-interval (`every`) rules are elapsed-time rules.
//! Daily, weekly and cron rules keep a local wall-clock time in an explicit
//! IANA zone, so a later offset change still resolves each local date: a
//! local time inside a DST gap is skipped for that date and an ambiguous
//! local time selects its earlier instant once.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Shortest accepted repeat interval.
pub const MIN_INTERVAL_SECONDS: u64 = 60;
/// Longest accepted fixed interval (one leap year).
pub const MAX_INTERVAL_SECONDS: u64 = 366 * 24 * 3600;

/// Durable recurrence rule of one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TaskRule {
    /// One delivery at an absolute instant (RFC 3339, stored in UTC).
    At { at: String },
    /// Fixed elapsed interval aligned to `anchor` (RFC 3339 UTC).
    Every {
        #[serde(rename = "everySeconds")]
        every_seconds: u64,
        /// Empty on input aligns the first run one interval after creation.
        #[serde(default)]
        anchor: String,
    },
    /// Every local date at `time` (`HH:MM` or `HH:MM:SS`).
    Daily {
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Local dates whose ISO weekday (1 = Monday … 7 = Sunday) is listed.
    Weekly {
        time: String,
        weekdays: Vec<u8>,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Five-field cron expression evaluated in `time_zone`.
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
}

impl TaskRule {
    pub fn kind(&self) -> &'static str {
        match self {
            TaskRule::At { .. } => "at",
            TaskRule::Every { .. } => "every",
            TaskRule::Daily { .. } => "daily",
            TaskRule::Weekly { .. } => "weekly",
            TaskRule::Cron { .. } => "cron",
        }
    }

    pub fn is_recurring(&self) -> bool {
        !matches!(self, TaskRule::At { .. })
    }
}

/// A rule the scheduler cannot represent, with a message fit for users and
/// the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub code: &'static str,
    pub message: String,
}

impl RuleError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// UTC timestamp in the stored millisecond RFC 3339 form.
pub fn format_instant(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub fn parse_instant(value: &str) -> Result<DateTime<Utc>, RuleError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| {
            RuleError::new(
                "invalid_time",
                "时间必须是带时区偏移的 RFC 3339 格式，例如 2026-09-27T09:00:00+08:00",
            )
        })
}

pub fn parse_time_zone(value: &str) -> Result<Tz, RuleError> {
    value.trim().parse::<Tz>().map_err(|_| {
        RuleError::new(
            "invalid_time_zone",
            "时区必须是 UTC 或 IANA 名称，例如 Asia/Shanghai",
        )
    })
}

fn parse_local_time(value: &str) -> Result<NaiveTime, RuleError> {
    NaiveTime::parse_from_str(value, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
        .map_err(|_| RuleError::new("invalid_time", "时间必须为 HH:MM 或 HH:MM:SS"))
}

fn format_local_time(value: NaiveTime) -> String {
    if value.format("%S").to_string() == "00" {
        value.format("%H:%M").to_string()
    } else {
        value.format("%H:%M:%S").to_string()
    }
}

/// Validate one rule and return its normalized stored form.
pub fn normalize(rule: TaskRule) -> Result<TaskRule, RuleError> {
    Ok(match rule {
        TaskRule::At { at } => TaskRule::At {
            at: format_instant(parse_instant(&at)?),
        },
        TaskRule::Every {
            every_seconds,
            anchor,
        } => {
            if every_seconds < MIN_INTERVAL_SECONDS {
                return Err(RuleError::new(
                    "frequency_too_high",
                    "重复间隔不能短于 1 分钟",
                ));
            }
            if every_seconds > MAX_INTERVAL_SECONDS {
                return Err(RuleError::new("invalid_rule", "重复间隔不能超过 366 天"));
            }
            TaskRule::Every {
                every_seconds,
                anchor: format_instant(parse_instant(&anchor)?),
            }
        }
        TaskRule::Daily { time, time_zone } => TaskRule::Daily {
            time: format_local_time(parse_local_time(&time)?),
            time_zone: parse_time_zone(&time_zone)?.name().to_string(),
        },
        TaskRule::Weekly {
            time,
            mut weekdays,
            time_zone,
        } => {
            weekdays.sort_unstable();
            weekdays.dedup();
            if weekdays.is_empty() || weekdays.iter().any(|day| !(1..=7).contains(day)) {
                return Err(RuleError::new(
                    "invalid_rule",
                    "每周规则至少选择一天，星期取 1（周一）到 7（周日）",
                ));
            }
            TaskRule::Weekly {
                time: format_local_time(parse_local_time(&time)?),
                weekdays,
                time_zone: parse_time_zone(&time_zone)?.name().to_string(),
            }
        }
        TaskRule::Cron {
            expression,
            time_zone,
        } => {
            let expression = expression.split_whitespace().collect::<Vec<_>>().join(" ");
            CronSpec::parse(&expression)?;
            TaskRule::Cron {
                expression,
                time_zone: parse_time_zone(&time_zone)?.name().to_string(),
            }
        }
    })
}

/// Earliest instant of one local date and time; None inside a DST gap.
fn resolve_local(zone: Tz, date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, second) => Some(first.min(second).with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// The first occurrence strictly after `after`, or None when the rule has
/// no representable later occurrence. `rule` must be normalized.
pub fn next_after(rule: &TaskRule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match rule {
        TaskRule::At { at } => parse_instant(at).ok().filter(|at| *at > after),
        TaskRule::Every {
            every_seconds,
            anchor,
        } => {
            let anchor = parse_instant(anchor).ok()?;
            if anchor > after {
                return Some(anchor);
            }
            let interval = i64::try_from(*every_seconds).ok()?.max(1);
            let elapsed = (after - anchor).num_seconds();
            let steps = elapsed / interval + 1;
            anchor.checked_add_signed(Duration::seconds(steps.checked_mul(interval)?))
        }
        TaskRule::Daily { time, time_zone } => {
            let zone = parse_time_zone(time_zone).ok()?;
            let time = parse_local_time(time).ok()?;
            let start = after.with_timezone(&zone).date_naive().pred_opt()?;
            (0..400)
                .filter_map(|offset| start.checked_add_signed(Duration::days(offset)))
                .filter_map(|date| resolve_local(zone, date, time))
                .find(|candidate| *candidate > after)
        }
        TaskRule::Weekly {
            time,
            weekdays,
            time_zone,
        } => {
            let zone = parse_time_zone(time_zone).ok()?;
            let time = parse_local_time(time).ok()?;
            let start = after.with_timezone(&zone).date_naive().pred_opt()?;
            (0..400)
                .filter_map(|offset| start.checked_add_signed(Duration::days(offset)))
                .filter(|date| weekdays.contains(&(date.weekday().number_from_monday() as u8)))
                .filter_map(|date| resolve_local(zone, date, time))
                .find(|candidate| *candidate > after)
        }
        TaskRule::Cron {
            expression,
            time_zone,
        } => {
            let zone = parse_time_zone(time_zone).ok()?;
            CronSpec::parse(expression).ok()?.next_after(zone, after)
        }
    }
}

/// Parsed five-field cron expression (minute hour day-of-month month
/// day-of-week) with Vixie semantics: when both day fields are restricted,
/// either may match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSpec {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    weekdays: Vec<bool>,
    days_any: bool,
    weekdays_any: bool,
}

const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

impl CronSpec {
    pub fn parse(expression: &str) -> Result<Self, RuleError> {
        let expanded = match expression.trim() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(RuleError::new(
                "invalid_rule",
                "Cron 表达式需要 5 个字段：分 时 日 月 周",
            ));
        }
        let (minutes, _) = parse_field(fields[0], 0, 59, &[])?;
        let (hours, _) = parse_field(fields[1], 0, 23, &[])?;
        let (days, days_any) = parse_field(fields[2], 1, 31, &[])?;
        let (months, _) = parse_field(fields[3], 1, 12, &MONTH_NAMES)?;
        let (mut weekdays, weekdays_any) = parse_field(fields[4], 0, 7, &WEEKDAY_NAMES)?;
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Ok(Self {
            minutes,
            hours,
            days,
            months,
            weekdays,
            days_any,
            weekdays_any,
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let day = self.days[date.day() as usize];
        let weekday = self.weekdays[date.weekday().num_days_from_sunday() as usize];
        match (self.days_any, self.weekdays_any) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            (false, false) => day || weekday,
        }
    }

    fn next_after(&self, zone: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let start = after.with_timezone(&zone).date_naive().pred_opt()?;
        for offset in 0..(366 * 5) {
            let date = start.checked_add_signed(Duration::days(offset))?;
            if !self.months[date.month() as usize] || !self.day_matches(date) {
                continue;
            }
            for hour in (0..24).filter(|hour| self.hours[*hour]) {
                for minute in (0..60).filter(|minute| self.minutes[*minute]) {
                    let time = NaiveTime::from_hms_opt(hour as u32, minute as u32, 0)?;
                    if let Some(candidate) = resolve_local(zone, date, time)
                        && candidate > after
                    {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }
}

/// One cron field as a membership table indexed by value, plus whether it
/// is unrestricted (`*`).
fn parse_field(
    field: &str,
    min: usize,
    max: usize,
    names: &[&str],
) -> Result<(Vec<bool>, bool), RuleError> {
    let invalid = || RuleError::new("invalid_rule", format!("Cron 字段无效：{field}"));
    let value = |text: &str| -> Result<usize, RuleError> {
        let upper = text.to_ascii_uppercase();
        if let Some(index) = names.iter().position(|name| *name == upper) {
            return Ok(index + if min == 1 { 1 } else { 0 });
        }
        let parsed: usize = text.parse().map_err(|_| invalid())?;
        if parsed < min || parsed > max {
            return Err(invalid());
        }
        Ok(parsed)
    };
    let mut table = vec![false; max + 1];
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: usize = step.parse().map_err(|_| invalid())?;
                if step == 0 {
                    return Err(invalid());
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (value(start)?, value(end)?)
        } else if part.contains('/') {
            (value(range)?, max)
        } else {
            let single = value(range)?;
            (single, single)
        };
        if start > end {
            return Err(invalid());
        }
        for index in (start..=end).step_by(step) {
            table[index] = true;
        }
    }
    Ok((table, field == "*"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        parse_instant(value).unwrap()
    }

    #[test]
    fn one_shot_and_interval_rules() {
        let rule = normalize(TaskRule::At {
            at: "2026-09-27T09:00:00+08:00".into(),
        })
        .unwrap();
        assert_eq!(
            rule,
            TaskRule::At {
                at: "2026-09-27T01:00:00.000Z".into()
            }
        );
        assert_eq!(
            next_after(&rule, at("2026-09-27T00:00:00Z")),
            Some(at("2026-09-27T01:00:00Z"))
        );
        assert_eq!(next_after(&rule, at("2026-09-27T01:00:00Z")), None);

        let every = normalize(TaskRule::Every {
            every_seconds: 3600,
            anchor: "2026-09-26T10:00:00Z".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&every, at("2026-09-26T09:00:00Z")),
            Some(at("2026-09-26T10:00:00Z"))
        );
        assert_eq!(
            next_after(&every, at("2026-09-26T10:00:00Z")),
            Some(at("2026-09-26T11:00:00Z"))
        );
        assert_eq!(
            next_after(&every, at("2026-09-26T13:30:00Z")),
            Some(at("2026-09-26T14:00:00Z"))
        );
        assert_eq!(
            normalize(TaskRule::Every {
                every_seconds: 30,
                anchor: "2026-09-26T10:00:00Z".into()
            })
            .unwrap_err()
            .code,
            "frequency_too_high"
        );
    }

    #[test]
    fn daily_rules_follow_local_wall_clock_across_dst() {
        let rule = normalize(TaskRule::Daily {
            time: "09:00".into(),
            time_zone: "Asia/Shanghai".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&rule, at("2026-09-26T00:30:00Z")),
            Some(at("2026-09-26T01:00:00Z"))
        );
        assert_eq!(
            next_after(&rule, at("2026-09-26T01:00:00Z")),
            Some(at("2026-09-27T01:00:00Z"))
        );

        // 02:30 does not exist on 2026-03-08 in New York; that date is skipped.
        let gap = normalize(TaskRule::Daily {
            time: "02:30".into(),
            time_zone: "America/New_York".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&gap, at("2026-03-08T05:00:00Z")),
            Some(at("2026-03-09T06:30:00Z"))
        );
        // 01:30 happens twice on 2026-11-01; the earlier instant is used once.
        let overlap = normalize(TaskRule::Daily {
            time: "01:30".into(),
            time_zone: "America/New_York".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&overlap, at("2026-11-01T04:00:00Z")),
            Some(at("2026-11-01T05:30:00Z"))
        );
        assert_eq!(
            next_after(&overlap, at("2026-11-01T05:30:00Z")),
            Some(at("2026-11-02T06:30:00Z"))
        );
    }

    #[test]
    fn weekly_rules_select_listed_weekdays() {
        let rule = normalize(TaskRule::Weekly {
            time: "18:30".into(),
            weekdays: vec![5, 1, 5],
            time_zone: "Asia/Shanghai".into(),
        })
        .unwrap();
        assert_eq!(
            rule,
            TaskRule::Weekly {
                time: "18:30".into(),
                weekdays: vec![1, 5],
                time_zone: "Asia/Shanghai".into()
            }
        );
        // 2026-09-26 is a Saturday; the next listed day is Monday 09-28.
        assert_eq!(
            next_after(&rule, at("2026-09-26T12:00:00Z")),
            Some(at("2026-09-28T10:30:00Z"))
        );
        assert!(
            normalize(TaskRule::Weekly {
                time: "09:00".into(),
                weekdays: vec![],
                time_zone: "UTC".into()
            })
            .is_err()
        );
        assert!(
            normalize(TaskRule::Weekly {
                time: "09:00".into(),
                weekdays: vec![8],
                time_zone: "UTC".into()
            })
            .is_err()
        );
    }

    #[test]
    fn cron_rules_cover_steps_ranges_names_and_either_day_field() {
        let workdays = TaskRule::Cron {
            expression: "0 9 * * MON-FRI".into(),
            time_zone: "Asia/Shanghai".into(),
        };
        let workdays = normalize(workdays).unwrap();
        assert_eq!(
            next_after(&workdays, at("2026-09-26T02:00:00Z")),
            Some(at("2026-09-28T01:00:00Z"))
        );
        let quarter = normalize(TaskRule::Cron {
            expression: "*/15 * * * *".into(),
            time_zone: "UTC".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&quarter, at("2026-09-26T10:07:00Z")),
            Some(at("2026-09-26T10:15:00Z"))
        );
        // Day 1 or any Sunday, whichever comes first.
        let either = normalize(TaskRule::Cron {
            expression: "0 0 1 * 0".into(),
            time_zone: "UTC".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&either, at("2026-09-26T00:00:00Z")),
            Some(at("2026-09-27T00:00:00Z"))
        );
        let yearly = normalize(TaskRule::Cron {
            expression: "@yearly".into(),
            time_zone: "UTC".into(),
        })
        .unwrap();
        assert_eq!(
            next_after(&yearly, at("2026-09-26T00:00:00Z")),
            Some(at("2027-01-01T00:00:00Z"))
        );
        for bad in [
            "* * * *",
            "60 * * * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
        ] {
            assert!(
                normalize(TaskRule::Cron {
                    expression: bad.into(),
                    time_zone: "UTC".into()
                })
                .is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn zones_and_times_are_validated() {
        assert!(
            normalize(TaskRule::Daily {
                time: "25:00".into(),
                time_zone: "UTC".into()
            })
            .is_err()
        );
        assert!(
            normalize(TaskRule::Daily {
                time: "09:00".into(),
                time_zone: "Mars/Base".into()
            })
            .is_err()
        );
        assert!(
            normalize(TaskRule::At {
                at: "2026-09-27 09:00".into()
            })
            .is_err()
        );
    }
}
