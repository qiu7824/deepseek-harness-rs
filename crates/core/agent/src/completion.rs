//! Host-owned completion assessment, independent of model prose and turn termination.
use crate::Agent;
use cordis::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionAssessment {
    pub status: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(skip)]
    pub follow_up: Option<String>,
}

pub struct CompletionRequest {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub attempts: u8,
    pub cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub struct CompletionReview {
    pub review: Arc<
        dyn Fn(
                CompletionRequest,
            ) -> BoxFuture<'static, Result<Option<CompletionAssessment>, String>>
            + Send
            + Sync,
    >,
}
impl cordis::Service for CompletionReview {
    fn service_name(&self) -> &'static str {
        "completionReview"
    }
}
