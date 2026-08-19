//! Durable and model-facing Schedule value types. Rust port of
//! `packages/schedule/schedule/src/types.ts`.

use dsh_brand::Branded;
use serde::{Deserialize, Serialize};

/// The brand marker for [`ScheduleId`].
#[doc(hidden)]
pub enum ScheduleIdTag {}

/// Stable reminder identity that is unique and never reused within one
/// session (TS `ScheduleId`).
pub type ScheduleId = Branded<ScheduleIdTag>;

/// Brand a raw session-local id without changing its runtime value.
pub fn schedule_id(value: impl Into<String>) -> ScheduleId {
    ScheduleId::new(value)
}

/// The v1 durable reminder record union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleRecord {
    After {
        id: ScheduleId,
        prompt: String,
        #[serde(rename = "afterSeconds")]
        after_seconds: i64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    At {
        id: ScheduleId,
        prompt: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Every {
        id: ScheduleId,
        prompt: String,
        #[serde(rename = "everySeconds")]
        every_seconds: i64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
}

impl ScheduleRecord {
    pub fn id(&self) -> &ScheduleId {
        match self {
            ScheduleRecord::After { id, .. }
            | ScheduleRecord::At { id, .. }
            | ScheduleRecord::Every { id, .. } => id,
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            ScheduleRecord::After { prompt, .. }
            | ScheduleRecord::At { prompt, .. }
            | ScheduleRecord::Every { prompt, .. } => prompt,
        }
    }

    pub fn scheduled_at(&self) -> &str {
        match self {
            ScheduleRecord::After { scheduled_at, .. }
            | ScheduleRecord::At { scheduled_at, .. }
            | ScheduleRecord::Every { scheduled_at, .. } => scheduled_at,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ScheduleRecord::After { .. } => "after",
            ScheduleRecord::At { .. } => "at",
            ScheduleRecord::Every { .. } => "every",
        }
    }

    /// Whether this is a fixed-rate record.
    pub fn is_every(&self) -> bool {
        matches!(self, ScheduleRecord::Every { .. })
    }
}

/// Structured local-calendar input accepted by `schedule_create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAtInput {
    /// Four-digit ISO calendar date.
    pub date: String,
    /// Local wall-clock time with optional one-to-three digit milliseconds.
    pub time: String,
    /// Explicit UTC or IANA Area/Location zone.
    pub time_zone: String,
}

/// Absolute selector accepted by `schedule_create` (TS `AtInput`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AtInput {
    Instant(String),
    Local(LocalAtInput),
}

/// Strict version-1 durable Schedule mutation union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase")]
pub enum ScheduleChange {
    Create {
        version: u32,
        schedule: ScheduleRecord,
    },
    Delete {
        version: u32,
        id: ScheduleId,
    },
    Dispatch {
        version: u32,
        id: ScheduleId,
        /// Wall-clock decision time used to select the latest due occurrence.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "acceptedAt"
        )]
        accepted_at: Option<String>,
    },
}

/// Current delivery timing derived from the durable record and wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScheduleState {
    Scheduled,
    Overdue,
}

impl ScheduleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScheduleState::Scheduled => "scheduled",
            ScheduleState::Overdue => "overdue",
        }
    }
}

/// Complete model-facing view of one active reminder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleView {
    #[serde(flatten)]
    pub record: ScheduleRecord,
    /// Whether the target remains in the future.
    pub state: ScheduleState,
    /// Reminder delivery never leaves the owning session.
    #[serde(rename = "deliveryMode")]
    pub delivery_mode: String,
}

/// Management operations whose persistence barrier may be uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePersistenceOperation {
    Create,
    List,
    Delete,
}

impl SchedulePersistenceOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            SchedulePersistenceOperation::Create => "create",
            SchedulePersistenceOperation::List => "list",
            SchedulePersistenceOperation::Delete => "delete",
        }
    }
}

/// Closed v1 Schedule management error union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ScheduleToolError {
    InvalidPrompt {
        message: String,
    },
    InvalidSelector {
        message: String,
    },
    InvalidRule {
        message: String,
    },
    InvalidTimeZone {
        message: String,
    },
    NotFuture {
        message: String,
    },
    TimeOutOfRange {
        message: String,
    },
    FrequencyTooHigh {
        message: String,
    },
    CorruptScheduleLog {
        message: String,
    },
    PersistenceUncertain {
        message: String,
        operation: SchedulePersistenceOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<ScheduleId>,
    },
    InternalError {
        message: String,
    },
}

impl ScheduleToolError {
    pub fn code(&self) -> &'static str {
        match self {
            ScheduleToolError::InvalidPrompt { .. } => "invalid_prompt",
            ScheduleToolError::InvalidSelector { .. } => "invalid_selector",
            ScheduleToolError::InvalidRule { .. } => "invalid_rule",
            ScheduleToolError::InvalidTimeZone { .. } => "invalid_time_zone",
            ScheduleToolError::NotFuture { .. } => "not_future",
            ScheduleToolError::TimeOutOfRange { .. } => "time_out_of_range",
            ScheduleToolError::FrequencyTooHigh { .. } => "frequency_too_high",
            ScheduleToolError::CorruptScheduleLog { .. } => "corrupt_schedule_log",
            ScheduleToolError::PersistenceUncertain { .. } => "persistence_uncertain",
            ScheduleToolError::InternalError { .. } => "internal_error",
        }
    }
}

/// Canonical `schedule_create` value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleCreateValue {
    View(ScheduleView),
    Error(ScheduleToolError),
}

/// Canonical `schedule_list` value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleListValue {
    Views(Vec<ScheduleView>),
    Error(ScheduleToolError),
}

/// Successful `schedule_delete` value, including the non-mutating not-found
/// result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleDeleteValue {
    Deleted {
        id: ScheduleId,
        deleted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    Error(ScheduleToolError),
}
