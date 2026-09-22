//! Host-owned goal requirements identity and completion validation boundary.

use dsh_agent::Agent;
use std::sync::Arc;

/// The exact objective generation. Lifecycle and usage changes do not change it.
/// The revision comes from validated goal events, never from model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRequirementsIdentity {
    pub goal_id: String,
    pub objective_revision: u64,
}

pub const GOAL_COMPLETION_GUARD_SERVICE: &str = "goalCompletionGuard";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCompletionError {
    Cancelled,
    Blocked(String),
}

impl From<String> for GoalCompletionError {
    fn from(message: String) -> Self {
        Self::Blocked(message)
    }
}

impl From<&str> for GoalCompletionError {
    fn from(message: &str) -> Self {
        Self::Blocked(message.to_owned())
    }
}

impl std::fmt::Display for GoalCompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("goal completion was cancelled"),
            Self::Blocked(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for GoalCompletionError {}

/// Bounded JSON projection for live and cold session-projection caches. The
/// caller retains its previous Arc when None is returned. No event bodies or
/// historical objective strings are retained in the checkpoint.
pub fn apply_goal_requirements_projection(
    previous: &serde_json::Value,
    event: &dsh_session::SessionEvent,
) -> Option<serde_json::Value> {
    use crate::domain::GoalChangeMeta;
    use serde_json::{Value, json};
    if event.type_ == "goal/change" {
        let change = crate::fold::decode_goal_change(&event.data)
            .ok()
            .flatten()?;
        return Some(match change {
            GoalChangeMeta::Clear(_) => Value::Null,
            GoalChangeMeta::Snapshot(change) => {
                let prior = &previous["goal"];
                let same_objective = prior["id"].as_str() == Some(change.goal.id.as_str())
                    && prior["objective"].as_str() == Some(change.goal.objective.as_str());
                let objective_revision = if same_objective {
                    prior["objectiveRevision"]
                        .as_u64()
                        .unwrap_or(change.goal.revision)
                } else {
                    change.goal.revision
                };
                let mut goal = event.data["goal"].clone();
                goal["objectiveRevision"] = json!(objective_revision);
                json!({"goal": goal, "roundsStarted": change.rounds_started,
                    "createdAt": change.created_at, "updatedAt": change.updated_at})
            }
        });
    }
    if event.type_ == "user/message" {
        let source = &event.data["source"];
        let goal = &previous["goal"];
        let next_round = previous["roundsStarted"].as_u64()?.checked_add(1)?;
        if source["kind"].as_str() == Some("goal")
            && goal["phase"].as_str() == Some("active")
            && source["goalId"] == goal["id"]
            && source["revision"] == goal["revision"]
            && source["round"].as_u64() == Some(next_round)
            && next_round <= goal["maxGoalRounds"].as_u64()?
        {
            let mut next = previous.clone();
            next["roundsStarted"] = json!(next_round);
            return Some(next);
        }
    }
    None
}

/// A Host proof whose cancellation/work reservation stays alive through commit.
pub trait GoalCompletionCommitGuard {}

pub trait GoalCompletionPermit: Send + Sync {
    /// Final bounded state/revision check. This runs under the Goal mutation
    /// claim, but without any Goal cache mutex held. Do not await or re-enter
    /// goal mutation here.
    fn check<'a>(
        &'a self,
        agent: &Arc<dyn Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionCommitGuard + 'a>, GoalCompletionError>;
}

#[async_trait::async_trait]
pub trait GoalCompletionGuard: Send + Sync {
    /// Verify current acceptance through the configured filesystem/approval
    /// provider. No Goal claim or cache mutex is held while this awaits.
    async fn prepare(
        &self,
        agent: &Arc<dyn Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionPermit>, GoalCompletionError>;
}
