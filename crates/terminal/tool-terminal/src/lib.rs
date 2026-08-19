use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use cordis::Context;
use dsh_jobs::{JobHooks, JobOutcome, JobOutcomeStatus, JobRegistry, JobStart};
use dsh_llm::ContentBlock;
use dsh_terminal::{
    TerminalReadRequest, TerminalSendOperation, TerminalSendRequest, TerminalSessionService,
    TerminalSessionStatus, TerminalSignal, TerminalSpawnRequest, terminal_session_id,
};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRunContext, ToolRuntime};
use futures::future::BoxFuture;

struct TerminalSendJob {
    operation: Arc<dyn TerminalSendOperation>,
    cancel_requested: Arc<AtomicBool>,
}

impl JobHooks for TerminalSendJob {
    fn cancel(&self, _reason: Option<String>) {
        self.cancel_requested.store(true, SeqCst);
        self.operation.cancel();
    }

    fn done(&self) -> BoxFuture<'static, JobOutcome> {
        let operation = self.operation.clone();
        let cancel_requested = self.cancel_requested.clone();
        Box::pin(async move {
            let result = operation.done().await;
            JobOutcome {
                status: if cancel_requested.load(SeqCst) {
                    JobOutcomeStatus::Killed
                } else {
                    JobOutcomeStatus::Completed
                },
                detail: Some(format!("wait: {}", result.wait_reason.as_str())),
                output: None,
            }
        })
    }

    fn read_output(&self) -> Option<String> {
        let read = self.operation.read_output();
        let mut text = read.delta;
        if read.truncated {
            text.push_str("\n[output truncated]");
        }
        Some(text)
    }
}

pub struct ToolTerminalService;

impl ToolTerminalService {
    pub fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-terminal requires tools".to_string())?;
        let terminals = ctx
            .get_typed::<Arc<TerminalSessionService>>("terminals", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-terminal requires terminals".to_string())?;
        let jobs = ctx
            .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-terminal requires jobs".to_string())?;

        let open_terminals = terminals.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_open".to_string(),
                description: "Create an owner-isolated persistent terminal session.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "type": { "type": "string" },
                        "name": { "type": "string" },
                        "cwd": { "type": "string" }
                    },
                    "required": ["type"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "sessionId": { "type": "string" },
                            "name": { "oneOf": [{ "type": "string" }, { "type": "null" }] },
                            "type": { "type": "string" },
                            "pid": { "oneOf": [{ "type": "integer" }, { "type": "null" }] },
                            "status": { "type": "object" },
                            "motd": { "type": "string" }
                        },
                        "required": ["sessionId", "name", "type", "pid", "status", "motd"]
                    }),
                    render: Arc::new(|_args, value| {
                        let id = value["sessionId"].as_str().unwrap_or_default();
                        let motd = value["motd"].as_str().unwrap_or_default();
                        Ok(vec![ContentBlock::Text {
                            text: format!("opened terminal session {id}\n{motd}"),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let terminals = open_terminals.clone();
                    let args = args.clone();
                    let owner = run.execution.agent.clone();
                    let signal = run.execution.signal.lock().clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        let type_ = args
                            .get("type")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| ToolBodyError::plain("type must be non-empty"))?;
                        let request = TerminalSpawnRequest {
                            type_: type_.to_string(),
                            name: args
                                .get("name")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                            cwd: args
                                .get("cwd")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                        };
                        let created = terminals
                            .spawn(owner, request, Some(signal))
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?
                            .await
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                        Ok(serde_json::json!({
                            "sessionId": created.session_id.as_str(),
                            "name": created.name,
                            "type": created.type_,
                            "pid": created.pid,
                            "status": status_json(&created.status),
                            "motd": created.motd,
                        }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;

        let send_terminals = terminals.clone();
        let send_jobs = jobs.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_send".to_string(),
                description: "Send text to a persistent terminal and wait for bounded readiness."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "sessionId": { "type": "string" },
                        "text": { "type": "string" },
                        "submit": { "type": "boolean" },
                        "run_in_background": { "type": "boolean" }
                    },
                    "required": ["sessionId", "text"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string" },
                            "jobId": { "type": "string" },
                            "viewport": { "type": "string" },
                            "waitReason": { "type": "string" },
                            "sessionStatus": { "type": "object" },
                            "truncated": { "type": "boolean" }
                        },
                        "required": ["kind"]
                    }),
                    render: Arc::new(|_args, value| {
                        let text = if value["kind"] == "background" {
                            format!(
                                "started background job {}",
                                value["jobId"].as_str().unwrap_or_default()
                            )
                        } else {
                            value["viewport"].as_str().unwrap_or_default().to_string()
                        };
                        Ok(vec![ContentBlock::Text { text }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let terminals = send_terminals.clone();
                    let jobs = send_jobs.clone();
                    let args = args.clone();
                    let owner = run.execution.agent.clone();
                    let signal = run.execution.signal.lock().clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        let id = required_session_id(&args)?;
                        let text = args
                            .get("text")
                            .and_then(|value| value.as_str())
                            .ok_or_else(|| ToolBodyError::plain("text must be a string"))?
                            .to_string();
                        let submit = args
                            .get("submit")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(true);
                        if args
                            .get("run_in_background")
                            .and_then(|value| value.as_bool())
                            == Some(true)
                        {
                            let job_terminals = terminals.clone();
                            let job_owner = owner.clone();
                            let job_session = id.clone();
                            let job_text = text.clone();
                            let label = format!("{}: {}", id.as_str(), text);
                            let job_id = jobs
                                .start(JobStart {
                                    kind: "pty-send".to_string(),
                                    label,
                                    output_limit_bytes: None,
                                    owner: Some(owner.clone()),
                                    run: Arc::new(move || {
                                        let operation = job_terminals
                                            .start_send(
                                                &job_owner,
                                                &job_session,
                                                TerminalSendRequest {
                                                    text: job_text.clone(),
                                                    submit,
                                                    signal: None,
                                                },
                                            )
                                            .unwrap_or_else(|error| {
                                                panic!("background terminal send failed: {error}")
                                            });
                                        Arc::new(TerminalSendJob {
                                            operation,
                                            cancel_requested: Arc::new(AtomicBool::new(false)),
                                        })
                                    }),
                                })
                                .map_err(ToolBodyError::plain)?;
                            return Ok(serde_json::json!({
                                "kind": "background",
                                "jobId": job_id.as_str(),
                            }));
                        }
                        let operation = terminals
                            .start_send(
                                &owner,
                                &id,
                                TerminalSendRequest {
                                    text,
                                    submit,
                                    signal: Some(signal),
                                },
                            )
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                        let result = operation.done().await;
                        Ok(serde_json::json!({
                            "kind": "foreground",
                            "viewport": result.viewport,
                            "waitReason": result.wait_reason.as_str(),
                            "sessionStatus": status_json(&result.session_status),
                            "truncated": result.truncated,
                        }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;

        let signal_terminals = terminals.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_signal".to_string(),
                description: "Signal the foreground process group of a persistent terminal."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "sessionId": { "type": "string" },
                        "signal": { "type": "string" }
                    },
                    "required": ["sessionId", "signal"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "delivered": { "type": "boolean" },
                            "targetPgid": { "type": "integer" }
                        },
                        "required": ["delivered", "targetPgid"]
                    }),
                    render: Arc::new(|args, value| {
                        Ok(vec![ContentBlock::Text {
                            text: format!(
                                "delivered {} to foreground process group {}",
                                args["signal"].as_str().unwrap_or_default(),
                                value["targetPgid"].as_u64().unwrap_or_default()
                            ),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let terminals = signal_terminals.clone();
                    let args = args.clone();
                    let owner = run.execution.agent.clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        let id = required_session_id(&args)?;
                        let signal = parse_signal(
                            args.get("signal")
                                .and_then(|value| value.as_str())
                                .unwrap_or_default(),
                        )?;
                        let result = terminals
                            .signal(&owner, &id, signal)
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?
                            .await
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                        Ok(serde_json::json!({
                            "delivered": result.delivered,
                            "targetPgid": result.target_pgid,
                        }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;

        let read_terminals = terminals.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_read".to_string(),
                description: "Read bounded retained output from a persistent terminal.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "sessionId": { "type": "string" },
                        "offset": { "type": "integer" },
                        "count": { "type": "integer" }
                    },
                    "required": ["sessionId"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "text": { "type": "string" },
                            "totalLines": { "type": "integer" },
                            "lineBegin": { "type": "integer" },
                            "lineEnd": { "type": "integer" },
                            "truncated": { "type": "boolean" }
                        },
                        "required": ["text", "totalLines", "lineBegin", "lineEnd", "truncated"]
                    }),
                    render: Arc::new(|_args, value| {
                        Ok(vec![ContentBlock::Text {
                            text: value["text"].as_str().unwrap_or_default().to_string(),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let terminals = read_terminals.clone();
                    let args = args.clone();
                    let owner = run.execution.agent.clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        let id = required_session_id(&args)?;
                        let result = terminals
                            .read(
                                &owner,
                                &id,
                                TerminalReadRequest {
                                    offset: args.get("offset").and_then(|value| value.as_u64()),
                                    count: args.get("count").and_then(|value| value.as_u64()),
                                },
                            )
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                        Ok(serde_json::json!({
                            "text": result.text,
                            "totalLines": result.total_lines,
                            "lineBegin": result.line_begin,
                            "lineEnd": result.line_end,
                            "truncated": result.truncated,
                        }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;

        let close_terminals = terminals.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_close".to_string(),
                description: "Close one persistent terminal and await its process tree."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": { "sessionId": { "type": "string" } },
                    "required": ["sessionId"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "sessionId": { "type": "string" },
                            "outcome": { "type": "string" }
                        },
                        "required": ["sessionId", "outcome"]
                    }),
                    render: Arc::new(|_args, value| {
                        Ok(vec![ContentBlock::Text {
                            text: format!(
                                "{} terminal session {}",
                                value["outcome"].as_str().unwrap_or_default(),
                                value["sessionId"].as_str().unwrap_or_default()
                            ),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let terminals = close_terminals.clone();
                    let args = args.clone();
                    let owner = run.execution.agent.clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        let id = required_session_id(&args)?;
                        let closed = terminals
                            .kill(&owner, &id, "terminal_close tool".to_string())
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?
                            .await
                            .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                        Ok(serde_json::json!({
                            "sessionId": id.as_str(),
                            "outcome": if closed { "closed" } else { "already-closing" },
                        }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;

        let list_terminals = terminals.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "terminal_list".to_string(),
                description: "List persistent terminal sessions owned by the initiating agent."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {}
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "array",
                        "items": { "type": "object" }
                    }),
                    render: Arc::new(|_args, value| {
                        Ok(vec![ContentBlock::Text {
                            text: value.to_string(),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |_args, run: &ToolRunContext| {
                    let terminals = list_terminals.clone();
                    let owner = run.execution.agent.clone();
                    Box::pin(async move {
                        let owner = owner.ok_or_else(|| {
                            ToolBodyError::plain("terminal tools require an initiating agent")
                        })?;
                        Ok(serde_json::Value::Array(
                            terminals
                                .list(&owner)
                                .into_iter()
                                .map(|session| {
                                    serde_json::json!({
                                        "sessionId": session.session_id.as_str(),
                                        "name": session.name,
                                        "type": session.type_,
                                        "pid": session.pid,
                                        "status": status_json(&session.status),
                                    })
                                })
                                .collect(),
                        ))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;
        Ok(Arc::new(Self))
    }
}

fn parse_signal(value: &str) -> Result<TerminalSignal, ToolBodyError> {
    match value {
        "SIGINT" => Ok(TerminalSignal::SigInt),
        "SIGTERM" => Ok(TerminalSignal::SigTerm),
        "SIGKILL" => Ok(TerminalSignal::SigKill),
        "SIGTSTP" => Ok(TerminalSignal::SigTstp),
        "SIGHUP" => Ok(TerminalSignal::SigHup),
        _ => Err(ToolBodyError::plain("unsupported terminal signal")),
    }
}

fn required_session_id(
    args: &serde_json::Value,
) -> Result<dsh_terminal::TerminalSessionId, ToolBodyError> {
    let id = args
        .get("sessionId")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolBodyError::plain("sessionId must be non-empty"))?;
    Ok(terminal_session_id(id))
}

fn status_json(status: &TerminalSessionStatus) -> serde_json::Value {
    match status {
        TerminalSessionStatus::Running => serde_json::json!({ "kind": "running" }),
        TerminalSessionStatus::Exited { exit_code, signal } => serde_json::json!({
            "kind": "exited",
            "exitCode": exit_code,
            "signal": signal,
        }),
    }
}
