//! Durable Host reminders and their management protocol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum HostScheduleRecord {
    After {
        id: String,
        title: String,
        prompt: String,
        #[serde(rename = "afterSeconds")]
        after_seconds: i64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    At {
        id: String,
        title: String,
        prompt: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Every {
        id: String,
        title: String,
        prompt: String,
        #[serde(rename = "everySeconds")]
        every_seconds: i64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Daily {
        id: String,
        title: String,
        prompt: String,
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Weekly {
        id: String,
        title: String,
        prompt: String,
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        weekdays: Vec<u8>,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Cron {
        id: String,
        title: String,
        prompt: String,
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
}

macro_rules! record_field {
    ($this:ident, $name:ident) => {
        match $this {
            Self::After { $name, .. }
            | Self::At { $name, .. }
            | Self::Every { $name, .. }
            | Self::Daily { $name, .. }
            | Self::Weekly { $name, .. }
            | Self::Cron { $name, .. } => $name,
        }
    };
}

impl HostScheduleRecord {
    pub fn id(&self) -> &str {
        record_field!(self, id)
    }
    pub fn title(&self) -> &str {
        record_field!(self, title)
    }
    pub fn prompt(&self) -> &str {
        record_field!(self, prompt)
    }
    pub fn scheduled_at(&self) -> &str {
        record_field!(self, scheduled_at)
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::After { .. } => "after",
            Self::At { .. } => "at",
            Self::Every { .. } => "every",
            Self::Daily { .. } => "daily",
            Self::Weekly { .. } => "weekly",
            Self::Cron { .. } => "cron",
        }
    }
    pub fn is_recurring(&self) -> bool {
        !matches!(self, Self::After { .. } | Self::At { .. })
    }
    pub fn with_scheduled_at(&self, value: String) -> Self {
        let mut record = self.clone();
        match &mut record {
            Self::After { scheduled_at, .. }
            | Self::At { scheduled_at, .. }
            | Self::Every { scheduled_at, .. }
            | Self::Daily { scheduled_at, .. }
            | Self::Weekly { scheduled_at, .. }
            | Self::Cron { scheduled_at, .. } => *scheduled_at = value,
        }
        record
    }
    pub fn with_content(&self, new_title: String, new_prompt: String) -> Self {
        let mut record = self.clone();
        match &mut record {
            Self::After { title, prompt, .. }
            | Self::At { title, prompt, .. }
            | Self::Every { title, prompt, .. }
            | Self::Daily { title, prompt, .. }
            | Self::Weekly { title, prompt, .. }
            | Self::Cron { title, prompt, .. } => {
                *title = new_title;
                *prompt = new_prompt;
            }
        }
        record
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAtInput {
    pub date: String,
    pub time: String,
    pub time_zone: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AtInput {
    Instant(String),
    Local(LocalAtInput),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyInput {
    pub time: String,
    pub time_zone: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyInput {
    pub time: String,
    pub time_zone: String,
    pub weekdays: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronInput {
    pub expression: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCreateRequest {
    pub title: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<AtInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<DailyInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<WeeklyInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<CronInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum TimingChange {
    At { at: AtInput },
    Every { every_seconds: i64 },
    Daily { daily: DailyInput },
    Weekly { weekly: WeeklyInput },
    Cron { cron: CronInput },
}

/// Raw expected and optional edits are validated after the complete observed record matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleUpdateRequest {
    pub session_id: String,
    pub id: String,
    pub expected: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleError {
    pub code: String,
    pub message: String,
}
impl ScheduleError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_rule", message)
    }
    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::new("corrupt_schedule_log", message)
    }
}
impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ScheduleError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleUpdateResult {
    Changed {
        id: String,
        updated: bool,
        record: HostScheduleRecord,
    },
    Miss {
        id: String,
        updated: bool,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub scheduled_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_scheduled_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Active,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryReceipt {
    pub scheduled_at: String,
    pub delivered_at: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryRecord {
    pub scheduled_at: String,
    pub delivered_at: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}
impl DeliveryRecord {
    pub fn receipt(&self) -> DeliveryReceipt {
        DeliveryReceipt {
            scheduled_at: self.scheduled_at.clone(),
            delivered_at: self.delivered_at.clone(),
            message_id: self.message_id.clone(),
        }
    }
    pub fn from_receipt(receipt: &DeliveryReceipt, prompt: Option<String>) -> Self {
        Self {
            scheduled_at: receipt.scheduled_at.clone(),
            delivered_at: receipt.delivered_at.clone(),
            message_id: receipt.message_id.clone(),
            prompt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryHistory {
    pub records: Vec<DeliveryRecord>,
    pub earlier_records_unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earlier_records_pruned: Option<bool>,
}
impl DeliveryHistory {
    pub fn empty() -> Self {
        Self {
            records: Vec::new(),
            earlier_records_unavailable: false,
            earlier_records_pruned: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostScheduleTask {
    pub session_id: String,
    pub record: HostScheduleRecord,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<DeliveryReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_history: Option<DeliveryHistory>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionBounds {
    pub days: u32,
    pub records: usize,
}
impl Default for RetentionBounds {
    fn default() -> Self {
        Self {
            days: 30,
            records: 200,
        }
    }
}
impl RetentionBounds {
    pub fn validate(&self) -> Result<(), ScheduleError> {
        if !(1..=3650).contains(&self.days) || !(1..=10_000).contains(&self.records) {
            return Err(ScheduleError::invalid(
                "Delivery retention requires days 1-3650 and records 1-10000.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRequest {
    pub session_id: String,
    pub id: String,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HistoryResult {
    Page {
        id: String,
        records: Vec<DeliveryRecord>,
        #[serde(rename = "earlierRecordsUnavailable")]
        earlier_records_unavailable: bool,
        #[serde(rename = "earlierRecordsPruned")]
        earlier_records_pruned: bool,
        retention: RetentionBounds,
        #[serde(rename = "nextBefore", skip_serializing_if = "Option::is_none")]
        next_before: Option<String>,
    },
    Miss {
        id: String,
        code: String,
    },
}
