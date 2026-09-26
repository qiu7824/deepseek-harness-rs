//! Agent-scoped model tools backed by the Host reminder store.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use cordis::{Context, Disposer};
use dsh_agent::{Agent, AgentRegistry};
use dsh_llm::ContentBlock;
use dsh_scope::{scope_of, store::PreparedRegistration};
use dsh_tools::{ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};

use crate::host_service::ScheduleService;
use crate::host_types::{
    HostScheduleRecord, ScheduleError, ScheduleUpdateRequest, ScheduleUpdateResult, TaskStatus,
};

const INPUT_ERRORS: &[&str] = &[
    "invalid_prompt",
    "invalid_selector",
    "invalid_rule",
    "invalid_time_zone",
    "not_future",
    "time_out_of_range",
    "frequency_too_high",
];

fn internal_error() -> Value {
    json!({"code":"internal_error", "message":"The schedule operation failed."})
}

fn operation_error(error: ScheduleError) -> Value {
    if INPUT_ERRORS.contains(&error.code.as_str()) {
        json!({"code":error.code,"message":error.message})
    } else {
        internal_error()
    }
}

/// Public tool views never expose the internal Session binding or receipt store.
pub fn host_schedule_view(record: &HostScheduleRecord, now_ms: i64) -> Value {
    let mut value = serde_json::to_value(record).expect("schedule records are JSON");
    let due = chrono::DateTime::parse_from_rfc3339(record.scheduled_at())
        .is_ok_and(|instant| instant.timestamp_millis() <= now_ms);
    value["state"] = json!(if due { "overdue" } else { "scheduled" });
    value["deliveryMode"] = json!("host");
    value
}

fn validate_id(value: &Value, operation: &str) -> Result<String, ScheduleError> {
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return Err(ScheduleError::invalid(format!(
            "{operation} id must be non-empty without surrounding whitespace."
        )));
    };
    if id.is_empty() || crate::calendar::trim_js(id) != id {
        return Err(ScheduleError::invalid(format!(
            "{operation} id must be non-empty without surrounding whitespace."
        )));
    }
    Ok(id.to_owned())
}

struct UpdateFields {
    id: String,
    change: Option<Value>,
    title: Option<Value>,
    prompt: Option<Value>,
}

fn decode_update_fields(value: &Value) -> Result<UpdateFields, ScheduleError> {
    let invalid_selector = || {
        ScheduleError::new(
            "invalid_selector",
            "schedule_update accepts at most one of at, every_seconds, daily, weekly, or cron.",
        )
    };
    let object = value.as_object().ok_or_else(invalid_selector)?;
    let selectors = ["at", "every_seconds", "daily", "weekly", "cron"];
    if object.keys().any(|key| {
        !["id", "title", "prompt"].contains(&key.as_str()) && !selectors.contains(&key.as_str())
    }) {
        return Err(invalid_selector());
    }
    let supplied: Vec<_> = selectors
        .into_iter()
        .filter(|key| object.contains_key(*key))
        .collect();
    if supplied.len() > 1 {
        return Err(invalid_selector());
    }
    let id = validate_id(value, "schedule_update")?;
    if supplied.is_empty() && !object.contains_key("title") && !object.contains_key("prompt") {
        return Err(ScheduleError::new(
            "invalid_selector",
            "schedule_update needs a new title, prompt, or one of at, every_seconds, daily, weekly, or cron.",
        ));
    }
    for field in ["title", "prompt"] {
        if let Some(content) = object.get(field) {
            let content = content.as_str().ok_or_else(|| {
                ScheduleError::new(
                    "invalid_prompt",
                    format!("{field} must be a non-empty string."),
                )
            })?;
            if field == "title" {
                crate::calendar::schedule_title(content)?;
            } else if crate::calendar::trim_js(content).is_empty() {
                return Err(ScheduleError::new(
                    "invalid_prompt",
                    "prompt must be non-empty after trimming.",
                ));
            }
        }
    }
    if let Some(interval) = object.get("every_seconds") {
        let seconds = interval
            .as_i64()
            .or_else(|| {
                interval
                    .as_f64()
                    .filter(|number| {
                        number.is_finite()
                            && number.fract() == 0.0
                            && number.abs() <= 9_007_199_254_740_991.0
                    })
                    .map(|number| number as i64)
            })
            .filter(|seconds| seconds.unsigned_abs() <= 9_007_199_254_740_991)
            .ok_or_else(|| ScheduleError::invalid("every_seconds must be a safe integer."))?;
        if seconds < 60 {
            return Err(ScheduleError::new(
                "frequency_too_high",
                "every_seconds must be at least 60.",
            ));
        }
    }
    let change = supplied.first().map(|key| {
        let kind = if *key == "every_seconds" {
            "every"
        } else {
            key
        };
        let mut change = serde_json::Map::new();
        change.insert("kind".into(), json!(kind));
        change.insert((*key).into(), object[*key].clone());
        Value::Object(change)
    });
    Ok(UpdateFields {
        id,
        change,
        title: object.get("title").cloned(),
        prompt: object.get("prompt").cloned(),
    })
}

#[derive(Clone, Copy)]
enum Operation {
    Create,
    List,
    Update,
    Delete,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "schedule_create",
            Self::List => "schedule_list",
            Self::Update => "schedule_update",
            Self::Delete => "schedule_delete",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Create => "Create reminder",
            Self::List => "List reminders",
            Self::Update => "Update reminder",
            Self::Delete => "Delete reminder",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Create => {
                "Create a reminder in the current session that delivers prompt when it becomes due. Supply exactly one timing parameter: after_seconds, at, every_seconds, daily, weekly, or cron. Local times that do not exist in the zone are skipped; repeated local times fire once, at the earlier instant. After downtime, a recurring reminder delivers only its latest missed occurrence. Delivery can repeat after a crash."
            }
            Self::List => "List the active reminders in the current session.",
            Self::Update => {
                "Change a reminder in place, keeping its id. Supply a new title, prompt, or at most one timing parameter; omitted fields keep their stored values. To change a relative delay, create a new reminder."
            }
            Self::Delete => {
                "Delete a reminder in the current session, active or inactive. Deletion does not retract a reminder message that is already queued."
            }
        }
    }
}

/// Register a complete tool catalog in one exact root Agent scope.
/// The disposer revokes frozen definitions as well as the visible registrations.
pub fn register_host_schedule_tools(
    root_ctx: &Context,
    tool_ctx: &Context,
    agent: Arc<dyn Agent>,
    service: Arc<ScheduleService>,
) -> Result<Disposer, String> {
    let registry = root_ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or("schedule requires agents")?;
    let tools = tool_ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or("schedule requires tools")?;
    if scope_of(tool_ctx)
        .as_ref()
        .is_none_or(|scope| scope != agent.scope_key())
        || agent.session().header().origin.as_deref() == Some("subagent")
        || !registry
            .roots()
            .iter()
            .any(|root| Arc::ptr_eq(root, &agent))
    {
        return Err("schedule tools require the exact registered root Agent scope".into());
    }
    let active = Arc::new(AtomicBool::new(true));
    let mut prepared = Vec::new();
    for operation in [
        Operation::Create,
        Operation::List,
        Operation::Delete,
        Operation::Update,
    ] {
        let owner = agent.clone();
        let service = service.clone();
        let registry = registry.clone();
        let active_for_tool = active.clone();
        let definition = ToolDefinition {
            name: operation.name().into(),
            description: operation.description().into(),
            parameters: parameters_schema(operation),
            output: ToolOutputDefinition {
                schema: output_schema(operation),
                render: Arc::new(|_, value| {
                    Ok(vec![ContentBlock::Text {
                        text: serde_json::to_string(value).expect("JSON output"),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |arguments, exec| {
                let arguments = arguments.clone();
                let agent = owner.clone();
                let service = service.clone();
                let registry = registry.clone();
                let caller = exec.agent.clone();
                let signal = exec.signal.lock().clone();
                let active = active_for_tool.clone();
                Box::pin(async move {
                    if !caller
                        .as_ref()
                        .is_some_and(|caller| Arc::ptr_eq(caller, &agent))
                    {
                        return Ok(internal_error());
                    }
                    let cancelled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new({
                        let agent = agent.clone();
                        let service = service.clone();
                        move || {
                            signal()
                                || !active.load(Ordering::Acquire)
                                || !service.enabled()
                                || !registry
                                    .roots()
                                    .iter()
                                    .any(|root| Arc::ptr_eq(root, &agent))
                        }
                    });
                    if cancelled() {
                        return Ok(internal_error());
                    }
                    let session = agent.session().id().as_str();
                    let result = async {
                        match operation {
                            Operation::Create => {
                                let request = crate::calendar::decode_create_request(&arguments)?;
                                let record = service.create_with_signal(session, request, cancelled.clone()).await?;
                                Ok(host_schedule_view(&record, chrono::Utc::now().timestamp_millis()))
                            }
                            Operation::List => {
                                if arguments.as_object().is_none_or(|object| !object.is_empty()) { return Err(ScheduleError::invalid("schedule_list accepts no parameters.")); }
                                let records = service.list(session).await?;
                                if cancelled() { return Ok(internal_error()); }
                                let now = chrono::Utc::now().timestamp_millis();
                                Ok(Value::Array(records.iter().map(|record| host_schedule_view(record, now)).collect()))
                            }
                            Operation::Delete => {
                                if arguments.as_object().is_none_or(|object| object.keys().any(|key| key != "id")) { return Err(ScheduleError::invalid("schedule_delete accepts only id.")); }
                                let id = validate_id(&arguments, "schedule_delete")?;
                                service.delete_with_signal(session, &id, cancelled.clone()).await
                            }
                            Operation::Update => {
                                let fields = decode_update_fields(&arguments)?;
                                let task = service.get_task(session, &fields.id).await?;
                                if cancelled() { return Ok(internal_error()); }
                                let Some(task) = task else { return Ok(json!({"id":fields.id,"updated":false,"code":"schedule_not_found"})); };
                                if task.status == TaskStatus::Inactive { return Ok(json!({"id":fields.id,"updated":false,"code":"schedule_ended"})); }
                                let request = ScheduleUpdateRequest {
                                    session_id: session.to_owned(), id: fields.id,
                                    expected: serde_json::to_value(task.record).expect("JSON schedule record"),
                                    change: fields.change, title: fields.title, prompt: fields.prompt,
                                };
                                match service.update_with_signal(request, cancelled.clone()).await? {
                                    ScheduleUpdateResult::Changed { record, .. } => Ok(host_schedule_view(&record, chrono::Utc::now().timestamp_millis())),
                                    missing @ ScheduleUpdateResult::Miss { .. } => Ok(serde_json::to_value(missing).expect("JSON schedule result")),
                                }
                            }
                        }
                    }.await;
                    Ok(result.unwrap_or_else(operation_error))
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(move |arguments| {
                Some(ToolCallView::Generic {
                    title: operation.title().into(),
                    kind: Some(if matches!(operation, Operation::List) {
                        ToolCallKind::Read
                    } else {
                        ToolCallKind::Other
                    }),
                    raw_input: if matches!(operation, Operation::List) {
                        None
                    } else {
                        arguments
                            .get(if matches!(operation, Operation::Create) {
                                "prompt"
                            } else {
                                "id"
                            })
                            .cloned()
                    },
                    content: None,
                    locations: None,
                })
            })),
            present_result: None,
        };
        prepared.push(tools.prepare_register_arc(tool_ctx, Arc::new(definition))?);
    }
    let registration = PreparedRegistration::commit_all(tool_ctx, "schedule.hostTools()", prepared);
    let disposer = cordis::events::make_disposer(move || {
        active.store(false, Ordering::Release);
        let registration = registration.clone();
        Box::pin(async move { registration().await })
    });
    // Scope disposal must invalidate bodies which were resolved before unload.
    Ok(tool_ctx.effect(
        "schedule.hostTools.owner()",
        Box::pin(async move { Some(disposer) }),
    ))
}

fn selectors_schema() -> serde_json::Map<String, Value> {
    let zone = json!({"type":"string","description":"UTC or IANA Area/Location, for example Asia/Shanghai."});
    let time =
        json!({"type":"string","description":"HH:mm:ss with optional 1-3 fractional digits."});
    serde_json::from_value(json!({
        "every_seconds":{"type":"number","description":"Fixed-rate interval in whole seconds, at least 60; changing it re-aligns to save time."},
        "daily":{"type":"object","additionalProperties":false,"properties":{"time":time,"time_zone":zone},"required":["time","time_zone"]},
        "weekly":{"type":"object","additionalProperties":false,"properties":{"time":time,"time_zone":zone,"weekdays":{"type":"array","items":{"type":"integer"},"description":"ISO weekdays Monday 1 through Sunday 7, without repetitions."}},"required":["time","time_zone","weekdays"]},
        "cron":{"type":"object","additionalProperties":false,"properties":{"expression":{"type":"string","description":"Five-field Vixie cron: minute hour day-of-month month day-of-week. Restricted day fields use OR."},"time_zone":zone},"required":["expression","time_zone"]},
        "at":{"description":"RFC 3339 with offset, or an explicit local date/time and IANA zone.","oneOf":[{"type":"string"},{"type":"object","additionalProperties":false,"properties":{"date":{"type":"string"},"time":time,"time_zone":zone},"required":["date","time","time_zone"]}]}
    })).expect("selector schema object")
}

fn parameters_schema(operation: Operation) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    if matches!(operation, Operation::Create | Operation::Update) {
        properties = selectors_schema();
        properties.insert(
            "title".into(),
            json!({"type":"string","description":"Task name of at most 120 characters."}),
        );
        properties.insert(
            "prompt".into(),
            json!({"type":"string","description":"Reminder content to present when due."}),
        );
    }
    if matches!(operation, Operation::Create) {
        properties.insert(
            "after_seconds".into(),
            json!({"type":"number","description":"Positive delay in whole seconds."}),
        );
        required.extend(["title", "prompt"]);
    }
    if matches!(operation, Operation::Delete | Operation::Update) {
        properties.insert(
            "id".into(),
            json!({"type":"string","description":"Exact schedule id returned by schedule_list."}),
        );
        required.push("id");
    }
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}

fn view_schema() -> Value {
    let alternatives: Vec<_> = ["after","at","every","daily","weekly","cron"].into_iter().map(|kind| {
        let mut properties = serde_json::from_value::<serde_json::Map<String, Value>>(json!({
            "id":{"type":"string"},"title":{"type":"string"},"prompt":{"type":"string"},"scheduledAt":{"type":"string"},
            "kind":{"type":"string","const":kind},"state":{"type":"string","enum":["scheduled","overdue"]},"deliveryMode":{"type":"string","const":"host"}
        })).unwrap();
        match kind {
            "after" => { properties.insert("afterSeconds".into(), json!({"type":"integer"})); }
            "every" => { properties.insert("everySeconds".into(), json!({"type":"integer"})); }
            "daily" | "weekly" | "cron" => {
                properties.insert("timeZone".into(), json!({"type":"string"}));
                properties.insert(if kind == "cron" { "expression" } else { "time" }.into(), json!({"type":"string"}));
                if kind == "weekly" { properties.insert("weekdays".into(), json!({"type":"array","items":{"type":"integer"}})); }
            }
            _ => {}
        }
        let required: Vec<_> = properties.keys().cloned().collect();
        json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
    }).collect();
    json!({"oneOf":alternatives})
}

fn output_schema(operation: Operation) -> Value {
    let mut options = match operation {
        Operation::Create => vec![view_schema()],
        Operation::List => vec![json!({"type":"array","items":view_schema()})],
        Operation::Update => vec![
            view_schema(),
            json!({"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"updated":{"type":"boolean","const":false},"code":{"type":"string","enum":["schedule_not_found","schedule_ended","schedule_conflict"]}},"required":["id","updated","code"]}),
        ],
        Operation::Delete => vec![
            json!({"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"deleted":{"type":"boolean","const":true}},"required":["id","deleted"]}),
            json!({"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"deleted":{"type":"boolean","const":false},"code":{"type":"string","const":"schedule_not_found"}},"required":["id","deleted","code"]}),
        ],
    };
    for code in INPUT_ERRORS.iter().copied().chain(["internal_error"]) {
        options.push(json!({"type":"object","additionalProperties":false,"properties":{"code":{"type":"string","const":code},"message":{"type":"string"}},"required":["code","message"]}));
    }
    json!({"oneOf":options})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_contract_is_supported_and_every_record_shape_is_closed() {
        for operation in [
            Operation::Create,
            Operation::List,
            Operation::Update,
            Operation::Delete,
        ] {
            dsh_tools::assert_supported_json_schema(&parameters_schema(operation)).unwrap();
            dsh_tools::assert_supported_json_schema(&output_schema(operation)).unwrap();
        }
        for (kind, extra) in [
            ("after", json!({"afterSeconds":60})),
            ("at", json!({})),
            ("every", json!({"everySeconds":60})),
            ("daily", json!({"time":"09:00:00.000","timeZone":"UTC"})),
            (
                "weekly",
                json!({"time":"09:00:00.000","timeZone":"UTC","weekdays":[1,5]}),
            ),
            ("cron", json!({"expression":"0 9 * * 1-5","timeZone":"UTC"})),
        ] {
            let mut value = json!({"id":"schedule-a","title":"Reminder","prompt":"Review","scheduledAt":"2026-09-01T00:00:00.000Z","kind":kind,"state":"scheduled","deliveryMode":"host"});
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(
                dsh_tools::validate_json_schema_value(&view_schema(), &value, "result").is_empty(),
                "{value}"
            );
            value["sessionId"] = json!("not-a-model-field");
            assert!(
                !dsh_tools::validate_json_schema_value(&view_schema(), &value, "result").is_empty()
            );
        }
    }

    #[test]
    fn update_rejects_cross_session_and_observation_injection() {
        for field in [
            "sessionId",
            "session_id",
            "expected",
            "after_seconds",
            "change",
        ] {
            let mut value = json!({"id":"schedule-a","title":"name"});
            value[field] = json!("injected");
            assert_eq!(
                decode_update_fields(&value).err().unwrap().code,
                "invalid_selector"
            );
        }
        assert!(
            decode_update_fields(&json!({"id":"schedule-a","title":"name"}))
                .unwrap()
                .change
                .is_none()
        );
        assert_eq!(
            decode_update_fields(&json!({"id":"schedule-a","every_seconds":60}))
                .unwrap()
                .change,
            Some(json!({"kind":"every","every_seconds":60}))
        );
    }

    #[test]
    fn update_validates_presence_precision_and_title_utf16_length() {
        for value in [
            json!({"id":"schedule-a"}),
            json!({"id":"schedule-a","title":null}),
            json!({"id":" schedule-a","title":"name"}),
            json!({"id":"schedule-a","every_seconds":60.5}),
            json!({"id":"schedule-a","every_seconds":9_007_199_254_740_992u64}),
            json!({"id":"schedule-a","daily":{},"weekly":{}}),
            json!({"id":"schedule-a","title":"😀".repeat(61)}),
        ] {
            assert!(decode_update_fields(&value).is_err(), "{value}");
        }
        assert!(decode_update_fields(&json!({"id":"schedule-a","title":"😀".repeat(60)})).is_ok());
        assert!(decode_update_fields(&json!({"id":"schedule-a","every_seconds":60.0})).is_ok());
        assert_eq!(
            decode_update_fields(&json!({"id":"schedule-a","every_seconds":59}))
                .err()
                .unwrap()
                .code,
            "frequency_too_high"
        );
    }

    #[test]
    fn views_are_host_owned_and_output_keeps_storage_details_private() {
        let record = HostScheduleRecord::At {
            id: "schedule-a".into(),
            title: "Reminder".into(),
            prompt: "Review".into(),
            scheduled_at: "2026-09-01T00:00:00.000Z".into(),
        };
        let instant = chrono::DateTime::parse_from_rfc3339(record.scheduled_at())
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            host_schedule_view(&record, instant - 1)["state"],
            "scheduled"
        );
        let value = host_schedule_view(&record, instant);
        assert_eq!(value["state"], "overdue");
        assert_eq!(value["deliveryMode"], "host");
        assert!(value.get("sessionId").is_none());
        assert_eq!(
            operation_error(ScheduleError::new("io_error", "C:/secret/path")),
            internal_error()
        );
        assert_eq!(view_schema()["oneOf"].as_array().unwrap().len(), 6);
    }
}
