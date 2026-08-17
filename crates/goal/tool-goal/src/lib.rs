//! Model-facing `get_goal`, `create_goal`, and `update_goal` tools over the
//! persisted same-session goal domain. Rust port of
//! `packages/goal/tool-goal`.

mod authority;
pub mod invariant;
pub mod wrapup;

use std::sync::Arc;

use authority::{
    GoalToolAuthority, completion_authority, domain_error, goal_tool_execution, policy_error,
    require_direct_human,
};
use cordis::{ArcValue, Context, Disposer, Plugin, PluginError};
use dsh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalBlockReason, GoalPhase, GoalRef,
    GoalService, GoalView, goal_id,
};
use dsh_llm::{
    ContentBlock, ContextForm, MessageSource, bound_context_summary, create_user_message,
};
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition,
    ToolRunContext, ToolRuntime, validate_json_schema_value,
};
use serde_json::{Value, json};

pub const NAME: &str = "tool-goal";
pub const INJECT: [&str; 4] = ["agents", "goals", "tools", "systemPrompt"];
pub const DEFAULT_BLOCKED_AFTER_CONSECUTIVE_ROUNDS: u64 = 3;

const INVALID_UPDATE: &str = "GOAL_TOOL_INVALID_UPDATE";
const BLOCK_THRESHOLD: &str = "GOAL_TOOL_BLOCK_THRESHOLD";

const CREATE_DESCRIPTION: &str = "Create one persisted same-session completion goal when the current direct human request is a long-running objective that should continue across autonomous goal rounds. You may infer that intent without requiring the user to say \"create a goal\". Do not use this for trivial single-turn work. Execution rejects non-human and subagent authority.";
const GET_DESCRIPTION: &str = "Read the current same-session goal, including its exact id/revision, objective, phase, completed continuation rounds, round limit, blocker reason when present, and whether another continuation is armed. Call this before updating a goal.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    pub blocked_after_consecutive_rounds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub blocked_after_consecutive_rounds: u64,
}

fn resolve_config(config: &Config) -> Result<ResolvedConfig, String> {
    let blocked_after = config
        .blocked_after_consecutive_rounds
        .unwrap_or(DEFAULT_BLOCKED_AFTER_CONSECUTIVE_ROUNDS);
    if blocked_after == 0 {
        return Err("blockedAfterConsecutiveRounds must be a positive safe integer".to_string());
    }
    Ok(ResolvedConfig {
        blocked_after_consecutive_rounds: blocked_after,
    })
}

pub fn guidance(blocked_after: u64) -> String {
    format!(
        "Use goal tools for one long-running completion objective in the current session. create_goal may infer goal intent from a direct human request in any language; do not create a goal for routine single-turn work. Call get_goal before update_goal and copy its exact goal_id and revision. After session resume or fork, an active goal is disarmed: when a human asks to continue or resume in any wording or language, use update_goal action resume to rearm it. Mark complete only when the objective is actually achieved. Mark blocked only after the same blocking condition persists for at least {blocked_after} consecutive goal rounds, and report that concrete condition in blocked_reason; difficulty, uncertainty, or useful remaining work is not blocked."
    )
}

fn goal_value(goal: Option<GoalView>) -> Value {
    let Some(goal) = goal else {
        return json!({ "goal": null });
    };
    let mut value = json!({
        "goal": {
            "id": goal.id.as_str(),
            "revision": goal.revision,
            "objective": goal.objective,
            "phase": goal.phase.as_str(),
            "roundsStarted": goal.rounds_started,
            "maxGoalRounds": goal.max_goal_rounds,
        },
        "activation": match goal.activation {
            GoalActivation::Armed => "armed",
            GoalActivation::Disarmed => "disarmed",
        },
    });
    if let Some(reason) = goal.blocked_reason {
        value["goal"]["blockedReason"] = json!({
            "code": reason.code,
            "message": reason.message,
        });
    }
    value
}

fn goal_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": { "goal": { "type": "null" } },
                "required": ["goal"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "goal": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": { "type": "string" },
                            "revision": { "type": "integer" },
                            "objective": { "type": "string" },
                            "phase": { "type": "string", "enum": ["active", "paused", "blocked", "complete"] },
                            "roundsStarted": { "type": "integer" },
                            "maxGoalRounds": { "type": "integer" },
                            "blockedReason": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "code": { "type": "string" },
                                    "message": { "type": "string" }
                                },
                                "required": ["code", "message"]
                            }
                        },
                        "required": ["id", "revision", "objective", "phase", "roundsStarted", "maxGoalRounds"]
                    },
                    "activation": { "type": "string", "enum": ["armed", "disarmed"] }
                },
                "required": ["goal", "activation"]
            }
        ]
    })
}

fn output_definition() -> ToolOutputDefinition {
    ToolOutputDefinition {
        schema: goal_output_schema(),
        render: Arc::new(|_args, value| {
            Ok(vec![ContentBlock::Text {
                text: value.to_string(),
            }])
        }),
        presentation_meta: None,
    }
}

fn get_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    })
}

fn create_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "objective": {
                "type": "string",
                "description": "The concrete completion objective inferred from the direct human request."
            },
            "max_goal_rounds": {
                "type": "number",
                "description": "Optional positive safe-integer limit on automatic continuation rounds."
            }
        },
        "required": ["objective"]
    })
}

fn update_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "goal_id": { "type": "string", "description": "Exact id returned by get_goal." },
            "revision": { "type": "number", "description": "Exact positive revision returned by get_goal." },
            "action": {
                "type": "string",
                "enum": ["edit", "pause", "resume", "complete", "blocked"],
                "description": "edit | pause | resume | complete | blocked"
            },
            "objective": { "type": "string", "description": "Replacement objective; valid only with action edit." },
            "max_goal_rounds": { "type": "number", "description": "Replacement cap; valid only with action edit." },
            "blocked_reason": { "type": "string", "description": "Concrete blocking condition; required only with action blocked." }
        },
        "required": ["goal_id", "revision", "action"]
    })
}

fn validate(schema: &Value, args: &Value) -> Result<(), ToolBodyError> {
    let violations = validate_json_schema_value(schema, args, "arguments");
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolBodyError::plain(violations.join("; ")))
    }
}

fn goals(ctx: &Context) -> Result<Arc<GoalService>, ToolBodyError> {
    ctx.get_typed::<Arc<GoalService>>("goals", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| policy_error("goal service is unavailable", "GOAL_TOOL_DRIVER_REQUIRED"))
}

fn has_text(args: &Value, key: &str) -> bool {
    args.get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn has_round_cap(args: &Value) -> bool {
    args.get("max_goal_rounds")
        .and_then(Value::as_f64)
        .is_some_and(|value| value != 0.0)
}

fn round_cap(args: &Value) -> Result<Option<u64>, ToolBodyError> {
    let Some(value) = args.get("max_goal_rounds") else {
        return Ok(None);
    };
    value.as_u64().map(Some).ok_or_else(|| {
        ToolBodyError::coded(
            "maxGoalRounds must be a positive safe integer",
            "GoalError",
            "GOAL_INVALID_MAX_ROUNDS",
        )
    })
}

fn exact_goal_ref(args: &Value) -> Result<GoalRef, ToolBodyError> {
    let id = args.get("goal_id").and_then(Value::as_str).unwrap_or("");
    let revision = args.get("revision").and_then(Value::as_u64).unwrap_or(0);
    if id.is_empty() || id.trim() != id || revision < 1 {
        return Err(policy_error(
            "goal_id must be non-empty and revision must be a positive safe integer",
            INVALID_UPDATE,
        ));
    }
    Ok(GoalRef {
        id: goal_id(id),
        revision,
    })
}

fn execute_get(ctx: &Context, args: &Value, exec: &ToolRunContext) -> Result<Value, ToolBodyError> {
    validate(&get_parameters(), args)?;
    let execution = goal_tool_execution(ctx, exec)?;
    Ok(goal_value(
        goals(ctx)?.get(&execution.agent).map_err(domain_error)?,
    ))
}

fn execute_create(
    ctx: &Context,
    args: &Value,
    exec: &ToolRunContext,
) -> Result<Value, ToolBodyError> {
    validate(&create_parameters(), args)?;
    let execution = goal_tool_execution(ctx, exec)?;
    require_direct_human(ctx, &execution)?;
    let objective = args
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let goal = goals(ctx)?
        .create(
            &execution.agent,
            CreateGoalRequest {
                objective,
                max_goal_rounds: round_cap(args)?,
            },
        )
        .map_err(domain_error)?;
    Ok(goal_value(Some(goal)))
}

fn invalid_update(message: &str) -> ToolBodyError {
    policy_error(message, INVALID_UPDATE)
}

fn execute_update(
    ctx: &Context,
    config: ResolvedConfig,
    args: &Value,
    exec: &ToolRunContext,
) -> Result<Value, ToolBodyError> {
    validate(&update_parameters(), args)?;
    let execution = goal_tool_execution(ctx, exec)?;
    let ref_ = exact_goal_ref(args)?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .expect("schema-validated action");
    let service = goals(ctx)?;

    let goal = match action {
        "edit" => {
            require_direct_human(ctx, &execution)?;
            if has_text(args, "blocked_reason") {
                return Err(invalid_update(
                    "blocked_reason is valid only with action blocked",
                ));
            }
            service
                .edit(
                    &execution.agent,
                    &ref_,
                    &EditGoalRequest {
                        objective: has_text(args, "objective")
                            .then(|| args["objective"].as_str().expect("string").to_string()),
                        max_goal_rounds: if has_round_cap(args) {
                            round_cap(args)?
                        } else {
                            None
                        },
                    },
                )
                .map_err(domain_error)?
        }
        "pause" | "resume" => {
            require_direct_human(ctx, &execution)?;
            if has_text(args, "objective")
                || has_round_cap(args)
                || has_text(args, "blocked_reason")
            {
                return Err(invalid_update(
                    "objective and max_goal_rounds are valid only with action edit; blocked_reason is valid only with action blocked",
                ));
            }
            if action == "pause" {
                service.pause(&execution.agent, &ref_)
            } else {
                service.resume(&execution.agent, &ref_)
            }
            .map_err(domain_error)?
        }
        "complete" | "blocked" => {
            let authority = completion_authority(ctx, &execution)?;
            if has_text(args, "objective") || has_round_cap(args) {
                return Err(invalid_update(
                    "objective and max_goal_rounds are valid only with action edit",
                ));
            }
            if action == "complete" && has_text(args, "blocked_reason") {
                return Err(invalid_update(
                    "blocked_reason is valid only with action blocked",
                ));
            }
            let blocked_reason = args.get("blocked_reason").and_then(Value::as_str);
            if action == "blocked" && blocked_reason.is_none_or(|reason| reason.trim().is_empty()) {
                return Err(invalid_update(
                    "blocked_reason is required with action blocked",
                ));
            }
            if let GoalToolAuthority::GoalRound(current) = &authority
                && action == "blocked"
                && current.rounds_started < config.blocked_after_consecutive_rounds
            {
                return Err(policy_error(
                    format!(
                        "blocked requires at least {} consecutive goal rounds; current round is {}",
                        config.blocked_after_consecutive_rounds, current.rounds_started
                    ),
                    BLOCK_THRESHOLD,
                ));
            }
            let goal = if action == "complete" {
                service.complete(&execution.agent, &ref_)
            } else {
                service.block(
                    &execution.agent,
                    &ref_,
                    &GoalBlockReason {
                        code: "model-reported".to_string(),
                        message: blocked_reason
                            .expect("validated blocked reason")
                            .to_string(),
                    },
                )
            }
            .map_err(domain_error)?;
            if matches!(authority, GoalToolAuthority::GoalRound(_)) {
                let summary = bound_context_summary(&format!("{action}: {}", goal.objective));
                exec.defer_context(create_user_message(
                    wrapup::render_wrapup_context(
                        &goal.objective,
                        (action == "blocked")
                            .then_some(blocked_reason.expect("validated blocked reason")),
                    ),
                    MessageSource::Plugin {
                        plugin: NAME.to_string(),
                        form: Some(ContextForm::Notice),
                        sections: None,
                        summary: Some(summary),
                        compaction_id: None,
                        source_command_id: None,
                    },
                ));
            }
            goal
        }
        _ => unreachable!("schema-validated action"),
    };
    Ok(goal_value(Some(goal)))
}

fn generic(title: &str, kind: ToolCallKind, raw_input: Option<Value>) -> ToolCallView {
    ToolCallView::Generic {
        title: title.to_string(),
        kind: Some(kind),
        raw_input,
        content: None,
        locations: None,
    }
}

fn present_update(args: &Value) -> Option<ToolCallView> {
    let action = args.get("action")?.as_str()?;
    if !["edit", "pause", "resume", "complete", "blocked"].contains(&action) {
        return None;
    }
    let goal_id = args.get("goal_id")?.as_str()?;
    args.get("revision")?.as_u64()?;
    let title = if action == "blocked" {
        "Mark goal".to_string()
    } else {
        format!("{}{} goal", action[..1].to_ascii_uppercase(), &action[1..])
    };
    let raw_input = if has_text(args, "blocked_reason") {
        Some(Value::String(args["blocked_reason"].as_str()?.to_string()))
    } else if has_text(args, "objective") {
        Some(Value::String(args["objective"].as_str()?.to_string()))
    } else if has_round_cap(args) {
        args.get("max_goal_rounds").cloned()
    } else {
        Some(Value::String(goal_id.to_string()))
    };
    Some(generic(&title, ToolCallKind::Other, raw_input))
}

/// Register the three Codex-shaped goal tools and their shared policy section.
pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    let resolved = resolve_config(config)?;
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-goal requires the tools service".to_string())?;
    let system_prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-goal requires the systemPrompt service".to_string())?;
    for name in ["get_goal", "create_goal", "update_goal"] {
        if tools.get(name, None).is_some() {
            panic!("tool \"{name}\" is already registered");
        }
    }

    let mut disposers = vec![system_prompt.section(
        ctx,
        PromptSection {
            name: "tool:goal".to_string(),
            order: 114.0,
            text: PromptText::Static(guidance(resolved.blocked_after_consecutive_rounds)),
            complete: None,
        },
    )];

    let get_ctx = ctx.clone();
    disposers.push(tools.register(
        ctx,
        ToolDefinition {
            name: "get_goal".to_string(),
            description: GET_DESCRIPTION.to_string(),
            parameters: get_parameters(),
            output: output_definition(),
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, exec| {
                let result = execute_get(&get_ctx, args, exec);
                Box::pin(async move { result })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|_args| {
                Some(generic("Read current goal", ToolCallKind::Read, None))
            })),
            present_result: None,
        },
    )?);

    let create_ctx = ctx.clone();
    disposers.push(tools.register(
        ctx,
        ToolDefinition {
            name: "create_goal".to_string(),
            description: CREATE_DESCRIPTION.to_string(),
            parameters: create_parameters(),
            output: output_definition(),
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, exec| {
                let result = execute_create(&create_ctx, args, exec);
                Box::pin(async move { result })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|args| {
                let objective = args.get("objective")?.as_str()?;
                Some(generic(
                    "Create goal",
                    ToolCallKind::Other,
                    Some(Value::String(objective.to_string())),
                ))
            })),
            present_result: None,
        },
    )?);

    let update_ctx = ctx.clone();
    disposers.push(tools.register(
        ctx,
        ToolDefinition {
            name: "update_goal".to_string(),
            description: "Update the exact current goal revision. edit, pause, and resume require a direct top-level human request. During an automatic continuation of the current goal, complete and blocked are also allowed. blocked is rejected before the configured minimum round count; the model remains responsible for judging that the same condition persisted across those rounds and must explain it in blocked_reason.".to_string(),
            parameters: update_parameters(),
            output: output_definition(),
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, exec| {
                let result = execute_update(&update_ctx, resolved, args, exec);
                Box::pin(async move { result })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(present_update)),
            present_result: None,
        },
    )?);

    Ok(cordis::make_disposer(move || {
        let disposers = disposers.clone();
        Box::pin(async move {
            for disposer in disposers.into_iter().rev() {
                disposer().await;
            }
        })
    }))
}

pub struct ToolGoalPlugin;

#[async_trait::async_trait]
impl Plugin for ToolGoalPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config
            .downcast_ref::<Config>()
            .cloned()
            .ok_or_else(|| PluginError::from(anyhow::anyhow!("tool-goal requires config")))?;
        let disposer =
            apply(ctx, &config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("tool-goal", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
