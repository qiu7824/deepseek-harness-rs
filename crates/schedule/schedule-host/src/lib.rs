//! Host-owned scheduled tasks.
//!
//! Tasks live in one Host JSON document independent of session activation.
//! The Host scheduler restores timers for active tasks at startup, delivers
//! a due task as an ordinary follow-up message into its original session
//! (restoring a cold session when needed), and records each delivery with
//! the instruction snapshot. Clients manage tasks through the service's
//! operations; the model uses the `scheduled_task_*` tools bound to its session.
//!
//! Rules: one-shot, fixed interval, daily and weekly local wall-clock times
//! in an explicit IANA zone, and five-field cron. Repeats are limited to
//! once a minute.

pub mod model;
pub mod rules;
pub mod service;
pub mod tools;

pub use model::*;
pub use rules::{TaskRule, next_after, normalize};
pub use service::{
    CreateTask, Deliver, HistoryPage, ScheduleService, ServiceConfig, UpdateTask, delivery_text,
};
pub use tools::{register_tools, rule_from_args};

/// The Host's IANA time zone, used as the default for new local-time rules.
pub fn host_time_zone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|zone| zone.parse::<chrono_tz::Tz>().is_ok())
        .unwrap_or_else(|| "UTC".to_string())
}

#[cfg(test)]
mod tests;
