//! Project delegated app-server items into the ordinary persisted child view.
use dsh_session::{Session, SessionStore};
use dsh_subagent::{ResolvedSubagentStartRequest, SubagentResult, SubagentStopReason};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

pub struct Transcript {
    pub session: Session,
    store: Arc<SessionStore>,
    detach: tokio::sync::Mutex<Option<cordis::Disposer>>,
    calls: parking_lot::Mutex<HashSet<String>>,
    messages: parking_lot::Mutex<HashSet<String>>,
    steps: parking_lot::Mutex<std::collections::HashMap<String, u64>>,
    next_step: std::sync::atomic::AtomicU64,
    model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commentary_and_tool_items_are_readable_and_terminal_is_durable() {
        let ctx = cordis::Context::root();
        let store = SessionStore::install(&ctx);
        let session = store
            .prepare(Some(dsh_session::session_id("external-codex-test")), None)
            .unwrap();
        let detach = store.enter(&session).unwrap();
        let recorder = Transcript {
            session: session.clone(),
            store,
            detach: tokio::sync::Mutex::new(Some(detach)),
            calls: Default::default(),
            messages: Default::default(),
            steps: Default::default(),
            next_step: std::sync::atomic::AtomicU64::new(1),
            model: "test-model".into(),
        };
        recorder.observe("item/completed", &json!({"item":{"id":"commentary-1","type":"agentMessage","phase":"commentary","text":"Inspecting files"}})).unwrap();
        let started = json!({"item":{"id":"command-1","type":"commandExecution","command":"echo test","cwd":"C:/work","status":"inProgress"}});
        recorder.observe("item/started", &started).unwrap();
        let done = json!({"item":{"id":"command-1","type":"commandExecution","command":"echo test","aggregatedOutput":"test\n","exitCode":0,"status":"completed"}});
        recorder.observe("item/completed", &done).unwrap();
        recorder.observe("item/completed", &done).unwrap();
        recorder
            .finish(&Ok(SubagentResult {
                output: vec![],
                structured: None,
                stop_reason: SubagentStopReason::Completed,
            }))
            .await
            .unwrap();
        let events = session.events();
        assert_eq!(events.iter().filter(|e| e.type_ == "tool/call").count(), 1);
        assert_eq!(
            events.iter().filter(|e| e.type_ == "tool/result").count(),
            1
        );
        assert!(
            events.iter().any(|e| e.type_ == "assistant/message"
                && e.data.to_string().contains("Inspecting files"))
        );
        assert_eq!(events.last().unwrap().data["reason"]["kind"], "completed");
    }
}

impl Transcript {
    pub async fn create(
        request: &ResolvedSubagentStartRequest,
    ) -> Result<Option<Arc<Self>>, String> {
        let Some(store) = request
            .request
            .parent
            .ctx()
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return Ok(None);
        };
        let session = store.prepare(
            Some(dsh_session::session_id(uuid::Uuid::new_v4().to_string())),
            Some(dsh_session::CreateSessionOptions {
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: request.request.parent.session().header().cwd.clone(),
                    parent_session: Some(request.request.parent.id().clone()),
                    origin: Some("subagent".to_string()),
                    delegation_depth: Some(
                        request
                            .request
                            .parent
                            .session()
                            .header()
                            .delegation_depth
                            .unwrap_or(0)
                            + 1,
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )?;
        let detach = store.enter(&session)?;
        if let Err(error) = store.announce(&session).await {
            detach().await;
            return Err(error);
        }
        let transcript = Arc::new(Self {
            session,
            store,
            detach: tokio::sync::Mutex::new(Some(detach)),
            calls: Default::default(),
            messages: Default::default(),
            steps: Default::default(),
            next_step: std::sync::atomic::AtomicU64::new(1),
            model: request
                .request
                .agent_options
                .as_ref()
                .and_then(|o| o.model.clone())
                .unwrap_or_else(|| "codex".into()),
        });
        let initialize = (|| {
            transcript.append(
                "subagent/descriptor",
                serde_json::to_value(&request.descriptor).map_err(|e| e.to_string())?,
            )?;
            transcript.append("session/end-seed", dsh_session::end_seed_data())?;
            transcript.append("turn/start", dsh_session::turn_start_data(1))?;
            let message = dsh_llm::create_user_message(
                request.request.prompt.clone(),
                dsh_llm::MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            );
            transcript.append(
                "user/message",
                serde_json::to_value(message).map_err(|error| error.to_string())?,
            )
        })();
        if let Err(error) = initialize {
            if let Some(detach) = transcript.detach.lock().await.take() {
                detach().await;
            }
            return Err(error);
        }
        Ok(Some(transcript))
    }

    fn append(&self, kind: &str, data: Value) -> Result<(), String> {
        let surface =
            dsh_session::is_surface_eligible_type(kind).then_some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            });
        self.session.append(kind, data, surface).map(|_| ())
    }

    fn open_step(&self, id: &str) -> Result<u64, String> {
        let mut steps = self.steps.lock();
        if let Some(step) = steps.get(id) {
            return Ok(*step);
        }
        let step = self
            .next_step
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append("step/start", dsh_session::step_data(1, step))?;
        steps.insert(id.into(), step);
        Ok(step)
    }

    fn close_step(&self, id: &str) -> Result<(), String> {
        if let Some(step) = self.steps.lock().remove(id) {
            self.append("step/end", dsh_session::step_data(1, step))?;
        }
        Ok(())
    }

    fn assistant(&self, step: u64, content: Vec<dsh_llm::ContentBlock>) -> Result<(), String> {
        let message = dsh_llm::create_assistant_message(
            content,
            dsh_llm::ModelMessageSource {
                provider: "codex".into(),
                model: self.model.clone(),
                replay_state: None,
            },
        );
        self.append(
            "assistant/message",
            dsh_session::assistant_message_data(1, step, &message, None),
        )
    }

    pub fn observe(&self, method: &str, params: &Value) -> Result<(), String> {
        if !matches!(method, "item/started" | "item/completed") {
            return Ok(());
        }
        let Some(item) = params.get("item") else {
            return Ok(());
        };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return Ok(());
        };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        let completed = method == "item/completed";
        if self.messages.lock().contains(id) {
            return Ok(());
        }
        if matches!(kind, "agentMessage" | "plan")
            && completed
            && self.messages.lock().insert(id.into())
        {
            let step = self.open_step(id)?;
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                self.assistant(
                    step,
                    vec![dsh_llm::ContentBlock::Text { text: text.into() }],
                )?;
            }
            self.close_step(id)?;
        } else if kind == "reasoning" && completed && self.messages.lock().insert(id.into()) {
            let step = self.open_step(id)?;
            let text = item
                .get("summary")
                .and_then(Value::as_array)
                .map(|summary| {
                    summary
                        .iter()
                        .filter_map(|part| {
                            part.get("text")
                                .and_then(Value::as_str)
                                .or_else(|| part.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !text.is_empty() {
                self.assistant(step, vec![dsh_llm::ContentBlock::Reasoning { text }])?;
            }
            self.close_step(id)?;
        } else if matches!(
            kind,
            "commandExecution"
                | "fileChange"
                | "mcpToolCall"
                | "dynamicToolCall"
                | "webSearch"
                | "imageView"
                | "collabAgentToolCall"
        ) {
            let step = self.open_step(id)?;
            let name = format!("codex.{kind}");
            if self.calls.lock().insert(id.into()) {
                let args = json!({"command":item.get("command"),"cwd":item.get("cwd"),"changes":item.get("changes"),"tool":item.get("tool"),"arguments":item.get("arguments"),"query":item.get("query"),"path":item.get("path")}).to_string();
                let call_id = dsh_llm::call_id(id);
                self.assistant(
                    step,
                    vec![dsh_llm::ContentBlock::ToolCall {
                        id: call_id.clone(),
                        name: name.clone(),
                        arguments: args.clone(),
                    }],
                )?;
                self.append(
                    "tool/call",
                    dsh_session::tool_call_data(1, step, &call_id, &name, &args),
                )?;
            }
            if completed && self.messages.lock().insert(id.into()) {
                let output = item.get("aggregatedOutput").and_then(Value::as_str)
                    .map(str::to_string).unwrap_or_else(|| json!({"status":item.get("status"),"result":item.get("result"),"changes":item.get("changes"),"content":item.get("contentItems").and_then(Value::as_array).map(|items| items.iter().filter_map(|item| item.get("text").and_then(Value::as_str)).collect::<Vec<_>>()),"error":item.get("error")}).to_string());
                let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                    || item
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .is_some_and(|code| code != 0);
                let message =
                    dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
                        call_id: dsh_llm::call_id(id),
                        content: vec![dsh_llm::ContentBlock::Text { text: output }],
                        is_error: failed,
                    });
                self.append(
                    "tool/result",
                    dsh_session::tool_result_data(
                        1,
                        step,
                        &message,
                        None,
                        Some(&json!({"exitCode":item.get("exitCode"),"status":item.get("status")})),
                    ),
                )?;
                self.close_step(id)?;
            }
        }
        Ok(())
    }

    pub async fn finish(&self, result: &Result<SubagentResult, String>) -> Result<(), String> {
        let mut detach = self.detach.lock().await;
        let Some(dispose) = detach.take() else {
            return Ok(());
        };
        let reason = match result {
            Ok(result) if result.stop_reason == SubagentStopReason::Completed => {
                json!({"kind":"completed"})
            }
            Ok(result) if result.stop_reason == SubagentStopReason::Aborted => {
                json!({"kind":"aborted","reason":{"kind":"parent"}})
            }
            Ok(result) if result.stop_reason == SubagentStopReason::MaxTokens => {
                json!({"kind":"max-tokens"})
            }
            Ok(result) => {
                json!({"kind":"error","error":{"message":result.stop_reason.as_str(),"code":"CODEX_TURN_FAILED"}})
            }
            Err(error) => {
                json!({"kind":"error","error":{"message":error,"code":"CODEX_TRANSPORT"}})
            }
        };
        let pending = self.steps.lock().drain().collect::<Vec<_>>();
        let close = (|| {
            for (id, step) in pending {
                if self.calls.lock().contains(&id) {
                    let message =
                        dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
                            call_id: dsh_llm::call_id(id),
                            content: vec![dsh_llm::ContentBlock::Text {
                                text: "Delegated turn ended before this tool reported an outcome."
                                    .into(),
                            }],
                            is_error: true,
                        });
                    self.append(
                        "tool/result",
                        dsh_session::tool_result_data(
                            1,
                            step,
                            &message,
                            None,
                            Some(&json!({"status":"interrupted"})),
                        ),
                    )?;
                }
                self.append("step/end", dsh_session::step_data(1, step))?;
            }
            self.append("turn/end", json!({"turn":1,"reason":reason}))
        })();
        let flushed = self.store.flush(&self.session).await;
        dispose().await;
        close?;
        flushed.map(|_| ())
    }
}
