//! Concrete loop-agent tests: Rust port of the core
//! `packages/core/agent-loop/tests/loop.spec.ts` behaviors (one model
//! turn, tool-call round trip, and cancellation).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cordis::Context;
use dsh_agent::Agent;
use dsh_agent_loop::ReactLoopAgent;
use dsh_llm::{
    ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk, TokenUsage,
    ChunkStream, create_user_message, call_id,
};
use dsh_session::{SessionStore, session_id};
use dsh_tools::schema::{
    ParameterPropertySpec, ParameterSchemaSpec, StringValueSchemaSpec, ValueSchemaAnnotations,
    ValueSchemaSpec,
};
use dsh_tools::{
    Config as ToolsConfig, ToolDefinition, ToolOutputDefinition, ToolRuntime,
    parameter_schema_spec_to_json_schema, value_schema_spec_to_json_schema,
};

fn script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart { index: 0, block_type: "text".to_string() },
        StreamChunk::TextDelta { index: 0, text: "hi".to_string() },
        StreamChunk::BlockEnd { index: 0, block: ContentBlock::Text { text: "hi".to_string() } },
        StreamChunk::Usage { usage: TokenUsage { input_tokens: 5, output_tokens: 2, ..TokenUsage::default() } },
        StreamChunk::Finish { reason: FinishReason::Stop, replay_state: None },
    ]
}

struct ScriptedAdapter {
    script: Vec<StreamChunk>,
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(self.script.clone()))
    }
}

/// First call requests the tool; every later call answers with plain text
/// (a fixed tool-call script would otherwise drive the loop forever).
struct ToolThenTextAdapter {
    tool_script: Vec<StreamChunk>,
    text_script: Vec<StreamChunk>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmAdapter for ToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let script = if index == 0 {
            self.tool_script.clone()
        } else {
            self.text_script.clone()
        };
        Box::pin(futures::stream::iter(script))
    }
}

fn tool_call_script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart { index: 0, block_type: "tool-call".to_string() },
        StreamChunk::ToolCallDelta {
            index: 0,
            id: call_id("tc1"),
            name: Some("echo".to_string()),
            arguments_delta: "{\"message\":\"hi\"}".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::ToolCall {
                id: call_id("tc1"),
                name: "echo".to_string(),
                arguments: "{\"message\":\"hi\"}".to_string(),
            },
        },
        StreamChunk::Finish { reason: FinishReason::ToolCalls, replay_state: None },
    ]
}

fn echo_tool() -> ToolDefinition {
    let mut parameters = ParameterSchemaSpec::new();
    parameters.insert(
        "message".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::String(StringValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                enum_: None,
                const_: None,
            }),
            required: true,
        },
    );
    ToolDefinition {
        name: "echo".to_string(),
        description: "echo a message".to_string(),
        parameters: parameter_schema_spec_to_json_schema(&parameters).expect("parameters"),
        output: ToolOutputDefinition {
            schema: value_schema_spec_to_json_schema(&ValueSchemaSpec::String(
                StringValueSchemaSpec {
                    annotations: ValueSchemaAnnotations::default(),
                    enum_: None,
                    const_: None,
                },
            ))
            .expect("output schema"),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text { text: value.as_str().expect("string").to_string() }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|args, _run_ctx| {
            let text = args["message"].as_str().expect("message").to_string();
            Box::pin(async move { Ok(serde_json::json!(text)) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

struct Harness {
    _ctx: Context,
    _store: Arc<SessionStore>,
    tools: Arc<ToolRuntime>,
    agent: Arc<ReactLoopAgent>,
}

async fn harness() -> Harness {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let _llm = LlmRuntime::install(&ctx);
    let tools = ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    let store = SessionStore::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("loop-test")), None)
        .await
        .expect("session");
    let agent = ReactLoopAgent::new(
        &ctx,
        session.id().clone(),
        dsh_agent::AgentOptions {
            provider: Some("test".to_string()),
            model: Some("model".to_string()),
            max_tokens: None,
            subagent_depth: None,
        },
        session,
    )
    .expect("agent");
    Harness { _ctx: ctx, _store: store, tools, agent }
}

fn register_adapter(harness: &Harness, adapter: Arc<dyn LlmAdapter>) {
    let llm = harness
        ._ctx
        .get_typed::<Arc<LlmRuntime>>("llm", false)
        .map(|arc| arc.as_ref().clone())
        .expect("llm");
    llm.register_adapter(&harness._ctx, vec!["test".to_string()], adapter)
        .expect("adapter");
}

fn events_of(harness: &Harness, type_: &str) -> Vec<serde_json::Value> {
    harness
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == type_)
        .map(|event| event.data.clone())
        .collect()
}

fn user_message(text: &str) -> dsh_llm::UserMessage {
    create_user_message(
        vec![ContentBlock::Text { text: text.to_string() }],
        dsh_llm::MessageSource::User { rpc_id: None, client_time_zone: None },
    )
}

#[tokio::test]
async fn drives_one_turn_through_the_model_boundary() {
    let harness = harness().await;
    let _ = harness.tools.register(&harness._ctx, echo_tool()).expect("register");
    register_adapter(&harness, Arc::new(ScriptedAdapter { script: script() }));

    harness.agent.followup(user_message("hello"));
    harness.agent.when_idle().await;

    // The turn envelope and the durable model artifacts are all present.
    assert_eq!(events_of(&harness, "turn/start").len(), 1);
    assert_eq!(events_of(&harness, "step/start").len(), 1);
    assert_eq!(events_of(&harness, "user/message").len(), 1);
    assert_eq!(events_of(&harness, "request/header").len(), 1);
    assert!(!events_of(&harness, "assistant/chunk").is_empty());
    assert_eq!(events_of(&harness, "assistant/message").len(), 1);
    assert_eq!(events_of(&harness, "step/end").len(), 1);
    let turns = events_of(&harness, "turn/end");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["reason"]["kind"], "completed");

    // The assembled assistant message carries the model provenance.
    let message = &events_of(&harness, "assistant/message")[0]["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["source"]["provider"], "test");
    assert_eq!(message["content"][0]["text"], "hi");
}

#[tokio::test]
async fn executes_tool_calls_requested_by_the_model() {
    let harness = harness().await;
    harness
        .tools
        .register(&harness._ctx, echo_tool())
        .expect("register");
    // First call requests the tool; every later call answers with plain
    // text.
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            tool_script: tool_call_script(),
            text_script: script(),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
    );

    harness.agent.followup(user_message("run the tool"));
    harness.agent.when_idle().await;

    // The tool call dispatched and its durable result landed.
    assert_eq!(events_of(&harness, "tool/call").len(), 1);
    let results = events_of(&harness, "tool/result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["message"]["content"][0]["content"][0]["text"], "hi");
    let turns = events_of(&harness, "turn/end");
    assert_eq!(turns[0]["reason"]["kind"], "completed");
}

#[tokio::test]
async fn cancellation_aborts_the_live_turn() {
    let harness = harness().await;
    let _ = harness.tools.register(&harness._ctx, echo_tool()).expect("register");
    register_adapter(&harness, Arc::new(ScriptedAdapter { script: script() }));

    harness.agent.followup(user_message("hello"));
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: false }),
    );
    harness.agent.when_idle().await;

    // Cancellation never lets a turn complete normally: the turn either
    // ends aborted or never opens at all (cancel before the driver started).
    let turns = events_of(&harness, "turn/end");
    assert!(
        turns.is_empty()
            || turns.last().expect("turn end")["reason"]["kind"] == "aborted",
        "got {:?}",
        turns
    );
}
