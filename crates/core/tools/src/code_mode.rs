//! Code Mode `run_code` transport.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use dsh_code_runtime::{CodeBindingErrorClass, CodeBindingFunction, CodeBindingNamespace};
use dsh_llm::{ContentBlock, call_id};
use serde_json::{Value as JsonValue, json};

use crate::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime,
};

struct NestedDispatchLog {
    session: Option<dsh_session::Session>,
    start: JsonValue,
    settled: bool,
    start_seq: Option<u64>,
}
impl NestedDispatchLog {
    fn begin(
        agent: Option<&Arc<dyn dsh_agent::Agent>>,
        start: JsonValue,
        signal: &crate::AbortPredicate,
    ) -> Result<Self, String> {
        let session = agent.map(|agent| agent.session().clone());
        let start_seq = if let Some(session) = &session {
            Some(
                session
                    .append_if_read("tool/ptc-dispatch-start", start.clone(), None, |_| {
                        Ok(!signal())
                    })?
                    .ok_or("Nested tool execution cancelled before dispatch")?
                    .seq
                    .get(),
            )
        } else {
            None
        };
        Ok(Self {
            session,
            start,
            settled: false,
            start_seq,
        })
    }
    fn finish(&mut self, result: &crate::ToolExecutionResult) -> Result<(), String> {
        let mut value = self.start.clone();
        value["isError"] = json!(result.is_error);
        value["content"] = json!(result.content);
        if let Some(meta) = &result.meta {
            value["meta"] = meta.clone();
        }
        if let Some(info) = result.error.as_ref().and_then(|error| error.info.as_ref()) {
            value["error"] = json!({"name":info.name,"code":info.code});
        }
        if let (Some(session), Some(seq)) = (&self.session, self.start_seq) {
            session.append_if_read("tool/ptc-dispatch", value, None, |reader| {
                pending_dispatch(reader, seq)
            })?;
        }
        self.settled = true;
        Ok(())
    }
}
impl Drop for NestedDispatchLog {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Some(session) = &self.session {
            let mut value = self.start.clone();
            value["isError"] = json!(true);
            value["content"] = json!([{"type":"text","text":"Nested tool execution interrupted; its result is unknown."}]);
            value["error"] = json!({"name":"ToolAbortedError","code":"ABORTED"});
            if let Some(seq) = self.start_seq {
                if let Err(error) =
                    session.append_if_read("tool/ptc-dispatch", value, None, |reader| {
                        pending_dispatch(reader, seq)
                    })
                {
                    eprintln!("nested tool finalization failed: {error}");
                }
            }
        }
    }
}

fn pending_dispatch(
    reader: &dsh_session::SessionEventReader<'_>,
    seq: u64,
) -> Result<bool, String> {
    let Some(start) = reader
        .read(seq)?
        .filter(|event| event.type_ == "tool/ptc-dispatch-start")
    else {
        return Ok(false);
    };
    let mut pending = true;
    reader.visit(seq + 1, None, |event| {
        if matches!(event.type_.as_str(), "step/end" | "turn/end")
            || event.type_ == "tool/ptc-dispatch"
                && event.data["subCallId"] == start.data["subCallId"]
                && event.data["rootCallId"] == start.data["rootCallId"]
        {
            pending = false;
        }
        Ok(pending)
    })?;
    Ok(pending)
}
struct CodeLifetime {
    closed: Arc<AtomicBool>,
    session: Option<dsh_session::Session>,
    root: dsh_llm::CallId,
    from: u64,
}

#[cfg(test)]
mod archive_dispatch_tests {
    use super::*;

    #[test]
    fn archived_nested_dispatch_cleanup_only_closes_unsettled_calls_once() {
        let session = dsh_session::Session::create(
            dsh_session::session_id("code-mode-archive"),
            None,
            None,
            None,
        )
        .unwrap();
        let first = json!({"rootCallId":"root","subCallId":"first","name":"read","arguments":{}});
        let second = json!({"rootCallId":"root","subCallId":"second","name":"read","arguments":{}});
        let first_seq = session
            .append("tool/ptc-dispatch-start", first.clone(), None)
            .unwrap()
            .seq
            .get();
        let second_seq = session
            .append("tool/ptc-dispatch-start", second, None)
            .unwrap()
            .seq
            .get();
        session.append("tool/ptc-dispatch", first, None).unwrap();
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        session
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        let archived = dsh_session::Session::from_event_archive(
            session.id().clone(),
            builder.finish().unwrap(),
            session.header(),
            session.inherited_event_count(),
            vec![],
        )
        .unwrap();
        archived.with_event_reader(|reader| {
            assert!(!pending_dispatch(reader, first_seq).unwrap());
            assert!(pending_dispatch(reader, second_seq).unwrap());
        });
        let closed = Arc::new(AtomicBool::new(false));
        drop(CodeLifetime {
            closed: closed.clone(),
            session: Some(archived.clone()),
            root: call_id("root"),
            from: 0,
        });
        assert!(closed.load(Ordering::SeqCst));
        archived
            .with_event_reader(|reader| assert!(!pending_dispatch(reader, second_seq).unwrap()));
        let mut completions = Vec::new();
        archived
            .visit_events(0, None, |event| {
                if event.type_ == "tool/ptc-dispatch" {
                    completions.push(event.data["subCallId"].clone());
                }
                Ok(true)
            })
            .unwrap();
        assert_eq!(completions, [json!("first"), json!("second")]);
    }
}
impl Drop for CodeLifetime {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        let Some(session) = &self.session else {
            return;
        };
        let mut pending: Vec<(u64, JsonValue)> = Vec::new();
        if let Err(error) = session.visit_events(self.from, None, |event| {
            match event.type_.as_str() {
                "step/end" | "turn/end" => pending.clear(),
                "tool/ptc-dispatch-start" if event.data["rootCallId"] == self.root.as_str() => {
                    pending.push((event.seq.get(), event.data.clone()));
                }
                "tool/ptc-dispatch" => pending.retain(|(_, start)| {
                    start["rootCallId"] != event.data["rootCallId"]
                        || start["subCallId"] != event.data["subCallId"]
                }),
                _ => {}
            }
            Ok(true)
        }) {
            eprintln!("nested tool cleanup could not read pending calls: {error}");
            return;
        }
        for (seq, mut value) in pending {
            value["isError"] = json!(true);
            value["content"] = json!([{"type":"text","text":"Nested tool execution interrupted; its result is unknown."}]);
            value["error"] = json!({"name":"ToolAbortedError","code":"ABORTED"});
            if let Err(error) = session.append_if_read("tool/ptc-dispatch", value, None, |reader| {
                pending_dispatch(reader, seq)
            }) {
                eprintln!("nested tool cleanup finalization failed: {error}");
            }
        }
    }
}

pub(crate) const TYPESCRIPT_RUN_CODE_DESCRIPTION: &str = "Execute a TypeScript program against the available tools. Takes two required arguments: `code`, the BODY of an async function (erasable syntax only; top-level `await` and `return` work), and `description`, a short summary of what the program does. Call tools as `await tools.name(args)` per the declarations in the system prompt. Only what you print or return comes back — curate it.";

pub(crate) fn create_run_code_tool(runtime: Weak<ToolRuntime>) -> Arc<ToolDefinition> {
    Arc::new(ToolDefinition {
        name: crate::RUN_CODE_NAME.to_string(),
        description: TYPESCRIPT_RUN_CODE_DESCRIPTION.to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The program: the body of an async TypeScript function."
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this program does in active voice, 5-10 words (shown in the UI)."
                },
                "timeoutMs": {"type":"integer","minimum":1,"maximum":600000,"description":"Elapsed budget in milliseconds, including nested tools and approval waits. Default 120000; maximum 600000. Zero does not disable the deadline."}
            },
            "required": ["code", "description"]
        }),
        output: ToolOutputDefinition {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "logs": { "type": "array", "items": { "type": "string" } },
                    "result": {}
                },
                "required": ["logs"]
            }),
            render: Arc::new(|_args, value| {
                let logs = value["logs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(JsonValue::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                let rendered = value.get("result").map(render_value).unwrap_or_default();
                let text = [logs, rendered]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(vec![ContentBlock::Text {
                    text: if text.is_empty() {
                        "(run_code completed with no output)".to_string()
                    } else {
                        text
                    },
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let runtime = runtime.clone();
            let code = args["code"].as_str().unwrap_or_default().to_string();
            let timeout_ms = match args.get("timeoutMs") {
                None => Ok(120_000),
                Some(value) => value
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= 600_000)
                    .ok_or_else(|| {
                        ToolBodyError::plain("timeoutMs must be an integer from 1 to 600000")
                    }),
            };
            let signal = exec.signal.lock().clone();
            let agent = exec.agent.clone();
            let root_call_id = exec.root_call_id.clone();
            let parent_call_id = exec.call_id.clone();
            let parent = exec.token;
            Box::pin(async move {
                let timeout_ms = timeout_ms?;
                let owner = runtime
                    .upgrade()
                    .ok_or_else(|| ToolBodyError::plain("tool runtime is unavailable"))?;
                let code_runtime = owner.code_runtime().map_err(ToolBodyError::plain)?;
                let closed = Arc::new(AtomicBool::new(false));
                let session = agent.as_ref().map(|agent| agent.session().clone());
                let _lifetime = CodeLifetime {
                    closed: closed.clone(),
                    from: session.as_ref().map(|s| s.seq().get()).unwrap_or(0),
                    session,
                    root: root_call_id.clone(),
                };
                let original_signal = signal;
                let signal: crate::AbortPredicate =
                    Arc::new(move || closed.load(Ordering::SeqCst) || original_signal());
                let sequence = Arc::new(AtomicU64::new(0));
                let mut functions = owner
                    .schemas(agent.as_ref().map(|agent| agent.scope_key()))
                    .into_iter()
                    .filter(|schema| schema.name != crate::RUN_CODE_NAME)
                    .map(|schema| {
                        let name = schema.name.clone();
                        let owner = Arc::clone(&owner);
                        let agent = agent.clone();
                        let root_call_id = root_call_id.clone();
                        let parent_call_id = parent_call_id.clone();
                        let signal = signal.clone();
                        let sequence = Arc::clone(&sequence);
                        let binding_name = name.clone();
                        let function: CodeBindingFunction = Arc::new(move |arguments| {
                            let owner = Arc::clone(&owner);
                            let agent = agent.clone();
                            let root_call_id = root_call_id.clone();
                            let parent_call_id = parent_call_id.clone();
                            let signal = signal.clone();
                            let name = binding_name.clone();
                            let schema=schema.clone();
                            let n = sequence.fetch_add(1, Ordering::Relaxed) + 1;
                            Box::pin(async move {
                                if signal(){return Err("Nested tool execution cancelled before dispatch".into());}
                                let sub_call_id=call_id(format!("{}:code:{n}",parent_call_id.as_str()));
                                let mut log=NestedDispatchLog::begin(agent.as_ref(),json!({"rootCallId":root_call_id,"parentCallId":parent_call_id,"subCallId":sub_call_id,"name":name,"arguments":arguments}),&signal)?;
                                let result=owner.execute_bound(crate::ToolExecutionInput {
                                    call_id:sub_call_id,root_call_id:Some(root_call_id),name:name.clone(),arguments,agent,parent:Some(parent),signal,
                                },schema).await;
                                log.finish(&result)?;
                                if result.is_error {
                                    return Err(result
                                        .error
                                        .as_ref()
                                        .map(|error| error.message.clone())
                                        .unwrap_or_else(|| "tool call failed".into()));
                                }
                                Ok(result.value.clone().unwrap_or(JsonValue::Null))
                            })
                        });
                        (name, function)
                    })
                    .collect::<Vec<_>>();
                functions.sort_by(|left, right| left.0.cmp(&right.0));
                let outcome = code_runtime
                    .run(dsh_code_runtime::CodeRunRequest {
                        timeout_ms: Some(timeout_ms),
                        program: code,
                        bindings: vec![CodeBindingNamespace {
                            global: "tools".to_string(),
                            functions,
                            error_class: Some(CodeBindingErrorClass {
                                name: "ToolCallError".to_string(),
                                member_name_property: "toolName".to_string(),
                            }),
                        }],
                        signal: Some(signal),
                    })
                    .await
                    .map_err(ToolBodyError::plain)?;
                if let Some(error) = outcome.error {
                    return Err(ToolBodyError::coded(
                        format!(
                            "code run failed ({}): {}{}",
                            error.kind.as_str(),
                            error.message,
                            if outcome.logs.is_empty() {
                                String::new()
                            } else {
                                format!("\nCaptured output:\n{}", outcome.logs.join("\n"))
                            }
                        ),
                        "CodeRunFailedError",
                        "CODE_RUN_FAILED",
                    ));
                }
                let mut value = serde_json::Map::new();
                value.insert("logs".to_string(), json!(outcome.logs));
                if let Some(result) = outcome.value {
                    value.insert("result".to_string(), result);
                }
                Ok(JsonValue::Object(value))
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args| {
            Some(ToolCallView::Generic {
                title: args["description"].as_str().unwrap_or_default().to_string(),
                kind: Some(ToolCallKind::Execute),
                raw_input: args.get("code").cloned(),
                content: None,
                locations: None,
            })
        })),
        present_result: None,
    })
}

fn render_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}
