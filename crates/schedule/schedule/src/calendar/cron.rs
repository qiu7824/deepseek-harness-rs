//! Strict five-field Vixie cron, including the original DOM_STAR/DOW_STAR rules.

use super::{MAX_INSTANT_MS, MIN_INSTANT_MS, local_datetime, local_instant};
use crate::host_types::ScheduleError;
use chrono::{Datelike, Months, NaiveDate, NaiveTime};
use jiff::tz::TimeZone as CalendarZone;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
struct Field {
    values: Vec<u32>,
    canonical: String,
    star: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Cron {
    pub expression: String,
    fields: [Field; 5],
}

const SPECS: [(u32, u32, u32); 5] = [
    (0, 59, 59),
    (0, 23, 23),
    (1, 31, 31),
    (1, 12, 12),
    (0, 7, 6),
];
const MAX_SAFE: u64 = 9_007_199_254_740_991;

fn numeric(text: &str) -> Result<u64, ScheduleError> {
    if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
        return Err(ScheduleError::invalid(
            "Cron fields accept only integer values, ranges, stars, lists and steps.",
        ));
    }
    text.parse::<u64>()
        .ok()
        .filter(|v| *v <= MAX_SAFE)
        .ok_or_else(|| ScheduleError::invalid("Cron numbers must be safe integers."))
}

fn encode(values: &[u32]) -> String {
    if values.len() == 1 {
        return values[0].to_string();
    }
    let step = values[1] - values[0];
    if values.windows(2).all(|w| w[1] - w[0] == step) {
        return format!(
            "{}-{}{}",
            values[0],
            values[values.len() - 1],
            if step == 1 {
                String::new()
            } else {
                format!("/{step}")
            }
        );
    }
    let mut parts = Vec::new();
    let mut start = values[0];
    let mut last = start;
    for &value in &values[1..] {
        if value != last + 1 {
            parts.push(if start == last {
                start.to_string()
            } else {
                format!("{start}-{last}")
            });
            start = value;
        }
        last = value;
    }
    parts.push(if start == last {
        start.to_string()
    } else {
        format!("{start}-{last}")
    });
    parts.join(",")
}

fn walk(min: u32, max: u32, canonical_max: u32, step: u64) -> Vec<u32> {
    let mut out = BTreeSet::new();
    let mut n = u64::from(min);
    while n <= u64::from(max) {
        out.insert(if canonical_max != max && n == u64::from(max) {
            min
        } else {
            n as u32
        });
        n += step;
    }
    out.into_iter().collect()
}

fn field(raw: &str, spec: (u32, u32, u32)) -> Result<Field, ScheduleError> {
    let (min, max, canonical_max) = spec;
    let mut values = BTreeSet::new();
    for element in raw.split(',') {
        let mut split = element.split('/');
        let range = split.next().unwrap_or("");
        let step_text = split.next();
        if split.next().is_some() {
            return Err(ScheduleError::invalid(
                "Cron field contains multiple steps.",
            ));
        }
        let step = step_text.map(numeric).transpose()?.unwrap_or(1);
        if step == 0 {
            return Err(ScheduleError::invalid(
                "Cron steps must be positive integers.",
            ));
        }
        let (first, last) = if range == "*" {
            (min, max)
        } else {
            let mut endpoints = range.split('-');
            let first = numeric(endpoints.next().unwrap_or(""))?;
            let last = endpoints.next();
            if endpoints.next().is_some() || (last.is_none() && step_text.is_some()) {
                return Err(ScheduleError::invalid(
                    "Cron steps require a star or a range.",
                ));
            }
            let last = last.map(numeric).transpose()?.unwrap_or(first);
            if first < u64::from(min) || last > u64::from(max) || first > last {
                return Err(ScheduleError::invalid(
                    "Cron field value or range is out of bounds.",
                ));
            }
            (first as u32, last as u32)
        };
        let mut current = u64::from(first);
        while current <= u64::from(last) {
            values.insert(if canonical_max != max && current == u64::from(max) {
                min
            } else {
                current as u32
            });
            current += step;
        }
    }
    let values: Vec<_> = values.into_iter().collect();
    let star = raw.starts_with('*');
    let canonical = if !star {
        encode(&values)
    } else if values.len() == (canonical_max - min + 1) as usize {
        "*".into()
    } else {
        let mut best_step = 1;
        let mut best_walk = Vec::new();
        for step in 1..=(max - min + 1) {
            let walked = walk(min, max, canonical_max, u64::from(step));
            if walked.len() > best_walk.len() && walked.iter().all(|v| values.contains(v)) {
                best_step = step;
                best_walk = walked;
            }
        }
        let rest: Vec<_> = values
            .iter()
            .copied()
            .filter(|v| !best_walk.contains(v))
            .collect();
        if rest.is_empty() {
            format!("*/{best_step}")
        } else {
            format!("*/{best_step},{}", encode(&rest))
        }
    };
    Ok(Field {
        values,
        canonical,
        star,
    })
}

impl Cron {
    pub fn parse(expression: &str) -> Result<Self, ScheduleError> {
        if expression.is_empty() || super::trim_js(expression) != expression {
            return Err(ScheduleError::invalid(
                "cron.expression must be a non-empty trimmed string.",
            ));
        }
        let raw: Vec<_> = expression
            .split(super::is_js_whitespace)
            .filter(|part| !part.is_empty())
            .collect();
        if raw.len() != 5 {
            return Err(ScheduleError::invalid(
                "cron.expression requires exactly five fields: minute hour day-of-month month day-of-week.",
            ));
        }
        let parsed = raw
            .iter()
            .zip(SPECS)
            .map(|(raw, spec)| field(raw, spec))
            .collect::<Result<Vec<_>, _>>()?;
        let fields: [Field; 5] = parsed.try_into().expect("five parsed cron fields");
        Ok(Self {
            expression: fields
                .iter()
                .map(|f| f.canonical.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            fields,
        })
    }

    fn matches(&self, date: NaiveDate) -> bool {
        if !self.fields[3].values.contains(&date.month()) {
            return false;
        }
        let dom = self.fields[2].values.contains(&date.day());
        let dow = self.fields[4]
            .values
            .contains(&date.weekday().num_days_from_sunday());
        if !self.fields[2].star && !self.fields[4].star {
            dom || dow
        } else {
            dom && dow
        }
    }

    fn times(&self) -> Vec<NaiveTime> {
        self.fields[1]
            .values
            .iter()
            .flat_map(|hour| {
                self.fields[0].values.iter().map(move |minute| {
                    NaiveTime::from_hms_opt(*hour, *minute, 0).expect("valid cron clock")
                })
            })
            .collect()
    }

    pub fn next(&self, zone: &CalendarZone, floor: i64) -> Option<i64> {
        let local = local_datetime(floor, zone);
        let first = local.date();
        let mut date = first;
        let ceiling = NaiveDate::from_ymd_opt(10_000, 1, 1)?;
        let last = date.checked_add_months(Months::new(4800))?.min(ceiling);
        let times = self.times();
        while date <= last {
            if self.matches(date) {
                for time in &times {
                    if date == first && *time < local.time() {
                        continue;
                    }
                    let Some(target) = local_instant(date.and_time(*time), zone) else {
                        continue;
                    };
                    if target > MAX_INSTANT_MS {
                        return None;
                    }
                    if target >= MIN_INSTANT_MS && target > floor {
                        return Some(target);
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    pub fn latest(&self, zone: &CalendarZone, decision: i64, saved_target: i64) -> i64 {
        let mut date = local_datetime(decision, &CalendarZone::UTC)
            .date()
            .succ_opt()
            .expect("four-digit date successor");
        let first = date
            .checked_sub_months(Months::new(4800))
            .expect("bounded cron horizon")
            .max(local_datetime(MIN_INSTANT_MS, zone).date());
        let times = self.times();
        while date >= first {
            if self.matches(date) {
                let earliest = times
                    .iter()
                    .find_map(|time| local_instant(date.and_time(*time), zone));
                if earliest.is_none_or(|instant| instant <= decision) {
                    for time in times.iter().rev() {
                        let Some(candidate) = local_instant(date.and_time(*time), zone) else {
                            continue;
                        };
                        if (MIN_INSTANT_MS..=decision).contains(&candidate) {
                            return candidate.max(saved_target);
                        }
                    }
                }
            }
            let Some(previous) = date.pred_opt() else {
                break;
            };
            date = previous;
        }
        saved_target
    }
}
