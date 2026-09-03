use cordis::Context;
use dsh_agent::{Agent, AgentRegistry};
use dsh_agent_loop::ReactLoopAgent;
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmFailure, LlmRuntime,
    StreamChunk, call_id, create_user_message,
};
use dsh_session::{SessionStore, session_id};
use dsh_tools::schema::{
    ParameterSchemaSpec, ValueSchemaAnnotations, ValueSchemaSpec,
    parameter_schema_spec_to_json_schema, value_schema_spec_to_json_schema,
};
use dsh_tools::{Config as ToolsConfig, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct ErrorAdapter;

impl LlmAdapter for ErrorAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: LlmFailure {
                    message: "GPT API stream failed: HTTP response body failed".to_string(),
                    code: "TRANSPORT".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        }]))
    }
}

pub(crate) struct BlockingFirstTurnAdapter {
    pub(crate) calls: std::sync::atomic::AtomicUsize,
    pub(crate) first_entered: Arc<tokio::sync::Notify>,
    pub(crate) release_first: Arc<tokio::sync::Notify>,
}

impl LlmAdapter for BlockingFirstTurnAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let entered = Arc::clone(&self.first_entered);
            let release = Arc::clone(&self.release_first);
            // Retain a permit if the test has not begun waiting yet; a
            // broadcast-only edge would make this harness scheduler-dependent.
            entered.notify_one();
            let chunks = vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "first".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "first".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ];
            return Box::pin(
                futures::stream::once(async move {
                    release.notified().await;
                    chunks
                })
                .flat_map(futures::stream::iter),
            );
        }
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".to_string(),
            },
            StreamChunk::TextDelta {
                index: 0,
                text: "second".to_string(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "second".to_string(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

pub(crate) struct ToolThenTextAdapter {
    pub(crate) calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for ToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let chunks = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let id = call_id("hang-1");
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: id.clone(),
                    name: Some("hang".to_string()),
                    arguments_delta: "{}".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id,
                        name: "hang".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "done".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "done".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(chunks))
    }
}

pub(crate) struct NamedToolThenTextAdapter {
    pub(crate) name: &'static str,
    pub(crate) calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for NamedToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let chunks = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let id = call_id("named-1");
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: id.clone(),
                    name: Some(self.name.to_string()),
                    arguments_delta: "{}".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id,
                        name: self.name.to_string(),
                        arguments: "{}".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "done".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "done".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(chunks))
    }
}

pub(crate) struct Harness {
    pub(crate) ctx: Context,
    _store: Arc<SessionStore>,
    pub(crate) agent: Arc<ReactLoopAgent>,
    pub(crate) tools: Arc<ToolRuntime>,
}

pub(crate) async fn harness() -> Harness {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("system prompt");
    LlmRuntime::install(&ctx);
    let tools = ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    let store = SessionStore::install(&ctx);
    let agents = AgentRegistry::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("lifecycle-failure")), None)
        .await
        .expect("session");
    let agent = ReactLoopAgent::new(
        &ctx,
        session.id().clone(),
        dsh_agent::AgentOptions {
            provider: Some("test".to_string()),
            model: Some("model".to_string()),
            ..Default::default()
        },
        session,
    )
    .expect("agent");
    let agent_dyn: Arc<dyn Agent> = agent.clone();
    agents.enter(agent_dyn, None).expect("enter agent");
    Harness {
        ctx,
        _store: store,
        agent,
        tools,
    }
}

pub(crate) fn register_adapter(harness: &Harness, adapter: Arc<dyn LlmAdapter>) {
    let llm = harness
        .ctx
        .get_typed::<Arc<LlmRuntime>>("llm", false)
        .map(|slot| slot.as_ref().clone())
        .expect("llm");
    llm.register_adapter(&harness.ctx, vec!["test".to_string()], adapter)
        .expect("register adapter");
}

pub(crate) fn message(text: &str) -> dsh_llm::UserMessage {
    create_user_message(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    )
}

pub(crate) fn turn_end_kinds(agent: &ReactLoopAgent) -> Vec<String> {
    agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "turn/end")
        .filter_map(|event| event.data["reason"]["kind"].as_str().map(str::to_string))
        .collect()
}

pub(crate) struct DropFlag(pub(crate) Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn hanging_tool(entered: Arc<AtomicBool>, dropped: Arc<AtomicBool>) -> ToolDefinition {
    ToolDefinition {
        name: "hang".to_string(),
        description: "hang until cancelled".to_string(),
        parameters: parameter_schema_spec_to_json_schema(&ParameterSchemaSpec::new())
            .expect("parameters"),
        output: ToolOutputDefinition {
            schema: value_schema_spec_to_json_schema(&ValueSchemaSpec::String(
                dsh_tools::schema::StringValueSchemaSpec {
                    annotations: ValueSchemaAnnotations::default(),
                    enum_: None,
                    const_: None,
                },
            ))
            .expect("output"),
            render: Arc::new(|_, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().unwrap_or_default().to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |_, _| {
            let entered = Arc::clone(&entered);
            let dropped = Arc::clone(&dropped);
            Box::pin(async move {
                let _drop_flag = DropFlag(dropped);
                entered.store(true, Ordering::SeqCst);
                futures::future::pending().await
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

pub(crate) fn quick_tool(entered: Arc<AtomicBool>) -> ToolDefinition {
    ToolDefinition {
        name: "quick".to_string(),
        description: "complete immediately".to_string(),
        parameters: parameter_schema_spec_to_json_schema(&ParameterSchemaSpec::new())
            .expect("parameters"),
        output: ToolOutputDefinition {
            schema: value_schema_spec_to_json_schema(&ValueSchemaSpec::String(
                dsh_tools::schema::StringValueSchemaSpec {
                    annotations: ValueSchemaAnnotations::default(),
                    enum_: None,
                    const_: None,
                },
            ))
            .expect("output"),
            render: Arc::new(|_, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().unwrap_or_default().to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: Some(Arc::new(|_| true)),
        execute: Arc::new(move |_, _| {
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                entered.store(true, Ordering::SeqCst);
                Ok(serde_json::json!("quick"))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

pub(crate) fn quick_context_tool(entered: Arc<AtomicBool>) -> ToolDefinition {
    let mut tool = quick_tool(Arc::clone(&entered));
    tool.name = "quick-context".to_string();
    tool.execute = Arc::new(move |_, run| {
        run.defer_context(message("quick-context"));
        run.conclude_turn();
        let entered = Arc::clone(&entered);
        Box::pin(async move {
            entered.store(true, Ordering::SeqCst);
            Ok(serde_json::json!("quick"))
        })
    });
    tool
}

pub(crate) struct ParallelToolAdapter;

impl LlmAdapter for ParallelToolAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let slow = call_id("slow");
        let quick = call_id("quick");
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "tool-call".to_string(),
            },
            StreamChunk::ToolCallDelta {
                index: 0,
                id: slow.clone(),
                name: Some("hang".to_string()),
                arguments_delta: "{}".to_string(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::ToolCall {
                    id: slow,
                    name: "hang".to_string(),
                    arguments: "{}".to_string(),
                },
            },
            StreamChunk::BlockStart {
                index: 1,
                block_type: "tool-call".to_string(),
            },
            StreamChunk::ToolCallDelta {
                index: 1,
                id: quick.clone(),
                name: Some("quick".to_string()),
                arguments_delta: "{}".to_string(),
            },
            StreamChunk::BlockEnd {
                index: 1,
                block: ContentBlock::ToolCall {
                    id: quick,
                    name: "quick".to_string(),
                    arguments: "{}".to_string(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::ToolCalls,
                replay_state: None,
            },
        ]))
    }
}
