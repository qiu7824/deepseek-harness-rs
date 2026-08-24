//! Automation-only ACP bridge over NDJSON stdio.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dsh_agent::{Agent, AgentFactory, AgentHandle, AgentOptions};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_session::{CreateSessionMeta, SessionEvent, session_id};
use serde_json::{Value, json};

struct PromptTicket {
    generation: u64,
    cancelled: Arc<AtomicBool>,
    slot: std::sync::Weak<PromptSlot>,
}

struct PromptState {
    generation: u64,
    cancelled: Arc<AtomicBool>,
}

impl Drop for PromptTicket {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.upgrade() {
            slot.complete_generation(self.generation);
        }
    }
}

#[derive(Default)]
struct PromptSlot {
    state: parking_lot::Mutex<(u64, Option<PromptState>)>,
}

impl PromptSlot {
    fn reserve(self: &Arc<Self>) -> Result<PromptTicket, String> {
        let mut state = self.state.lock();
        if state.1.is_some() {
            return Err("a prompt is already in flight for this session".to_string());
        }
        state.0 += 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let ticket = PromptTicket {
            generation: state.0,
            cancelled: cancelled.clone(),
            slot: Arc::downgrade(self),
        };
        state.1 = Some(PromptState {
            generation: ticket.generation,
            cancelled,
        });
        Ok(ticket)
    }

    fn cancel(&self) {
        if let Some(ticket) = &self.state.lock().1 {
            ticket.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn admit(&self, ticket: &PromptTicket, start: impl FnOnce()) -> bool {
        let state = self.state.lock();
        let current = state
            .1
            .as_ref()
            .is_some_and(|current| current.generation == ticket.generation);
        if !current || ticket.cancelled.load(Ordering::SeqCst) {
            return false;
        }
        start();
        true
    }

    fn complete(&self, ticket: &PromptTicket) {
        self.complete_generation(ticket.generation);
    }

    fn complete_generation(&self, generation: u64) {
        let mut state = self.state.lock();
        if state
            .1
            .as_ref()
            .is_some_and(|current| current.generation == generation)
        {
            state.1 = None;
        }
    }
}

struct AcpSession {
    handle: AgentHandle,
    prompt: Arc<PromptSlot>,
}

pub async fn run() -> Result<(), String> {
    let ctx = cordis::Context::root();
    let spine = dsh_host::compose_persistent_host(&ctx, Some("acp"))
        .map_err(|error| format!("compose Host: {error}"))?;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            if input_tx
                .send(line.map_err(|error| format!("stdin read failed: {error}")))
                .is_err()
            {
                return;
            }
        }
    });

    let mut sessions: HashMap<String, AcpSession> = HashMap::new();
    let mut prompts = tokio::task::JoinSet::<()>::new();
    while let Some(line) = input_rx.recv().await {
        let frame: Value = match serde_json::from_str(line?.trim()) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = frame.get("id").cloned();
        let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
        match method {
            "initialize" => {
                if let Some(id) = id {
                    write_response(
                        id,
                        json!({
                            "protocolVersion": dsh_acp::PROTOCOL_VERSION,
                            "agentInfo": { "name": "deepseek-harness-acp", "version": env!("CARGO_PKG_VERSION") },
                            "agentCapabilities": {
                                "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false }
                            },
                            "authMethods": []
                        }),
                    )?;
                }
            }
            "authenticate" => {
                if let Some(id) = id {
                    write_response(id, Value::Null)?;
                }
            }
            "session/new" => {
                let Some(id) = id else { continue };
                match create_session(&ctx, &spine, &params).await {
                    Ok((session_id, handle)) => {
                        sessions.insert(
                            session_id.clone(),
                            AcpSession {
                                handle,
                                prompt: Arc::new(PromptSlot::default()),
                            },
                        );
                        write_response(id, json!({ "sessionId": session_id }))?;
                    }
                    Err(error) => write_error(id, -32602, &error)?,
                }
            }
            "session/prompt" => {
                let Some(id) = id else { continue };
                match prepare_prompt(&spine, &sessions, &params) {
                    Ok(prepared) => prompts.spawn(async move {
                        let result = run_prompt(prepared).await;
                        match result {
                            Ok(stop_reason) => {
                                let _ = write_response(id, json!({ "stopReason": stop_reason }));
                            }
                            Err(error) => {
                                let _ = write_error(id, -32603, &error);
                            }
                        }
                    }),
                    Err(error) => {
                        write_error(id, -32603, &error)?;
                        continue;
                    }
                };
            }
            "session/cancel" => {
                if let Some(session_id) = params.get("sessionId").and_then(Value::as_str)
                    && let Some(session) = sessions.get(session_id)
                {
                    session.prompt.cancel();
                    session
                        .handle
                        .agent
                        .cancel(dsh_session::AgentCancelCause::User, None);
                }
            }
            other => {
                if let Some(id) = id {
                    write_error(id, -32601, &format!("method not found: {other}"))?;
                }
            }
        }
    }

    let roots: Vec<Arc<dyn dsh_agent::Agent>> = sessions
        .values()
        .map(|session| session.handle.agent.clone())
        .collect();
    for session in sessions.values() {
        session.prompt.cancel();
        session
            .handle
            .agent
            .cancel(dsh_session::AgentCancelCause::User, None);
    }
    while prompts.join_next().await.is_some() {}
    if let Some(subagents) = spine
        .ctx
        .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
    {
        subagents
            .drain_continuable_descendants(&roots)
            .await
            .map_err(|error| error.to_string())?;
    }
    for (_, session) in sessions.drain() {
        session.handle.dispose.await;
    }
    spine.shutdown().await
}

async fn create_session(
    ctx: &cordis::Context,
    spine: &dsh_host::HostSpine,
    params: &Value,
) -> Result<(String, AgentHandle), String> {
    let cwd = required_string(params, "cwd")?;
    if !std::path::Path::new(&cwd).is_absolute() {
        return Err("cwd must be an absolute path".to_string());
    }
    if params
        .get("mcpServers")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        return Err("mcpServers is not supported".to_string());
    }
    if params
        .get("additionalDirectories")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        return Err("additionalDirectories is not supported".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let provider =
        std::env::var("DSH_ACP_PROVIDER").unwrap_or_else(|_| "deepseek-official".to_string());
    let model = std::env::var("DSH_ACP_MODEL")
        .or_else(|_| std::env::var("DSH_DEEPSEEK_MODEL"))
        .unwrap_or_else(|_| "deepseek-chat".to_string());
    let handle = spine
        .agent_loop
        .create_agent(
            ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id(id.clone())),
                meta: Some(CreateSessionMeta {
                    cwd: Some(cwd),
                    ..Default::default()
                }),
                agent_options: Some(AgentOptions {
                    provider: Some(provider),
                    model: Some(model),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;
    Ok((id, handle))
}

struct PreparedPrompt {
    session_id: String,
    agent: Arc<dyn Agent>,
    sessions: Arc<dsh_session::SessionStore>,
    slot: Arc<PromptSlot>,
    ticket: PromptTicket,
    text: String,
}

fn prepare_prompt(
    spine: &dsh_host::HostSpine,
    sessions: &HashMap<String, AcpSession>,
    params: &Value,
) -> Result<PreparedPrompt, String> {
    let session_id = required_string(params, "sessionId")?;
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| format!("unknown session: {session_id}"))?;
    let agent = session.handle.agent.clone();
    if spine
        .agents
        .get(agent.id())
        .is_none_or(|live| !Arc::ptr_eq(&live, &agent))
    {
        return Err("prompt was not queued: the agent was disposed outside the bridge".to_string());
    }
    let text = prompt_text(params)?;
    let ticket = session.prompt.reserve()?;
    Ok(PreparedPrompt {
        session_id,
        agent,
        sessions: spine.sessions.clone(),
        slot: session.prompt.clone(),
        ticket,
        text,
    })
}

async fn run_prompt(prepared: PreparedPrompt) -> Result<&'static str, String> {
    let message = create_user_message(
        vec![ContentBlock::Text {
            text: prepared.text,
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let mut baseline = 0_u64;
    let admitted = prepared.slot.admit(&prepared.ticket, || {
        baseline = prepared.agent.session().seq() as u64;
        prepared.agent.followup(message);
    });
    if !admitted {
        prepared.slot.complete(&prepared.ticket);
        return Ok("cancelled");
    }
    prepared.agent.when_idle().await;
    let events = prepared.agent.session().events_from(baseline);
    for event in &events {
        emit_committed_text(&prepared.session_id, event)?;
    }
    prepared.sessions.flush(prepared.agent.session()).await?;
    let cancelled = prepared.ticket.cancelled.load(Ordering::SeqCst);
    prepared.slot.complete(&prepared.ticket);
    if cancelled {
        Ok("cancelled")
    } else {
        stop_reason(&events)
    }
}

fn prompt_text(params: &Value) -> Result<String, String> {
    let blocks = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| "session/prompt prompt must be an array".to_string())?;
    let mut text = String::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "ACP text block requires text".to_string())?,
            ),
            Some("resource_link") => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
                text.push_str(&format!("\n[resource_link name={name:?} uri={uri:?}]\n"));
            }
            Some(_) => {
                return Err("only text and resource_link prompt content is supported".to_string());
            }
            None => return Err("ACP prompt block requires type".to_string()),
        }
    }
    if text.trim().is_empty() {
        return Err("empty prompt".to_string());
    }
    Ok(text)
}

fn emit_committed_text(session_id: &str, event: &SessionEvent) -> Result<(), String> {
    if event.type_ != "assistant/message" {
        return Ok(());
    }
    let content = event
        .data
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| event.data.get("content"))
        .and_then(Value::as_array);
    for block in content.into_iter().flatten() {
        if let Some(text) = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            write_notification(
                "session/update",
                json!({
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": text }
                    }
                }),
            )?;
        }
    }
    Ok(())
}

fn stop_reason(events: &[SessionEvent]) -> Result<&'static str, String> {
    let reason = events
        .iter()
        .rev()
        .find(|event| event.type_ == "turn/end")
        .and_then(|event| event.data.get("reason"))
        .ok_or_else(|| "prompt ended without a durable turn/end event".to_string())?;
    match reason.get("kind").and_then(Value::as_str) {
        Some("completed") => Ok("end_turn"),
        Some("max-tokens") => Ok("max_tokens"),
        Some("interrupted" | "aborted") => Ok("cancelled"),
        Some("error") => Err(reason
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("prompt failed")
            .to_string()),
        Some("blocked") => Err("prompt was blocked".to_string()),
        Some(kind) => Err(format!("prompt ended with unsupported reason: {kind}")),
        None => Err("prompt ended with a malformed reason".to_string()),
    }
}

fn required_string(params: &Value, name: &str) -> Result<String, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{name} must be a non-empty string"))
}

fn write_response(id: Value, result: Value) -> Result<(), String> {
    write_frame(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn write_error(id: Value, code: i64, message: &str) -> Result<(), String> {
    write_frame(&json!({
        "jsonrpc": "2.0", "id": id,
        "error": { "code": code, "message": message }
    }))
}

fn write_notification(method: &str, params: Value) -> Result<(), String> {
    write_frame(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
}

fn write_frame(frame: &Value) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, frame).map_err(|error| error.to_string())?;
    output.write_all(b"\n").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}
