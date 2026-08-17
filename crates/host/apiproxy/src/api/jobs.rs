//! `jobs` domain contract: the background-job view vocabulary. Rust port
//! of `packages/host/apiproxy/src/api/jobs.ts` + `jobs.schema.ts`
//! (taskViewSchema).

use serde::{Deserialize, Serialize};

/// Task identity on the wire (branded in the domain crate; the contract
/// layer reads and writes it as an opaque string).
pub type JobId = dsh_brand::Branded<JobIdTag>;

#[doc(hidden)]
pub enum JobIdTag {}

/// One background-job view row (the TS `JobView` / `taskViewSchema`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    /// Non-empty job kind literal.
    pub kind: String,
    /// Non-empty operator-facing label.
    pub label: String,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Unix epoch milliseconds.
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

/// The closed job-status vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}
