//! Model tools over the Host task domain. Each call binds to the calling
//! agent's session; tasks are delivered back into that session.

use std::sync::Arc;

use chrono::Duration;
use serde_json::{Value, json};

use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};

use crate::model::{ScheduleError, TaskOrigin, TaskView};
use crate::rules::{TaskRule, format_instant};
use crate::service::{CreateTask, ScheduleService, UpdateTask};

const CREATE_DESCRIPTION: &str = "Create a scheduled task in the current conversation. At its time the task's prompt is delivered back into this conversation as a new message and you carry it out, even if the conversation was closed or the app restarted. Give a non-empty prompt written as the instruction to execute later, an optional short title, and exactly one timing selector: after_seconds (delay), at (RFC 3339 time with offset), every_seconds (fixed interval), daily {time, time_zone}, weekly {time, weekdays 1=Mon..7=Sun, time_zone} or cron {expression (5 fields), time_zone}. Repeats cannot be more frequent than once a minute. Use the user's time zone.";
const LIST_DESCRIPTION: &str = "List the scheduled tasks of the current conversation with their exact ids, rules, enabled status, next run time and latest delivery.";
const UPDATE_DESCRIPTION: &str = "Change the title, prompt or timing of one scheduled task in the current conversation by its exact id. Supply only the fields to change; a timing selector replaces the whole rule.";
const DELETE_DESCRIPTION: &str = "Delete one scheduled task of the current conversation by its exact id. Future deliveries stop; messages already delivered stay in the conversation.";

fn zone_schema() -> Value {
    json!({"type": "string", "description": "IANA time zone such as Asia/Shanghai, or UTC"})
}

fn timing_properties() -> serde_json::Map<String, Value> {
    // The tool schema subset rejects `pattern`; the rule validator checks the
    // HH:MM form and reports a readable error instead.
    let time = json!({"type": "string", "description": "Local time HH:MM (24-hour)"});
    json!({
        "after_seconds": {"type": "integer", "minimum": 60, "description": "Run once after this many seconds"},
        "at": {"type": "string", "description": "Run once at this RFC 3339 time with offset, e.g. 2026-09-27T09:00:00+08:00"},
        "every_seconds": {"type": "integer", "minimum": 60, "description": "Repeat at this fixed interval"},
        "daily": {"type": "object", "additionalProperties": false, "required": ["time", "time_zone"], "properties": {"time": time, "time_zone": zone_schema()}},
        "weekly": {"type": "object", "additionalProperties": false, "required": ["time", "weekdays", "time_zone"], "properties": {"time": time, "weekdays": {"type": "array", "minItems": 1, "maxItems": 7, "items": {"type": "integer", "minimum": 1, "maximum": 7}}, "time_zone": zone_schema()}},
        "cron": {"type": "object", "additionalProperties": false, "required": ["expression", "time_zone"], "properties": {"expression": {"type": "string", "description": "minute hour day-of-month month day-of-week"}, "time_zone": zone_schema()}}
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

const SELECTORS: [&str; 6] = [
    "after_seconds",
    "at",
    "every_seconds",
    "daily",
    "weekly",
    "cron",
];

/// The rule named by the one selector present in `args`, or None when no
/// selector is present.
pub fn rule_from_args(
    args: &Value,
    service: &ScheduleService,
) -> Result<Option<TaskRule>, ScheduleError> {
    let object = args
        .as_object()
        .ok_or_else(|| ScheduleError::new("invalid_request", "工具参数必须是对象"))?;
    if object.contains_key("rule") {
        return Err(ScheduleError::new(
            "invalid_selector",
            "scheduled_task_* 执行定时任务，使用 after_seconds、at、every_seconds、daily、weekly 或 cron 平铺时间参数；schedule_* 仅管理提醒。",
        ));
    }
    if object.keys().any(|key| {
        !SELECTORS.contains(&key.as_str()) && !["id", "title", "prompt"].contains(&key.as_str())
    }) {
        return Err(ScheduleError::new("invalid_request", "未知定时任务参数"));
    }
    if SELECTORS
        .iter()
        .any(|key| object.get(*key).is_some_and(Value::is_null))
    {
        return Err(ScheduleError::new(
            "invalid_selector",
            "时间参数不能为 null",
        ));
    }
    let present: Vec<&str> = SELECTORS
        .into_iter()
        .filter(|key| args.get(*key).is_some_and(|value| !value.is_null()))
        .collect();
    if present.len() > 1 {
        return Err(ScheduleError::new(
            "invalid_selector",
            "只能提供一个时间选择：after_seconds、at、every_seconds、daily、weekly 或 cron",
        ));
    }
    let Some(selector) = present.first() else {
        return Ok(None);
    };
    let value = &args[*selector];
    let text = |object: &Value, key: &str| -> Result<String, ScheduleError> {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ScheduleError::new("invalid_rule", format!("{selector}.{key} 缺失")))
    };
    let seconds = |value: &Value| -> Result<u64, ScheduleError> {
        value
            .as_u64()
            .ok_or_else(|| ScheduleError::new("invalid_rule", format!("{selector} 必须是正整数秒")))
    };
    Ok(Some(match *selector {
        "after_seconds" => {
            let seconds = seconds(value)?;
            if seconds < 60 {
                return Err(ScheduleError::new(
                    "invalid_rule",
                    "after_seconds 至少为 60",
                ));
            }
            if seconds > 10 * 365 * 86400 {
                return Err(ScheduleError::new(
                    "invalid_rule",
                    "after_seconds 不能超过 10 年",
                ));
            }
            let target = service
                .now()
                .checked_add_signed(Duration::seconds(seconds as i64))
                .ok_or_else(|| ScheduleError::new("invalid_rule", "时间超出支持范围"))?;
            TaskRule::At {
                at: format_instant(target),
            }
        }
        "at" => TaskRule::At {
            at: value
                .as_str()
                .ok_or_else(|| ScheduleError::new("invalid_rule", "at 必须是时间字符串"))?
                .to_string(),
        },
        "every_seconds" => TaskRule::Every {
            every_seconds: seconds(value)?,
            anchor: String::new(),
        },
        "daily" => TaskRule::Daily {
            time: text(value, "time")?,
            time_zone: text(value, "time_zone")?,
        },
        "weekly" => TaskRule::Weekly {
            time: text(value, "time")?,
            weekdays: value
                .get("weekdays")
                .and_then(Value::as_array)
                .map(|days| {
                    days.iter()
                        .filter_map(Value::as_u64)
                        .map(|day| day.min(255) as u8)
                        .collect()
                })
                .unwrap_or_default(),
            time_zone: text(value, "time_zone")?,
        },
        _ => TaskRule::Cron {
            expression: text(value, "expression")?,
            time_zone: text(value, "time_zone")?,
        },
    }))
}

fn tool_error(error: ScheduleError) -> ToolBodyError {
    ToolBodyError::plain(format!("{}: {}", error.code, error.message))
}

fn output() -> ToolOutputDefinition {
    ToolOutputDefinition {
        schema: json!({"type": "object"}),
        render: Arc::new(|_, value| {
            Ok(vec![dsh_llm::ContentBlock::Text {
                text: value.to_string(),
            }])
        }),
        presentation_meta: None,
    }
}

fn session_of(run: &dsh_tools::ToolRunContext) -> Result<String, ToolBodyError> {
    let agent = run
        .execution
        .agent
        .clone()
        .ok_or_else(|| ToolBodyError::plain("定时任务需要在会话中创建"))?;
    if agent.session().header().origin.as_deref() == Some("subagent") {
        return Err(ToolBodyError::plain(
            "子智能体不能创建或管理定时任务，请在主会话中操作",
        ));
    }
    Ok(agent.id().as_str().to_string())
}

pub(crate) fn create_parameters() -> Value {
    let mut properties = timing_properties();
    properties.insert(
        "prompt".into(),
        json!({"type": "string", "minLength": 1, "maxLength": 8000}),
    );
    properties.insert("title".into(), json!({"type": "string", "maxLength": 120}));
    json!({"type": "object", "additionalProperties": false, "required": ["prompt"], "properties": properties})
}

pub(crate) fn list_parameters() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {}})
}

pub(crate) fn update_parameters() -> Value {
    let mut properties = timing_properties();
    properties.insert("id".into(), json!({"type": "string"}));
    properties.insert(
        "prompt".into(),
        json!({"type": "string", "minLength": 1, "maxLength": 8000}),
    );
    properties.insert("title".into(), json!({"type": "string", "maxLength": 120}));
    json!({"type": "object", "additionalProperties": false, "required": ["id"], "properties": properties})
}

pub(crate) fn delete_parameters() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["id"], "properties": {"id": {"type": "string"}}})
}

fn view_json(view: &TaskView) -> Value {
    serde_json::to_value(view).unwrap_or(Value::Null)
}

/// Register `scheduled_task_create`, `scheduled_task_list`, `scheduled_task_update` and
/// `scheduled_task_delete` on the global tool runtime.
pub fn register_tools(ctx: &cordis::Context, service: Arc<ScheduleService>) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("缺少工具运行时")?;
    let create_service = service.clone();
    tools.register(
        ctx,
        ToolDefinition {
            name: "scheduled_task_create".into(),
            description: CREATE_DESCRIPTION.into(),
            parameters: create_parameters(),
            output: output(),
            timeout_ms: Some(15_000),
            is_concurrency_safe: Some(Arc::new(|_| false)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, run| {
                let service = create_service.clone();
                let args = args.clone();
                let session = session_of(run);
                Box::pin(async move {
                    let session_id = session?;
                    let rule = rule_from_args(&args, &service)
                        .map_err(tool_error)?
                        .ok_or_else(|| {
                            ToolBodyError::plain("invalid_selector: 需要提供一个时间选择")
                        })?;
                    let view = service
                        .create(CreateTask {
                            session_id,
                            title: args
                                .get("title")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            prompt: args
                                .get("prompt")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            rule,
                            origin: TaskOrigin::Agent,
                        })
                        .await
                        .map_err(tool_error)?;
                    Ok(json!({"task": view_json(&view)}))
                })
            }),
        },
    )?;

    let list_service = service.clone();
    tools.register(
        ctx,
        ToolDefinition {
            name: "scheduled_task_list".into(),
            description: LIST_DESCRIPTION.into(),
            parameters: list_parameters(),
            output: output(),
            timeout_ms: Some(15_000),
            is_concurrency_safe: Some(Arc::new(|_| true)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, run| {
                let valid = args.as_object().is_some_and(|object| object.is_empty());
                let service = list_service.clone();
                let session = session_of(run);
                Box::pin(async move {
                    let session_id = session?;
                    if !valid {
                        return Err(ToolBodyError::plain("scheduled_task_list 不接受参数"));
                    }
                    let catalog = service.catalog(Some(&session_id));
                    Ok(json!({"tasks": catalog["tasks"], "now": catalog["now"]}))
                })
            }),
        },
    )?;

    let update_service = service.clone();
    tools.register(
        ctx,
        ToolDefinition {
            name: "scheduled_task_update".into(),
            description: UPDATE_DESCRIPTION.into(),
            parameters: update_parameters(),
            output: output(),
            timeout_ms: Some(15_000),
            is_concurrency_safe: Some(Arc::new(|_| false)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, run| {
                let service = update_service.clone();
                let args = args.clone();
                let session = session_of(run);
                Box::pin(async move {
                    let session_id = session?;
                    let rule = rule_from_args(&args, &service).map_err(tool_error)?;
                    let id = args.get("id").and_then(Value::as_str).unwrap_or_default();
                    let view = service
                        .update(
                            id,
                            Some(&session_id),
                            None,
                            UpdateTask {
                                title: args
                                    .get("title")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                prompt: args
                                    .get("prompt")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                rule,
                            },
                        )
                        .await
                        .map_err(tool_error)?;
                    Ok(json!({"task": view_json(&view)}))
                })
            }),
        },
    )?;

    let delete_service = service;
    tools.register(
        ctx,
        ToolDefinition {
            name: "scheduled_task_delete".into(),
            description: DELETE_DESCRIPTION.into(),
            parameters: delete_parameters(),
            output: output(),
            timeout_ms: Some(15_000),
            is_concurrency_safe: Some(Arc::new(|_| false)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, run| {
                let service = delete_service.clone();
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let session = session_of(run);
                Box::pin(async move {
                    let session_id = session?;
                    service
                        .delete(&id, Some(&session_id))
                        .await
                        .map_err(tool_error)?;
                    Ok(json!({"id": id, "deleted": true}))
                })
            }),
        },
    )?;
    Ok(())
}
