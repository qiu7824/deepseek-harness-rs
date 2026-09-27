//! Durable task records and the views clients receive.

use serde::{Deserialize, Serialize};

use crate::rules::TaskRule;

/// Longest stored title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// Longest stored instruction, in characters.
pub const MAX_PROMPT_CHARS: usize = 8000;
/// Storage document version.
pub const DOCUMENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Active,
    Inactive,
}

/// Who created the task: the user through a client, or the model through
/// `scheduled_task_create`. Delivery framing differs between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskOrigin {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryOutcome {
    Delivered,
    Failed,
}

/// One acknowledged delivery attempt. `delivered` means the message entered
/// the session inbox, not that the model finished the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub occurrence_at: String,
    pub delivered_at: String,
    pub outcome: DeliveryOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Instruction as sent, so a later edit does not rewrite history.
    pub prompt: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTask {
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub prompt: String,
    pub rule: TaskRule,
    pub status: TaskStatus,
    pub origin: TaskOrigin,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<Delivery>,
    #[serde(default)]
    pub delivery_history: Vec<Delivery>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub earlier_records_pruned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDocument {
    pub version: u32,
    pub tasks: Vec<ScheduleTask>,
}

/// Catalog row: the task without its delivery history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub prompt: String,
    pub rule: TaskRule,
    pub status: TaskStatus,
    pub origin: TaskOrigin,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<Delivery>,
    pub history_count: usize,
}

impl From<&ScheduleTask> for TaskView {
    fn from(task: &ScheduleTask) -> Self {
        Self {
            id: task.id.clone(),
            session_id: task.session_id.clone(),
            title: task.title.clone(),
            prompt: task.prompt.clone(),
            rule: task.rule.clone(),
            status: task.status,
            origin: task.origin,
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
            next_run_at: task.next_run_at.clone(),
            last_delivery: task.last_delivery.clone(),
            history_count: task.delivery_history.len(),
        }
    }
}

/// Management failure with a stable code for clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleError {
    pub code: &'static str,
    pub message: String,
}

impl ScheduleError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<crate::rules::RuleError> for ScheduleError {
    fn from(error: crate::rules::RuleError) -> Self {
        Self::new(error.code, error.message)
    }
}
