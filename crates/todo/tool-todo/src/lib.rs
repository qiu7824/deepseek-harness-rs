//! Model-facing whole-list replacement. Each call appends a `todo/write`
//! snapshot to the calling agent's session; replay is last-write-wins, and
//! UIs render from session events. A non-agent caller has no owning list and
//! is rejected.
//! Rust port of `packages/todo/tool-todo/src/index.ts` (+ `types.ts`).
//!
//! # Deviations
//!
//! - The Rust tool runtime does not yet validate arguments against the tool
//!   parameters schema before dispatch, so the body validates its own input
//!   with the shared JSON Schema engine first (same rejection surface, run
//!   one stage later).
//! - The `todos` projection unit is registered through
//!   `ctx.inject(["sessionProjections"])` when the seam is composed, like the
//!   TS child.

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, Plugin, PluginError};
use dsh_session::{Session, TodoItem, TodoStatus, todo_write_data};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition,
    ToolRunContext, ToolRuntime, validate_json_schema_value,
};

pub use dsh_session::TodoItem as TodoItemType;
pub use dsh_session::TodoStatus as TodoStatusType;

/// Compare-and-swap failure for a user-authored whole-list replacement.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplaceTodosError {
    Conflict { current: Vec<TodoItem> },
    Invalid(String),
    Append(String),
}

fn current_todos(events: &[dsh_session::SessionEvent]) -> Vec<TodoItem> {
    events
        .iter()
        .rev()
        .take_while(|event| event.type_ != "turn/start")
        .find(|event| event.type_ == "todo/write")
        .and_then(|event| event.data.get("todos"))
        .and_then(serde_json::Value::as_array)
        .and_then(|raw| to_todo_list(raw, true).ok())
        .unwrap_or_default()
}

pub fn replace_if_current(
    session: &Session,
    expected: &[TodoItem],
    replacement: &[TodoItem],
    allow_parallel_in_progress: bool,
) -> Result<dsh_session::SessionEvent, ReplaceTodosError> {
    let raw = replacement
        .iter()
        .map(|todo| {
            serde_json::json!({
                "content": todo.content,
                "status": status_str(todo.status),
            })
        })
        .collect::<Vec<_>>();
    let replacement =
        to_todo_list(&raw, allow_parallel_in_progress).map_err(ReplaceTodosError::Invalid)?;
    let data = todo_write_data(&replacement);
    let expected = expected.to_vec();
    match session.append_if("todo/write", data, None, move |events| {
        current_todos(events) == expected
    }) {
        Ok(Some(event)) => Ok(event),
        Ok(None) => Err(ReplaceTodosError::Conflict {
            current: current_todos(&session.events()),
        }),
        Err(message) => Err(ReplaceTodosError::Append(message)),
    }
}

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "tool-todo";

/// The tool registry service this plugin registers into.
pub const INJECT: [&str; 1] = ["tools"];

/// Model-facing todo tool configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Whether several todos may be `in_progress` at once.
    pub allow_parallel_in_progress: bool,
}

/// The valid [`TodoItem`] statuses, as wire strings.
const STATUSES: [&str; 3] = ["pending", "in_progress", "completed"];

const DESCRIPTION_HEAD: &str = "Record and update a structured task list for the current work. Send the ENTIRE \
     list every call — it REPLACES the previous list (there are no partial updates, \
     no per-item edits). Use it to plan multi-step work and show progress: add one \
     todo per concrete step before you start. ";

const DESCRIPTION_PARALLEL: &str = "Mark every todo being actively worked \
     on `in_progress` — several at once when work genuinely runs in parallel (e.g. \
     concurrent subagents or background commands), one for sequential work; while \
     work remains, at least one task should be `in_progress`. ";

const DESCRIPTION_SINGLE: &str = "Keep AT MOST ONE todo `in_progress` at a \
     time; while work remains, exactly one active task should be `in_progress`. ";

const DESCRIPTION_TAIL: &str = "Mark a todo \
     `completed` the moment it is done (do not batch completions), and allow no \
     `in_progress` item only once all work is complete. Skip the list for trivial \
     single-step tasks. Statuses: `pending` (not started), `in_progress` (being \
     worked on now), `completed` (finished).";

/// The model-facing description for one activation (TS `describe`).
pub fn describe(allow_parallel: bool) -> String {
    format!(
        "{}{}{}",
        DESCRIPTION_HEAD,
        if allow_parallel {
            DESCRIPTION_PARALLEL
        } else {
            DESCRIPTION_SINGLE
        },
        DESCRIPTION_TAIL
    )
}

fn status_str(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::Completed => "completed",
    }
}

fn status_from_str(value: &str) -> Option<TodoStatus> {
    match value {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        _ => None,
    }
}

/// Validate the value constraints the parameters schema cannot express and
/// build the canonical list (TS `toTodoList`).
pub fn to_todo_list(
    raw: &[serde_json::Value],
    allow_parallel: bool,
) -> Result<Vec<TodoItem>, String> {
    let mut todos: Vec<TodoItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut active = 0;
    for item in raw {
        let content = item
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "invalid todo: `content` must be a non-empty string".to_string())?
            .trim()
            .to_string();
        if content.is_empty() {
            return Err("invalid todo: `content` must be a non-empty string".to_string());
        }
        if seen.contains(&content) {
            return Err(format!(
                "invalid todos: duplicate content {}",
                serde_json::to_string(&content).expect("content")
            ));
        }
        seen.insert(content.clone());
        let status = item
            .get("status")
            .and_then(|value| value.as_str())
            .and_then(status_from_str)
            .ok_or_else(|| "invalid todo: unknown status".to_string())?;
        if status == TodoStatus::InProgress {
            active += 1;
        }
        todos.push(TodoItem { content, status });
    }
    if !allow_parallel && active > 1 {
        return Err(format!(
            "invalid todos: at most one task may be in_progress (got {active})"
        ));
    }
    Ok(todos)
}

/// The wire JSON Schema of the `todo_write` arguments (the compiled form of
/// the TS ParameterSchemaSpec DSL).
fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "content": { "type": "string" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                    },
                    "required": ["content", "status"]
                }
            }
        },
        "required": ["todos"]
    })
}

/// The wire JSON Schema of the tool's canonical output value.
fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "content": { "type": "string" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                    },
                    "required": ["content", "status"]
                }
            },
            "counts": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "pending": { "type": "integer" },
                    "inProgress": { "type": "integer" },
                    "completed": { "type": "integer" }
                },
                "required": ["pending", "inProgress", "completed"]
            }
        },
        "required": ["todos", "counts"]
    })
}

/// Validate the `todos` projection view payload (whole list or null).
pub fn todos_schema(value: &ArcValue) -> Result<serde_json::Value, String> {
    let value: &serde_json::Value = cordis::downcast(value)
        .ok_or_else(|| "todos view must produce a JSON value".to_string())?;
    if value.is_null() {
        return Ok(value.clone());
    }
    let items = value
        .as_array()
        .ok_or_else(|| "todos must be an array or null".to_string())?;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| "todo entries must be objects".to_string())?;
        if object.len() != 2 {
            return Err("todo entries must carry exactly content and status".to_string());
        }
        let content = object
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "todo content must be a string".to_string())?;
        if content.is_empty() || content.trim() != content {
            return Err("todo content must be non-empty and already trimmed".to_string());
        }
        if !seen.insert(content.to_string()) {
            return Err(format!(
                "todo/write repeats content {}",
                serde_json::to_string(content).expect("content")
            ));
        }
        let status = object
            .get("status")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "todo status must be a string".to_string())?;
        if !STATUSES.contains(&status) {
            return Err(format!(
                "todo/write carries unknown status {}",
                serde_json::to_string(status).expect("status")
            ));
        }
    }
    Ok(value.clone())
}

/// The `todos` projection unit (TS `todosProjectionSchema` + the fold).
pub fn todos_projection() -> dsh_session_projection::ProjectionDefinition {
    dsh_session_projection::ProjectionDefinition {
        key: "todos".to_string(),
        schema: Arc::new(todos_schema),
        init: Arc::new(|| cordis::arc(serde_json::Value::Null)),
        apply: Arc::new(|state: &ArcValue, event: &dsh_session::SessionEvent| {
            if event.type_ == "todo/write" {
                cordis::arc(
                    event
                        .data
                        .get("todos")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
            } else if event.type_ == "turn/start" {
                cordis::arc(serde_json::Value::Null)
            } else {
                state.clone()
            }
        }),
        view: Arc::new(|state: &ArcValue| state.clone()),
        state_version: 2,
    }
}

/// Register the `todo_write` tool on `ctx.tools` and, when the
/// session-projection seam is composed, the `todos` unit (TS `apply`).
pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    let allow_parallel = config.allow_parallel_in_progress;

    // The unit child activates only when a projection registry is composed.
    let projection_fiber =
        ctx.inject(
            cordis::InjectSpec::new(["sessionProjections"]),
            Arc::new(move |type_ctx: &Context, _config: ArcValue| {
                let type_ctx = type_ctx.clone();
                Box::pin(async move {
                    if let Some(registry) = type_ctx
                        .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                            "sessionProjections",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    {
                        let disposer = registry.register(&type_ctx, todos_projection()).map_err(
                            |message| PluginError::from(anyhow::anyhow!("tool-todo: {message}")),
                        )?;
                        let _ = type_ctx.effect(
                            "tool-todo projection",
                            Box::pin(async move { Some(disposer) }),
                        );
                    }
                    Ok(())
                })
            }),
        );

    let description = describe(allow_parallel);
    let definition = ToolDefinition {
        name: "todo_write".to_string(),
        description,
        parameters: parameters_schema(),
        output: ToolOutputDefinition {
            schema: output_schema(),
            render: Arc::new(|_args, value| {
                let counts = &value["counts"];
                let pending = counts["pending"].as_i64().unwrap_or(0);
                let in_progress = counts["inProgress"].as_i64().unwrap_or(0);
                let completed = counts["completed"].as_i64().unwrap_or(0);
                Ok(vec![dsh_llm::ContentBlock::Text {
                    text: format!(
                        "Updated todo list: {pending} pending, {in_progress} in progress, {completed} completed."
                    ),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args: &serde_json::Value, exec: &ToolRunContext| {
            let allow_parallel = allow_parallel;
            let args = args.clone();
            let agent = exec.agent.clone();
            Box::pin(async move {
                // The TS registry rejects schema-level violations before the
                // body runs; the Rust runtime delegates that gate to the
                // body (documented deviation) using the same engine.
                let violations =
                    validate_json_schema_value(&parameters_schema(), &args, "arguments");
                if !violations.is_empty() {
                    return Err(ToolBodyError::plain(violations.join("; ")));
                }
                let raw: Vec<serde_json::Value> =
                    args["todos"].as_array().cloned().unwrap_or_default();
                let todos = to_todo_list(&raw, allow_parallel).map_err(ToolBodyError::plain)?;
                let agent = agent.ok_or_else(|| {
                    ToolBodyError::plain("todo_write requires an owning agent session")
                })?;
                agent
                    .session()
                    .append("todo/write", todo_write_data(&todos), None)
                    .map_err(ToolBodyError::plain)?;
                let count = |status: TodoStatus| -> i64 {
                    todos.iter().filter(|todo| todo.status == status).count() as i64
                };
                Ok(serde_json::json!({
                    "todos": todos.iter().map(|todo| serde_json::json!({
                        "content": todo.content,
                        "status": status_str(todo.status),
                    })).collect::<Vec<_>>(),
                    "counts": {
                        "pending": count(TodoStatus::Pending),
                        "inProgress": count(TodoStatus::InProgress),
                        "completed": count(TodoStatus::Completed),
                    },
                }))
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args: &serde_json::Value| {
            Some(ToolCallView::Generic {
                title: "Update todo list".to_string(),
                kind: Some(ToolCallKind::Other),
                raw_input: Some(
                    args.get("todos")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ),
                content: None,
                locations: None,
            })
        })),
        present_result: None,
    };

    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-todo requires the tools service".to_string())?;
    let tool_disposer = tools
        .register(ctx, definition)
        .map_err(|message| format!("tool-todo: {message}"))?;

    Ok(cordis::make_disposer(move || {
        let tool_disposer = tool_disposer.clone();
        let projection_fiber = projection_fiber.clone();
        Box::pin(async move {
            projection_fiber.dispose().await;
            tool_disposer().await;
        })
    }))
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `Config`,
/// `apply`).
pub struct ToolTodoPlugin;

#[async_trait::async_trait]
impl Plugin for ToolTodoPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = if let Some(config) = config.downcast_ref::<Config>() {
            config.clone()
        } else if let Some(value) = config.downcast_ref::<serde_json::Value>() {
            Config {
                allow_parallel_in_progress: value
                    .get("allowParallelInProgress")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            }
        } else {
            Config {
                allow_parallel_in_progress: false,
            }
        };
        let disposer =
            apply(ctx, &config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("tool-todo", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status,
        }
    }

    fn session() -> Session {
        Session::create(dsh_session::session_id("todo-service-test"), None, None)
            .expect("detached test session")
    }

    fn current(session: &Session) -> Vec<TodoItem> {
        current_todos(&session.events())
    }

    #[test]
    fn compare_and_swap_replaces_the_current_list() {
        let session = session();
        let initial = vec![
            item("implement", TodoStatus::InProgress),
            item("verify", TodoStatus::Pending),
        ];
        replace_if_current(&session, &[], &initial, false).expect("initial replacement");
        let edited = vec![
            item("implement UI", TodoStatus::InProgress),
            item("verify", TodoStatus::Pending),
        ];
        let event =
            replace_if_current(&session, &initial, &edited, false).expect("matching replacement");
        assert_eq!(event.type_, "todo/write");
        assert_eq!(current(&session), edited);
    }

    #[test]
    fn compare_and_swap_rejects_a_stale_snapshot() {
        let session = session();
        let current_list = vec![item("current", TodoStatus::InProgress)];
        replace_if_current(&session, &[], &current_list, false).expect("initial replacement");
        let error = replace_if_current(
            &session,
            &[item("stale", TodoStatus::InProgress)],
            &[item("replacement", TodoStatus::InProgress)],
            false,
        )
        .expect_err("stale replacement must fail");
        assert_eq!(
            error,
            ReplaceTodosError::Conflict {
                current: current_list.clone(),
            }
        );
        assert_eq!(current(&session), current_list);
    }

    #[test]
    fn a_new_turn_resets_the_current_list() {
        let session = session();
        replace_if_current(
            &session,
            &[],
            &[item("previous turn", TodoStatus::InProgress)],
            false,
        )
        .expect("initial replacement");
        session
            .append("turn/start", serde_json::json!({ "turn": 1 }), None)
            .expect("turn start");
        assert!(current(&session).is_empty());
        replace_if_current(
            &session,
            &[],
            &[item("new turn", TodoStatus::InProgress)],
            false,
        )
        .expect("new turn replacement");
    }

    #[test]
    fn compare_and_swap_reuses_tool_validation() {
        let session = session();
        let invalid = vec![
            item("first", TodoStatus::InProgress),
            item("second", TodoStatus::InProgress),
        ];
        let error = replace_if_current(&session, &[], &invalid, false)
            .expect_err("parallel active todos must fail");
        assert!(matches!(error, ReplaceTodosError::Invalid(_)));
        assert!(current(&session).is_empty());
    }
}
