//! Agent-scoped Schedule management tools over the durable session fold.
//! Rust port of `packages/schedule/schedule/src/tools.ts`.
//!
//! # Deviations
//!
//! - The Rust execution result is a `serde_json::Value`; the closed value
//!   unions are the [`crate::types`] enums serialized to lossless JSON.
//! - The cancellation placeholder reuses the tool runtime's abort predicate
//!   (`exec.signal`) instead of an `AbortSignal`.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::Agent;
use dsh_llm::ContentBlock;
use dsh_tools::{ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime};

use crate::domain::{
    MIN_EVERY_INTERVAL_SECONDS, allocate_schedule_id, create_after_schedule_record,
    create_at_schedule_record, create_every_schedule_record, fold_schedule_events, schedule_view,
};
use crate::persistence::flush_schedule_persistence;
use crate::transaction::run_schedule_transaction;
use crate::types::{
    AtInput, ScheduleDeleteValue, ScheduleId, SchedulePersistenceOperation, ScheduleToolError,
};

fn json(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("lossless JSON")
}

fn internal_error() -> ScheduleToolError {
    ScheduleToolError::InternalError {
        message: "The schedule operation failed.".to_string(),
    }
}

fn corrupt_log_error() -> ScheduleToolError {
    ScheduleToolError::CorruptScheduleLog {
        message: "The session schedule log is corrupt.".to_string(),
    }
}

fn persistence_error(
    operation: SchedulePersistenceOperation,
    id: Option<&ScheduleId>,
) -> ScheduleToolError {
    ScheduleToolError::PersistenceUncertain {
        message:
            "Schedule persistence is uncertain; retry with schedule_list before relying on this result."
                .to_string(),
        operation,
        id: id.cloned(),
    }
}

fn input_error(error: &crate::domain::ScheduleInputError) -> ScheduleToolError {
    match error.code {
        "invalid_prompt" => ScheduleToolError::InvalidPrompt {
            message: error.message.clone(),
        },
        "invalid_selector" => ScheduleToolError::InvalidSelector {
            message: error.message.clone(),
        },
        "invalid_time_zone" => ScheduleToolError::InvalidTimeZone {
            message: error.message.clone(),
        },
        "not_future" => ScheduleToolError::NotFuture {
            message: error.message.clone(),
        },
        "time_out_of_range" => ScheduleToolError::TimeOutOfRange {
            message: error.message.clone(),
        },
        "frequency_too_high" => ScheduleToolError::FrequencyTooHigh {
            message: error.message.clone(),
        },
        _ => ScheduleToolError::InvalidRule {
            message: error.message.clone(),
        },
    }
}

/// Fold only after a successful preflight, mapping corruption to a stable
/// value.
fn fold_for_tool(agent: &dyn Agent) -> Result<crate::domain::FoldedSchedules, ScheduleToolError> {
    fold_schedule_events(
        &agent.session().events(),
        agent.session().header().seed_length.unwrap_or(0) as usize,
    )
    .map_err(|_| corrupt_log_error())
}

/// Require one persistence checkpoint without leaking the backend failure.
async fn preflight(
    root_ctx: &Context,
    agent: &dyn Agent,
    operation: SchedulePersistenceOperation,
    id: Option<&ScheduleId>,
) -> Option<ScheduleToolError> {
    match flush_schedule_persistence(root_ctx, agent.session()).await {
        Ok(()) => None,
        Err(_) => Some(persistence_error(operation, id)),
    }
}

/// Validate the v1 selector constraints that the open parameter root cannot
/// express.
fn validate_create_args(args: &serde_json::Value) -> Option<ScheduleToolError> {
    let Some(map) = args.as_object() else {
        return Some(ScheduleToolError::InvalidSelector {
            message: "schedule_create accepts exactly one of after_seconds, at, or every_seconds."
                .to_string(),
        });
    };
    let allowed = ["prompt", "after_seconds", "at", "every_seconds"];
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Some(ScheduleToolError::InvalidSelector {
            message: "schedule_create accepts exactly one of after_seconds, at, or every_seconds."
                .to_string(),
        });
    }
    let selectors = ["after_seconds", "at", "every_seconds"]
        .iter()
        .filter(|key| map.get(**key).is_some())
        .count();
    if selectors != 1 {
        return Some(ScheduleToolError::InvalidSelector {
            message: "schedule_create accepts exactly one of after_seconds, at, or every_seconds."
                .to_string(),
        });
    }
    let prompt = args
        .get("prompt")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if prompt.trim().is_empty() {
        return Some(ScheduleToolError::InvalidPrompt {
            message: "prompt must be non-empty after trimming.".to_string(),
        });
    }
    if let Some(value) = args.get("after_seconds") {
        let Some(seconds) = value.as_i64().filter(|seconds| *seconds > 0) else {
            return Some(ScheduleToolError::InvalidRule {
                message: "after_seconds must be a positive safe integer.".to_string(),
            });
        };
        let _ = seconds;
    }
    if let Some(value) = args.get("every_seconds") {
        let Some(seconds) = value.as_i64() else {
            return Some(ScheduleToolError::InvalidRule {
                message: "every_seconds must be a safe integer.".to_string(),
            });
        };
        if seconds < MIN_EVERY_INTERVAL_SECONDS {
            return Some(ScheduleToolError::FrequencyTooHigh {
                message: format!("every_seconds must be at least {MIN_EVERY_INTERVAL_SECONDS}."),
            });
        }
    }
    None
}

fn parse_at_input(value: &serde_json::Value) -> Option<AtInput> {
    if let Some(text) = value.as_str() {
        return Some(AtInput::Instant(text.to_string()));
    }
    let map = value.as_object()?;
    Some(AtInput::Local(crate::types::LocalAtInput {
        date: map.get("date")?.as_str()?.to_string(),
        time: map.get("time")?.as_str()?.to_string(),
        time_zone: map.get("time_zone")?.as_str()?.to_string(),
    }))
}

const CREATE_DESCRIPTION: &str = "Create one reminder in the current session. Supply a non-empty prompt and exactly one selector: a positive safe-integer after_seconds delay, at as a strict offset date-time or local date/time object, or safe-integer every_seconds of at least 300. Fixed-rate reminders stay creation-aligned, skip missed occurrences, and batch one latest occurrence per overdue rule. Delivery is session-local: the reminder runs on time only while this session is live and otherwise becomes overdue until the session is resumed.";

const LIST_DESCRIPTION: &str = "List every active reminder in the current session in creation order, including its exact id, UTC target, scheduled or overdue state, and session-local delivery mode.";

const DELETE_DESCRIPTION: &str = "Delete one active reminder in the current session by the exact id returned by schedule_create or schedule_list. Unknown or already-finished ids return deleted false.";

/// Deterministic model content for every canonical Schedule value.
fn render_value(
    _args: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<Vec<ContentBlock>, String> {
    Ok(vec![ContentBlock::Text {
        text: serde_json::to_string(value).expect("lossless JSON"),
    }])
}

fn present(title: &str, kind: ToolCallKind, raw_input: Option<&serde_json::Value>) -> ToolCallView {
    ToolCallView::Generic {
        title: title.to_string(),
        kind: Some(kind),
        raw_input: raw_input.cloned(),
        content: None,
        locations: None,
    }
}

/// Register all three Schedule tools in one exact agent scope.
pub fn register_schedule_tools(
    root_ctx: &Context,
    tool_ctx: &Context,
    agent: Arc<dyn Agent>,
    on_durable_change: Arc<dyn Fn() + Send + Sync>,
) -> cordis::Disposer {
    let tools = tool_ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone());
    let Some(tools) = tools else {
        return cordis::events::make_disposer(move || Box::pin(async move {}));
    };
    let mut disposers: Vec<cordis::Disposer> = Vec::new();

    let notify_durable_change: Arc<dyn Fn() + Send + Sync> = Arc::new({
        let root_ctx = root_ctx.clone();
        let on_durable_change = on_durable_change.clone();
        move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_durable_change()))
                .map_err(|_| {
                    root_ctx.logger.warn(
                        &root_ctx,
                        vec![cordis::arc(
                            "schedule: durable-change observer failed".to_string(),
                        )],
                    )
                });
        }
    });

    // schedule_create
    {
        let agent_for_tool = agent.clone();
        let root_ctx_for_tool = root_ctx.clone();
        let notify = notify_durable_change.clone();
        let definition = ToolDefinition {
            name: "schedule_create".to_string(),
            description: CREATE_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Reminder content to present when the target becomes due."
                    },
                    "after_seconds": {
                        "type": "number",
                        "description": "Positive safe-integer delay in seconds."
                    },
                    "every_seconds": {
                        "type": "number",
                        "description": "Fixed-rate safe-integer interval in seconds, at least 300."
                    },
                    "at": {
                        "description": "Absolute target as strict offset RFC 3339 or local date/time with an explicit IANA zone.",
                        "oneOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "date": { "type": "string" },
                                    "time": { "type": "string" },
                                    "time_zone": { "type": "string" }
                                },
                                "required": ["date", "time", "time_zone"]
                            }
                        ]
                    }
                },
                "required": ["prompt"],
                "additionalProperties": false
            }),
            output: ToolOutputDefinition {
                schema: create_output_schema(),
                render: Arc::new(render_value),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, exec| {
                let args = args.clone();
                let agent = agent_for_tool.clone();
                let root_ctx = root_ctx_for_tool.clone();
                let notify = notify.clone();
                let caller_agent = exec.agent.clone();
                let signal = exec.signal.lock().clone();
                Box::pin(async move {
                    if !caller_agent
                        .as_ref()
                        .is_some_and(|caller| Arc::ptr_eq(caller, &agent))
                    {
                        return Ok(json(&internal_error()));
                    }
                    if let Some(invalid) = validate_create_args(&args) {
                        return Ok(json(&invalid));
                    }
                    Ok(run_schedule_transaction(agent.as_ref(), || {
                        let args = args.clone();
                        let agent = agent.clone();
                        let root_ctx = root_ctx.clone();
                        let notify = notify.clone();
                        let signal = signal.clone();
                        async move {
                            if signal() {
                                return json(&internal_error());
                            }
                            if let Some(uncertain) = preflight(
                                &root_ctx,
                                agent.as_ref(),
                                SchedulePersistenceOperation::Create,
                                None,
                            )
                            .await
                            {
                                return json(&uncertain);
                            }
                            notify();
                            let folded = match fold_for_tool(agent.as_ref()) {
                                Ok(folded) => folded,
                                Err(error) => return json(&error),
                            };
                            let id = allocate_schedule_id(&folded);
                            let record = if let Some(at) = args.get("at").and_then(parse_at_input) {
                                create_at_schedule_record(
                                    id.clone(),
                                    args.get("prompt")
                                        .and_then(|value| value.as_str())
                                        .unwrap_or(""),
                                    &at,
                                    chrono::Utc::now().timestamp_millis(),
                                )
                            } else if let Some(seconds) =
                                args.get("after_seconds").and_then(|value| value.as_i64())
                            {
                                create_after_schedule_record(
                                    id.clone(),
                                    args.get("prompt")
                                        .and_then(|value| value.as_str())
                                        .unwrap_or(""),
                                    seconds,
                                    chrono::Utc::now().timestamp_millis(),
                                )
                            } else {
                                let seconds = args
                                    .get("every_seconds")
                                    .and_then(|value| value.as_i64())
                                    .unwrap_or(0);
                                create_every_schedule_record(
                                    id.clone(),
                                    args.get("prompt")
                                        .and_then(|value| value.as_str())
                                        .unwrap_or(""),
                                    seconds,
                                    chrono::Utc::now().timestamp_millis(),
                                )
                            };
                            let record = match record {
                                Ok(record) => record,
                                Err(error) => return json(&input_error(&error)),
                            };
                            if signal() {
                                return json(&internal_error());
                            }
                            let appended = agent.session().append(
                                "schedule/change",
                                json(&crate::types::ScheduleChange::Create {
                                    version: 1,
                                    schedule: record.clone(),
                                }),
                                None,
                            );
                            if appended.is_err() {
                                return json(&internal_error());
                            }
                            if let Some(barrier) = preflight(
                                &root_ctx,
                                agent.as_ref(),
                                SchedulePersistenceOperation::Create,
                                Some(record.id()),
                            )
                            .await
                            {
                                return json(&barrier);
                            }
                            notify();
                            json(&schedule_view(
                                &record,
                                chrono::Utc::now().timestamp_millis(),
                            ))
                        }
                    })
                    .await)
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|args: &serde_json::Value| {
                Some(present(
                    "Create reminder",
                    ToolCallKind::Other,
                    Some(args.get("prompt").unwrap_or(&serde_json::Value::Null)),
                ))
            })),
            present_result: None,
        };
        match tools.register(tool_ctx, definition) {
            Ok(disposer) => disposers.push(disposer),
            Err(error) => {
                rollback(root_ctx, &mut disposers, &error);
                return cordis::events::make_disposer(move || Box::pin(async move {}));
            }
        }
    }

    // schedule_list
    {
        let agent_for_tool = agent.clone();
        let root_ctx_for_tool = root_ctx.clone();
        let notify = notify_durable_change.clone();
        let definition = ToolDefinition {
            name: "schedule_list".to_string(),
            description: LIST_DESCRIPTION.to_string(),
            parameters: serde_json::json!({ "type": "object", "additionalProperties": false }),
            output: ToolOutputDefinition {
                schema: list_output_schema(),
                render: Arc::new(render_value),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |_args, exec| {
                let agent = agent_for_tool.clone();
                let root_ctx = root_ctx_for_tool.clone();
                let notify = notify.clone();
                let caller_agent = exec.agent.clone();
                let signal = exec.signal.lock().clone();
                Box::pin(async move {
                    if !caller_agent
                        .as_ref()
                        .is_some_and(|caller| Arc::ptr_eq(caller, &agent))
                    {
                        return Ok(json(&internal_error()));
                    }
                    Ok(run_schedule_transaction(agent.as_ref(), || {
                        let agent = agent.clone();
                        let root_ctx = root_ctx.clone();
                        let notify = notify.clone();
                        let signal = signal.clone();
                        async move {
                            if signal() {
                                return json(&internal_error());
                            }
                            if let Some(uncertain) = preflight(
                                &root_ctx,
                                agent.as_ref(),
                                SchedulePersistenceOperation::List,
                                None,
                            )
                            .await
                            {
                                return json(&uncertain);
                            }
                            notify();
                            let folded = match fold_for_tool(agent.as_ref()) {
                                Ok(folded) => folded,
                                Err(error) => return json(&error),
                            };
                            let now = chrono::Utc::now().timestamp_millis();
                            json(&crate::types::ScheduleListValue::Views(
                                folded
                                    .active
                                    .iter()
                                    .map(|record| schedule_view(record, now))
                                    .collect(),
                            ))
                        }
                    })
                    .await)
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|_args: &serde_json::Value| {
                Some(present("List reminders", ToolCallKind::Read, None))
            })),
            present_result: None,
        };
        match tools.register(tool_ctx, definition) {
            Ok(disposer) => disposers.push(disposer),
            Err(error) => {
                rollback(root_ctx, &mut disposers, &error);
                return cordis::events::make_disposer(move || Box::pin(async move {}));
            }
        }
    }

    // schedule_delete
    {
        let agent_for_tool = agent.clone();
        let root_ctx_for_tool = root_ctx.clone();
        let notify = notify_durable_change.clone();
        let definition = ToolDefinition {
            name: "schedule_delete".to_string(),
            description: DELETE_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Exact session-local schedule id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            output: ToolOutputDefinition {
                schema: delete_output_schema(),
                render: Arc::new(render_value),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, exec| {
                let args = args.clone();
                let agent = agent_for_tool.clone();
                let root_ctx = root_ctx_for_tool.clone();
                let notify = notify.clone();
                let caller_agent = exec.agent.clone();
                let signal = exec.signal.lock().clone();
                Box::pin(async move {
                    let Some(raw_id) = args.get("id").and_then(|value| value.as_str()) else {
                        return Ok(json(&ScheduleToolError::InvalidRule {
                            message: "schedule_delete id must be non-empty without surrounding whitespace."
                                .to_string(),
                        }));
                    };
                    if raw_id.is_empty() || raw_id.trim() != raw_id {
                        return Ok(json(&ScheduleToolError::InvalidRule {
                            message: "schedule_delete id must be non-empty without surrounding whitespace."
                                .to_string(),
                        }));
                    }
                    let id = crate::types::schedule_id(raw_id);
                    if !caller_agent
                        .as_ref()
                        .is_some_and(|caller| Arc::ptr_eq(caller, &agent))
                    {
                        return Ok(json(&internal_error()));
                    }
                    Ok(run_schedule_transaction(agent.as_ref(), || {
                        let agent = agent.clone();
                        let root_ctx = root_ctx.clone();
                        let notify = notify.clone();
                        let signal = signal.clone();
                        let id = id.clone();
                        async move {
                            if signal() {
                                return json(&internal_error());
                            }
                            if let Some(uncertain) = preflight(
                                &root_ctx,
                                agent.as_ref(),
                                SchedulePersistenceOperation::Delete,
                                Some(&id),
                            )
                            .await
                            {
                                return json(&uncertain);
                            }
                            notify();
                            let folded = match fold_for_tool(agent.as_ref()) {
                                Ok(folded) => folded,
                                Err(error) => return json(&error),
                            };
                            if !folded.active.iter().any(|record| record.id() == &id) {
                                return json(&ScheduleDeleteValue::Deleted {
                                    id,
                                    deleted: false,
                                    code: Some("schedule_not_found".to_string()),
                                });
                            }
                            if signal() {
                                return json(&internal_error());
                            }
                            let appended = agent.session().append(
                                "schedule/change",
                                json(&crate::types::ScheduleChange::Delete {
                                    version: 1,
                                    id: id.clone(),
                                }),
                                None,
                            );
                            if appended.is_err() {
                                return json(&internal_error());
                            }
                            if let Some(barrier) = preflight(
                                &root_ctx,
                                agent.as_ref(),
                                SchedulePersistenceOperation::Delete,
                                Some(&id),
                            )
                            .await
                            {
                                return json(&barrier);
                            }
                            notify();
                            json(&ScheduleDeleteValue::Deleted {
                                id,
                                deleted: true,
                                code: None,
                            })
                        }
                    })
                    .await)
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|args: &serde_json::Value| {
                Some(present(
                    "Delete reminder",
                    ToolCallKind::Other,
                    Some(args.get("id").unwrap_or(&serde_json::Value::Null)),
                ))
            })),
            present_result: None,
        };
        match tools.register(tool_ctx, definition) {
            Ok(disposer) => disposers.push(disposer),
            Err(error) => {
                rollback(root_ctx, &mut disposers, &error);
                return cordis::events::make_disposer(move || Box::pin(async move {}));
            }
        }
    }

    let shared = Arc::new(parking_lot::Mutex::new((true, disposers)));
    cordis::events::make_disposer(move || {
        let shared = shared.clone();
        Box::pin(async move {
            let disposers = {
                let (active, disposers) = &mut *shared.lock();
                if !*active {
                    return;
                }
                *active = false;
                std::mem::take(disposers)
            };
            for dispose in disposers.into_iter().rev() {
                dispose().await;
            }
        })
    })
}

/// Dispose already-registered tools and report the registration failure
/// (TS rethrows after rolling the disposers back).
fn rollback(root_ctx: &Context, disposers: &mut Vec<cordis::Disposer>, error: &str) {
    for dispose in std::mem::take(disposers).into_iter().rev() {
        let _ = futures::executor::block_on(dispose());
    }
    root_ctx.logger.warn(
        root_ctx,
        vec![cordis::arc(format!(
            "schedule: tool registration failed: {error}"
        ))],
    );
}

fn view_schema() -> serde_json::Value {
    let shared = |kind_value: &str, extra: Option<(&str, serde_json::Value)>| {
        let mut properties = serde_json::Map::new();
        let required = vec![
            "id".to_string(),
            "prompt".to_string(),
            "scheduledAt".to_string(),
            "state".to_string(),
            "deliveryMode".to_string(),
            "kind".to_string(),
        ];
        properties.insert("id".into(), serde_json::json!({ "type": "string" }));
        properties.insert("prompt".into(), serde_json::json!({ "type": "string" }));
        properties.insert(
            "scheduledAt".into(),
            serde_json::json!({ "type": "string" }),
        );
        properties.insert(
            "state".into(),
            serde_json::json!({ "type": "string", "enum": ["scheduled", "overdue"] }),
        );
        properties.insert(
            "deliveryMode".into(),
            serde_json::json!({ "type": "string", "const": "session-local" }),
        );
        properties.insert(
            "kind".into(),
            serde_json::json!({ "type": "string", "const": kind_value }),
        );
        if let Some((name, schema)) = extra {
            properties.insert(name.into(), schema);
        }
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": required,
        })
    };
    serde_json::json!({
        "oneOf": [
            shared("after", Some(("afterSeconds", serde_json::json!({ "type": "integer" })))),
            shared("at", None),
            shared("every", Some(("everySeconds", serde_json::json!({ "type": "integer" })))),
        ]
    })
}

fn basic_error_schema(code: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "code": { "type": "string", "const": code },
            "message": { "type": "string" }
        },
        "required": ["code", "message"]
    })
}

fn error_schemas() -> Vec<serde_json::Value> {
    let schemas = vec![
        basic_error_schema("invalid_prompt"),
        basic_error_schema("invalid_selector"),
        basic_error_schema("invalid_rule"),
        basic_error_schema("invalid_time_zone"),
        basic_error_schema("not_future"),
        basic_error_schema("time_out_of_range"),
        basic_error_schema("frequency_too_high"),
        basic_error_schema("corrupt_schedule_log"),
        basic_error_schema("internal_error"),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "code": { "type": "string", "const": "persistence_uncertain" },
                "message": { "type": "string" },
                "operation": { "type": "string", "enum": ["create", "list", "delete"] },
                "id": { "type": "string" }
            },
            "required": ["code", "message", "operation"]
        }),
    ];
    schemas
}

fn create_output_schema() -> serde_json::Value {
    let mut one_of = vec![view_schema()];
    one_of.extend(error_schemas());
    serde_json::json!({ "oneOf": one_of })
}

fn list_output_schema() -> serde_json::Value {
    let mut one_of = vec![serde_json::json!({ "type": "array", "items": view_schema() })];
    one_of.extend(error_schemas());
    serde_json::json!({ "oneOf": one_of })
}

fn delete_output_schema() -> serde_json::Value {
    let mut one_of = vec![
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "id": { "type": "string" },
                "deleted": { "type": "boolean", "const": true }
            },
            "required": ["id", "deleted"]
        }),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "id": { "type": "string" },
                "deleted": { "type": "boolean", "const": false },
                "code": { "type": "string", "const": "schedule_not_found" }
            },
            "required": ["id", "deleted", "code"]
        }),
    ];
    one_of.extend(error_schemas());
    serde_json::json!({ "oneOf": one_of })
}
