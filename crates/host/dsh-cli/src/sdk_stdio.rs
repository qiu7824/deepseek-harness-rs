//! Python/TypeScript SDK-compatible NDJSON JSON-RPC runtime over stdio.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use dsh_agent::{AgentFactory, AgentHandle, AgentOptions};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_session::{CreateSessionMeta, SessionEvent, session_id};
use serde_json::{Value, json};

struct SdkConfig {
    cwd: String,
    provider: String,
    model: String,
    max_tokens: Option<u64>,
}

pub async fn run() -> Result<(), String> {
    let ctx = cordis::Context::root();
    let spine = dsh_host::compose_persistent_host(&ctx, Some("sdk"))
        .map_err(|error| format!("compose Host: {error}"))?;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let item = line.map_err(|error| format!("stdin read failed: {error}"));
            if input_tx.send(item).is_err() {
                return;
            }
        }
    });

    let mut config: Option<SdkConfig> = None;
    let mut agents: HashMap<String, AgentHandle> = HashMap::new();
    while let Some(line) = input_rx.recv().await {
        let line = line?;
        let frame: Value = match serde_json::from_str(line.trim()) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        let Some(object) = frame.as_object() else {
            continue;
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = object.get("id").cloned();
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        match method {
            "initialize" => {
                let response = initialize(&spine, &params);
                match response {
                    Ok(next) => {
                        config = Some(next);
                        if let Some(id) = id {
                            write_response(
                                id,
                                json!({
                                    "serverInfo": {
                                        "name": "deepseek-harness-sdk-runtime",
                                        "version": env!("CARGO_PKG_VERSION")
                                    }
                                }),
                            )?;
                        }
                    }
                    Err(error) => write_error(id.unwrap_or(Value::Null), -32602, &error)?,
                }
            }
            "session/prompt" => {
                let Some(id) = id else {
                    continue;
                };
                let Some(config) = config.as_ref() else {
                    write_error(id, -32603, "SDK runtime is not initialized")?;
                    continue;
                };
                if let Err(error) =
                    prompt(&ctx, &spine, &mut agents, config, id.clone(), params).await
                {
                    write_error(id, -32603, &error)?;
                }
            }
            "shutdown" => {
                for (_, handle) in agents.drain() {
                    handle.dispose.await;
                }
                let shutdown = spine.shutdown().await;
                match shutdown {
                    Ok(()) => {
                        if let Some(id) = id {
                            write_response(id, json!({}))?;
                        }
                        return Ok(());
                    }
                    Err(error) => {
                        if let Some(id) = id {
                            write_error(id, -32603, &error)?;
                        }
                        return Err(error);
                    }
                }
            }
            other => {
                if let Some(id) = id {
                    write_error(id, -32601, &format!("method not found: {other}"))?;
                }
            }
        }
    }

    for (_, handle) in agents.drain() {
        handle.dispose.await;
    }
    spine.shutdown().await
}

fn initialize(spine: &dsh_host::HostSpine, params: &Value) -> Result<SdkConfig, String> {
    let cwd = required_string(params, "cwd")?;
    if !std::path::Path::new(&cwd).is_absolute() {
        return Err("initialize cwd must be an absolute path".to_string());
    }
    let provider = required_string(params, "provider")?;
    if !spine
        .llm
        .list_providers()
        .iter()
        .any(|entry| entry.id == provider)
    {
        return Err(format!("no adapter registered for provider {provider:?}"));
    }
    let model = required_string(params, "model")?;
    let max_tokens = match params.get("maxTokens") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|tokens| *tokens > 0)
                .ok_or_else(|| "initialize maxTokens must be a positive integer".to_string())?,
        ),
    };
    Ok(SdkConfig {
        cwd,
        provider,
        model,
        max_tokens,
    })
}

async fn prompt(
    ctx: &cordis::Context,
    spine: &dsh_host::HostSpine,
    agents: &mut HashMap<String, AgentHandle>,
    config: &SdkConfig,
    request_id: Value,
    params: Value,
) -> Result<(), String> {
    let session_id_text = required_string(&params, "sessionId")?;
    if !agents.contains_key(&session_id_text) {
        let handle = spine
            .agent_loop
            .create_agent(
                ctx,
                dsh_agent::CreateAgentOptions {
                    session_id: Some(session_id(session_id_text.clone())),
                    meta: Some(CreateSessionMeta {
                        cwd: Some(config.cwd.clone()),
                        ..Default::default()
                    }),
                    agent_options: Some(AgentOptions {
                        provider: Some(config.provider.clone()),
                        model: Some(config.model.clone()),
                        max_tokens: config.max_tokens,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        agents.insert(session_id_text.clone(), handle);
    }
    let agent = agents
        .get(&session_id_text)
        .map(|handle| handle.agent.clone())
        .ok_or_else(|| "session agent disappeared during creation".to_string())?;
    if spine
        .agents
        .get(agent.id())
        .is_none_or(|live| !std::sync::Arc::ptr_eq(&live, &agent))
    {
        return Err(format!(
            "session agent was disposed outside the server: {session_id_text}"
        ));
    }
    let blocks: Vec<ContentBlock> = serde_json::from_value(
        params
            .get("contentBlocks")
            .cloned()
            .ok_or_else(|| "session/prompt contentBlocks is required".to_string())?,
    )
    .map_err(|error| format!("invalid session/prompt contentBlocks: {error}"))?;
    if blocks.is_empty() {
        return Err("session/prompt contentBlocks must not be empty".to_string());
    }
    let message = create_user_message(
        blocks,
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let baseline = agent.session().seq().get();
    agent.followup(message.clone());
    let receipt_end = agent.session().seq().get();
    for event in agent.session().events_from(baseline) {
        notify_event(&session_id_text, &event)?;
    }
    write_notification(
        "session.status",
        json!({ "sessionId": session_id_text, "status": "running" }),
    )?;
    write_response(request_id, json!({ "messageId": message.id.as_str() }))?;

    agent.when_idle().await;
    for event in agent.session().events_from(receipt_end) {
        notify_event(&session_id_text, &event)?;
    }
    write_notification(
        "session.status",
        json!({ "sessionId": session_id_text, "status": "idle" }),
    )?;
    spine
        .sessions
        .flush(agent.session())
        .await
        .map_err(|error| format!("session flush failed: {error}"))?;
    Ok(())
}

fn notify_event(session_id: &str, event: &SessionEvent) -> Result<(), String> {
    write_notification(
        "session.event",
        json!({ "sessionId": session_id, "event": event }),
    )
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
        "jsonrpc": "2.0",
        "id": id,
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
