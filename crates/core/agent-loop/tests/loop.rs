//! Concrete loop-agent tests: Rust port of the core
//! `packages/core/agent-loop/tests/loop.spec.ts` behaviors (one model
//! turn, tool-call round trip, and cancellation).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use dsh_agent::{Agent, AgentRegistry};
use dsh_agent_loop::ReactLoopAgent;
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk,
    TokenUsage, call_id, create_user_message,
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
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::TextDelta {
            index: 0,
            text: "hi".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "hi".to_string(),
            },
        },
        StreamChunk::Usage {
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 2,
                ..TokenUsage::default()
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
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

struct GatedAdapter {
    entered: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

struct InterruptedThenRecordAdapter {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    first_emitted: Arc<AtomicBool>,
    recorded: Arc<parking_lot::Mutex<Option<GenerateOptions>>>,
}

impl LlmAdapter for InterruptedThenRecordAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let first_emitted = self.first_emitted.clone();
            Box::pin(futures::stream::unfold(0_u8, move |state| {
                let first_emitted = first_emitted.clone();
                async move {
                    match state {
                        0 => Some((
                            StreamChunk::BlockStart {
                                index: 0,
                                block_type: "text".to_string(),
                            },
                            1,
                        )),
                        1 => {
                            first_emitted.store(true, Ordering::SeqCst);
                            Some((
                                StreamChunk::TextDelta {
                                    index: 0,
                                    text: "visible prefix".to_string(),
                                },
                                2,
                            ))
                        }
                        _ => futures::future::pending().await,
                    }
                }
            }))
        } else {
            *self.recorded.lock() = Some(options.clone());
            Box::pin(futures::stream::iter(script()))
        }
    }
}

impl LlmAdapter for GatedAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let entered = self.entered.clone();
        let released = self.released.clone();
        Box::pin(futures::stream::once(async move {
            entered.store(true, Ordering::SeqCst);
            while !released.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            }
        }))
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
        StreamChunk::BlockStart {
            index: 0,
            block_type: "tool-call".to_string(),
        },
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
        StreamChunk::Finish {
            reason: FinishReason::ToolCalls,
            replay_state: None,
        },
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
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().expect("string").to_string(),
                }])
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
    agents: Arc<AgentRegistry>,
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
    let agents = AgentRegistry::install(&ctx);
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
    let agent_dyn: Arc<dyn Agent> = agent.clone();
    agents.enter(agent_dyn, None).expect("enter root agent");
    Harness {
        _ctx: ctx,
        _store: store,
        agents,
        tools,
        agent,
    }
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
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    )
}

#[tokio::test]
async fn drives_one_turn_through_the_model_boundary() {
    let harness = harness().await;
    let _ = harness
        .tools
        .register(&harness._ctx, echo_tool())
        .expect("register");
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
    assert_eq!(
        results[0]["message"]["content"][0]["content"][0]["text"],
        "hi"
    );
    let turns = events_of(&harness, "turn/end");
    assert_eq!(turns[0]["reason"]["kind"], "completed");
}

#[tokio::test]
async fn real_driver_exposes_the_exact_agent_as_ambient_initiator() {
    let harness = harness().await;
    let saw_exact = Arc::new(AtomicBool::new(false));
    let saw_exact_for_tool = saw_exact.clone();
    let agents = harness.agents.clone();
    let mut tool = echo_tool();
    tool.execute = Arc::new(move |args, run_ctx| {
        let exact = run_ctx.agent.as_ref().is_some_and(|agent| {
            agents
                .current_initiator()
                .ok()
                .flatten()
                .is_some_and(|initiator| Arc::ptr_eq(&initiator, agent))
        });
        saw_exact_for_tool.store(exact, Ordering::SeqCst);
        let text = args["message"].as_str().expect("message").to_string();
        Box::pin(async move { Ok(serde_json::json!(text)) })
    });
    harness
        .tools
        .register(&harness._ctx, tool)
        .expect("register");
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            tool_script: tool_call_script(),
            text_script: script(),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
    );

    harness
        .agent
        .followup(user_message("run under the real driver"));
    harness.agent.when_idle().await;

    assert!(
        saw_exact.load(Ordering::SeqCst),
        "the full ReactLoopAgent driver must carry its exact ambient initiator"
    );
}

#[tokio::test]
async fn maintenance_handoff_keeps_when_idle_bound_to_the_latched_turn() {
    let harness = harness().await;
    let entered = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    register_adapter(
        &harness,
        Arc::new(GatedAdapter {
            entered: entered.clone(),
            released: released.clone(),
        }),
    );

    let agent = harness.agent.clone();
    let maintenance = harness.agent.run_maintenance(Arc::new(move || {
        let agent = agent.clone();
        Box::pin(async move {
            agent.followup(user_message("queued during maintenance"));
        })
    }));
    maintenance.await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("latched turn should enter the model");

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            harness.agent.when_idle(),
        )
        .await
        .is_err(),
        "when_idle must not resolve while the latched turn is still running"
    );

    released.store(true, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(1), harness.agent.when_idle())
        .await
        .expect("when_idle should resolve after the latched turn settles");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_listener_started_maintenance_keeps_when_idle_pending() {
    let harness = harness().await;
    let triggered = Arc::new(AtomicBool::new(false));
    let second_entered = Arc::new(AtomicBool::new(false));
    let second_released = Arc::new(AtomicBool::new(false));
    let triggered_listener = triggered.clone();
    let entered_listener = second_entered.clone();
    let released_listener = second_released.clone();
    harness
        ._ctx
        .on(
            "agent/status",
            Arc::new(move |_ctx, args| {
                let triggered = triggered_listener.clone();
                let entered = entered_listener.clone();
                let released = released_listener.clone();
                Box::pin(async move {
                    let payload = args
                        .first()
                        .and_then(|value| value.downcast_ref::<dsh_agent::AgentStatusPayload>())
                        .expect("status payload");
                    if payload.status == dsh_agent::AgentStatus::Idle
                        && !triggered.swap(true, Ordering::SeqCst)
                    {
                        let maintenance = payload.agent.run_maintenance(Arc::new(move || {
                            let entered = entered.clone();
                            let released = released.clone();
                            Box::pin(async move {
                                entered.store(true, Ordering::SeqCst);
                                while !released.load(Ordering::SeqCst) {
                                    tokio::task::yield_now().await;
                                }
                            })
                        }));
                        tokio::spawn(maintenance);
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;

    register_adapter(&harness, Arc::new(ScriptedAdapter { script: script() }));
    harness.agent.followup(user_message("open the first turn"));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !second_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("idle listener must start the second maintenance");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            harness.agent.when_idle(),
        )
        .await
        .is_err(),
        "the first maintenance must not clear the activity opened by its idle listener"
    );

    second_released.store(true, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(1), harness.agent.when_idle())
        .await
        .expect("second maintenance should settle after release");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn followup_queued_while_running_latches_the_next_turn() {
    let harness = harness().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            tool_script: script(),
            text_script: script(),
            calls: calls.clone(),
        }),
    );
    let queued = Arc::new(AtomicBool::new(false));
    let queued_for_listener = queued.clone();
    harness
        ._ctx
        .on(
            "agent/turn-stopping",
            Arc::new(move |_ctx, args| {
                let queued = queued_for_listener.clone();
                Box::pin(async move {
                    let payload = args
                        .first()
                        .and_then(|value| {
                            value.downcast_ref::<dsh_agent::AgentTurnStoppingPayload>()
                        })
                        .expect("turn-stopping payload");
                    if !queued.swap(true, Ordering::SeqCst) {
                        payload
                            .agent
                            .followup(user_message("queued at the running tail"));
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;

    harness.agent.followup(user_message("first turn"));
    tokio::time::timeout(std::time::Duration::from_secs(1), harness.agent.when_idle())
        .await
        .expect("both turns should settle");

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(harness.agent.inbox().next_turn().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_an_unpolled_maintenance_future_restores_idle() {
    let harness = harness().await;
    let maintenance = harness.agent.run_maintenance(Arc::new(|| {
        Box::pin(async {
            futures::future::pending::<()>().await;
        })
    }));
    drop(maintenance);

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        harness.agent.when_idle(),
    )
    .await
    .expect("dropping maintenance must settle its activity");
    assert_eq!(harness.agent.status(), dsh_agent::AgentStatus::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_a_pending_model_stream() {
    let harness = harness().await;
    let entered = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    register_adapter(
        &harness,
        Arc::new(GatedAdapter {
            entered: entered.clone(),
            released,
        }),
    );

    harness.agent.followup(user_message("wait forever"));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model stream must be polled");

    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: false }),
    );
    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        harness.agent.when_idle(),
    )
    .await
    .expect("cancellation must interrupt a pending model stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_persists_visible_assistant_prefix_for_the_next_request() {
    let harness = harness().await;
    let first_emitted = Arc::new(AtomicBool::new(false));
    let recorded = Arc::new(parking_lot::Mutex::new(None));
    register_adapter(
        &harness,
        Arc::new(InterruptedThenRecordAdapter {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            first_emitted: first_emitted.clone(),
            recorded: recorded.clone(),
        }),
    );

    harness.agent.followup(user_message("first"));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !harness.agent.session().events().iter().any(|event| {
            event.type_ == "assistant/chunk" && event.data["chunk"]["text"] == "visible prefix"
        }) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("visible assistant prefix must be durably projected before cancellation");
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: false }),
    );
    harness.agent.when_idle().await;

    let events = harness.agent.session().events();
    let interrupted = events
        .iter()
        .find(|event| {
            event.type_ == "assistant/message"
                && event.data["interrupted"] == serde_json::Value::Bool(true)
        })
        .expect("interrupted assistant message is durable");
    assert_eq!(
        interrupted.data["message"]["content"][0]["text"],
        "visible prefix"
    );

    harness.agent.followup(user_message("second"));
    harness.agent.when_idle().await;
    let options = recorded.lock().clone().expect("second request recorded");
    assert!(options.messages.iter().any(|message| {
        message.role == dsh_llm::Role::Assistant
            && message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text } if text == "visible prefix"
                )
            })
    }));
}

#[tokio::test]
async fn cancellation_aborts_the_live_turn() {
    let harness = harness().await;
    let _ = harness
        .tools
        .register(&harness._ctx, echo_tool())
        .expect("register");
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
        turns.is_empty() || turns.last().expect("turn end")["reason"]["kind"] == "aborted",
        "got {:?}",
        turns
    );
}
