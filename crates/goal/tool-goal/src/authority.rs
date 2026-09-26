//! Execution-time authority checks for the model-facing goal tools.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentRegistry, AgentStatus};
use dsh_goal::{GoalService, GoalView};
use dsh_tools::{ToolBodyError, ToolRunContext};
use serde_json::Value;

pub(crate) const AGENT_REQUIRED: &str = "GOAL_TOOL_AGENT_REQUIRED";
pub(crate) const DRIVER_REQUIRED: &str = "GOAL_TOOL_DRIVER_REQUIRED";
pub(crate) const AUTHORITY_REQUIRED: &str = "GOAL_TOOL_AUTHORITY_REQUIRED";

/// Authenticated calling agent and the events accepted in its current open turn.
pub(crate) struct GoalToolExecution {
    pub agent: Arc<dyn Agent>,
    pub sources: Vec<Value>,
}

/// Hard authority granted to a terminal state-changing call.
pub(crate) enum GoalToolAuthority {
    DirectHuman,
    GoalRound(GoalView),
}

pub(crate) fn policy_error(message: impl Into<String>, code: &str) -> ToolBodyError {
    ToolBodyError::coded(message, "HarnessError", code)
}

pub(crate) fn domain_error(error: dsh_goal::GoalError) -> ToolBodyError {
    ToolBodyError::coded(error.message, "GoalError", error.code.as_str())
}

fn reject<T>(message: impl Into<String>, code: &str) -> Result<T, ToolBodyError> {
    Err(policy_error(message, code))
}

/// Locate the open turn enclosing a model tool call.
fn open_turn(agent: &Arc<dyn Agent>) -> Result<Vec<Value>, ToolBodyError> {
    agent.session().with_event_reader(|reader| {
        let boundary = reader.find_rev(|event| matches!(event.type_.as_str(), "turn/start" | "turn/end"))
            .map_err(|error| policy_error(format!("goal authority history could not be read: {error}"), DRIVER_REQUIRED))?;
        let Some(boundary) = boundary.filter(|event| event.type_ == "turn/start") else {
            return reject("goal tools require an open model turn", DRIVER_REQUIRED);
        };
        let mut sources = Vec::new();
        reader.visit(boundary.seq.get() + 1, None, |event| {
            if event.type_ == "user/message" && let Some(source) = event.data.get("source") {
                match source.get("kind").and_then(Value::as_str) {
                    Some("user") => sources.push(serde_json::json!({"kind":"user"})),
                    Some("goal") => sources.push(serde_json::json!({"kind":"goal","goalId":source.get("goalId"),"revision":source.get("revision"),"round":source.get("round")})),
                    _ => {}
                }
            }
            Ok(true)
        }).map_err(|error| policy_error(format!("goal authority history could not be read: {error}"), DRIVER_REQUIRED))?;
        Ok(sources)
    })
}

/// Resolve the exact live calling agent and its current driver boundary.
pub(crate) fn goal_tool_execution(
    ctx: &Context,
    exec: &ToolRunContext,
) -> Result<GoalToolExecution, ToolBodyError> {
    let agent = exec
        .agent
        .clone()
        .ok_or_else(|| policy_error("goal tools require a calling agent", AGENT_REQUIRED))?;
    let agents = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            policy_error(
                "goal tools require the exact live calling agent inside its active driver",
                DRIVER_REQUIRED,
            )
        })?;
    let live = agents.get(agent.id());
    let initiator = agents.current_initiator().ok().flatten();
    if agent.status() != AgentStatus::Running
        || !live
            .as_ref()
            .is_some_and(|candidate| Arc::ptr_eq(candidate, &agent))
        || !initiator
            .as_ref()
            .is_some_and(|candidate| Arc::ptr_eq(candidate, &agent))
    {
        return reject(
            "goal tools require the exact live calling agent inside its active driver",
            DRIVER_REQUIRED,
        );
    }
    Ok(GoalToolExecution {
        sources: open_turn(&agent)?,
        agent,
    })
}

/// Whether host-attested human input appears in the current root-agent turn.
fn has_direct_human_input(ctx: &Context, execution: &GoalToolExecution) -> bool {
    let Some(agents) = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
    else {
        return false;
    };
    let is_root = agents
        .roots()
        .iter()
        .any(|root| Arc::ptr_eq(root, &execution.agent));
    is_root
        && execution
            .sources
            .iter()
            .any(|source| source["kind"].as_str() == Some("user"))
}

/// Whether this turn carries the current goal's exact admitted round source.
fn is_matching_goal_round(execution: &GoalToolExecution, goal: &GoalView) -> bool {
    if goal.rounds_started == 0 {
        return false;
    }
    execution.sources.iter().any(|source| {
        source["kind"].as_str() == Some("goal")
            && source["goalId"].as_str() == Some(goal.id.as_str())
            && source["revision"].as_u64() == Some(goal.revision)
            && source["round"].as_u64() == Some(goal.rounds_started)
    })
}

/// Require authority originating in a human message accepted by a runtime root.
pub(crate) fn require_direct_human(
    ctx: &Context,
    execution: &GoalToolExecution,
) -> Result<(), ToolBodyError> {
    if has_direct_human_input(ctx, execution) {
        return Ok(());
    }
    reject(
        "this goal operation requires a direct human turn on a top-level agent",
        AUTHORITY_REQUIRED,
    )
}

/// Resolve terminal authority from direct human input or the exact goal round.
pub(crate) fn completion_authority(
    ctx: &Context,
    execution: &GoalToolExecution,
) -> Result<GoalToolAuthority, ToolBodyError> {
    if has_direct_human_input(ctx, execution) {
        return Ok(GoalToolAuthority::DirectHuman);
    }
    let goals = ctx
        .get_typed::<Arc<GoalService>>("goals", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            policy_error(
                "complete and blocked require a direct human turn or the current goal round",
                AUTHORITY_REQUIRED,
            )
        })?;
    let goal = goals
        .validated_authority_view(&execution.agent)
        .map_err(domain_error)?;
    if let Some(goal) = goal {
        if is_matching_goal_round(execution, &goal) {
            return Ok(GoalToolAuthority::GoalRound(goal));
        }
    }
    reject(
        "complete and blocked require a direct human turn or the current goal round",
        AUTHORITY_REQUIRED,
    )
}
