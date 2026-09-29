//! Stable goal objective identity and historical projection.

/// The exact objective generation. Lifecycle and usage changes do not change it.
/// The revision comes from validated goal events, never from model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRequirementsIdentity {
    pub goal_id: String,
    pub objective_revision: u64,
}

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
