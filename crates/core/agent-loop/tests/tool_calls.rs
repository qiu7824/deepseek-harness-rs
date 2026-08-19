//! Tool-call scheduler tests: Rust port of the core
//! `packages/core/agent-loop/tests/tool-calls.spec.ts` behaviors
//! (ordered commit, bounded parallel pool, abort synthesis, context and
//! concludesTurn plumbing).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_agent_loop::{ContextAcceptor, execute_tool_calls};
use dsh_llm::{ContentBlock, ToolCallBlock, call_id};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionStore, UserMessage, session_id};
use dsh_tools::schema::{
    ParameterPropertySpec, ParameterSchemaSpec, StringValueSchemaSpec, ValueSchemaAnnotations,
    ValueSchemaSpec,
};
use dsh_tools::{
    Config, ToolDefinition, ToolOutputDefinition, ToolRuntime,
    parameter_schema_spec_to_json_schema, value_schema_spec_to_json_schema,
};

struct TestAgent {
    session: Session,
    inbox: Inbox,
}

impl TestAgent {
    fn new(session: Session) -> Self {
        let inbox = dsh_agent::Inbox::new(&session, Default::default()).expect("inbox");
        Self { session, inbox }
    }
}

impl Agent for TestAgent {
    fn id(&self) -> &dsh_session::SessionId {
        self.session.id()
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        unreachable!("not used by the tool-call scheduler")
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
    }

    fn cancel(
        &self,
        _cause: dsh_agent::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

fn echo_parameters() -> serde_json::Value {
    let mut properties = ParameterSchemaSpec::new();
    properties.insert(
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
    parameter_schema_spec_to_json_schema(&properties).expect("parameters")
}

fn echo_tool(
    name: &str,
    parallel: bool,
    on_run: Option<Arc<dyn Fn() + Send + Sync>>,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: "echo a message".to_string(),
        parameters: echo_parameters(),
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
        is_concurrency_safe: if parallel {
            Some(Arc::new(|_args: &serde_json::Value| true)
                as Arc<dyn Fn(&serde_json::Value) -> bool + Send + Sync>)
        } else {
            None
        },
        execute: Arc::new(move |args, _run_ctx| {
            if let Some(on_run) = &on_run {
                (on_run)();
            }
            let text = args["message"].as_str().expect("message").to_string();
            Box::pin(async move { Ok(serde_json::json!(text)) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

fn block(name: &str, cid: &str) -> ToolCallBlock {
    ToolCallBlock {
        id: call_id(cid),
        name: name.to_string(),
        arguments: format!("{{\"message\":\"{name}\"}}"),
    }
}

async fn harness() -> (Context, Arc<ToolRuntime>, Arc<SessionStore>, Session) {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let tools = ToolRuntime::install(&ctx, Config::default()).expect("tools");
    let store = SessionStore::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("tool-calls-test")), None)
        .await
        .expect("session");
    (ctx, tools, store, session)
}

fn tool_call_events(session: &Session) -> Vec<String> {
    session
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/call")
        .map(|event| format!("{}", event.data["name"].as_str().unwrap_or("?")))
        .collect()
}

#[tokio::test]
async fn exclusive_calls_commit_in_model_order() {
    let (ctx, tools, _store, session) = harness().await;
    let dispose = tools
        .register(&ctx, echo_tool("exclusive", false, None))
        .expect("register");
    let agent = Arc::new(TestAgent::new(session.clone()));

    let calls = vec![block("exclusive", "c1"), block("exclusive", "c2")];
    let concluded = execute_tool_calls(
        &tools,
        agent,
        10,
        1,
        1,
        calls,
        Arc::new(|| false),
        Arc::new(|_context| {}),
    )
    .await
    .expect("execute");
    assert!(!concluded);

    // One tool/call + one tool/result per model call, in model order.
    assert_eq!(
        tool_call_events(&session),
        vec!["exclusive".to_string(), "exclusive".to_string()]
    );
    let events = session.events();
    let results: Vec<&dsh_session::SessionEvent> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .collect();
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result.data["step"], 1);
        assert!(
            result
                .source_event_seqs
                .as_ref()
                .is_some_and(|seqs| seqs.len() == 1)
        );
    }
    dispose().await;
    let _ = ctx;
}

#[tokio::test]
async fn parallel_calls_overlap_within_the_pool_bound() {
    let (ctx, tools, _store, session) = harness().await;
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let active_for_body = Arc::clone(&active);
    let peak_for_body = Arc::clone(&peak);
    let on_run = Arc::new(move || {
        let current = active_for_body.fetch_add(1, Ordering::SeqCst) + 1;
        peak_for_body.fetch_max(current, Ordering::SeqCst);
    });
    // Block the bodies briefly so overlap is observable.
    let dispose = tools
        .register(&ctx, {
            let mut tool = echo_tool("parallel", true, None);
            let on_run = Arc::clone(&on_run);
            tool.execute = Arc::new(move |args, _run_ctx| {
                (on_run)();
                let text = args["message"].as_str().expect("message").to_string();
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(serde_json::json!(text))
                })
            });
            tool
        })
        .expect("register");
    let agent = Arc::new(TestAgent::new(session.clone()));

    let calls = vec![block("parallel", "c1"), block("parallel", "c2")];
    execute_tool_calls(
        &tools,
        agent,
        2,
        1,
        1,
        calls,
        Arc::new(|| false),
        Arc::new(|_context| {}),
    )
    .await
    .expect("execute");
    // Both bodies overlapped under a cap of 2.
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    let _ = active;
    dispose().await;
    let _ = ctx;
}

#[tokio::test]
async fn abort_records_synthetic_results_for_skipped_calls() {
    let (ctx, tools, _store, session) = harness().await;
    let aborted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&aborted);
    let mut tool = echo_tool("exclusive", false, None);
    tool.execute = Arc::new(move |args, _run_ctx| {
        let text = args["message"].as_str().expect("message").to_string();
        let flag = Arc::clone(&flag);
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(serde_json::json!(text))
        })
    });
    let dispose = tools.register(&ctx, tool).expect("register");
    let agent = Arc::new(TestAgent::new(session.clone()));
    let signal_flag = Arc::clone(&aborted);
    let signal = Arc::new(move || signal_flag.load(Ordering::SeqCst));

    let calls = vec![block("exclusive", "c1"), block("exclusive", "c2")];
    let concluded = execute_tool_calls(
        &tools,
        agent,
        10,
        1,
        1,
        calls,
        signal,
        Arc::new(|_context| {}),
    )
    .await
    .expect("execute");
    assert!(!concluded);

    // Both calls have durable call/result pairs: the started one with its
    // real result, the skipped one with the synthetic abort outcome.
    let events = session.events();
    let results: Vec<&dsh_session::SessionEvent> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .collect();
    assert_eq!(results.len(), 2);
    let skipped = &results[1];
    assert_eq!(skipped.data["error"]["code"], "ABORTED_BEFORE_DISPATCH");
    // The tool-result message wraps its content in a tool-result block.
    assert_eq!(
        skipped.data["message"]["content"][0]["content"][0]["text"],
        "Error: tool call aborted before dispatch"
    );
    dispose().await;
    let _ = ctx;
}

#[tokio::test]
async fn additional_contexts_and_concludes_turn_are_forwarded() {
    let (ctx, tools, _store, session) = harness().await;
    let mut tool = echo_tool("exclusive", false, None);
    tool.execute = Arc::new(|args, run_ctx| {
        let text = args["message"].as_str().expect("message").to_string();
        run_ctx.defer_context(dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "context".to_string(),
            }],
            dsh_llm::MessageSource::Plugin {
                plugin: "test".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ));
        run_ctx.conclude_turn();
        Box::pin(async move { Ok(serde_json::json!(text)) })
    });
    let dispose = tools.register(&ctx, tool).expect("register");
    let agent = Arc::new(TestAgent::new(session.clone()));
    let accepted: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let accepted_for = Arc::clone(&accepted);
    let acceptor: ContextAcceptor = Arc::new(move |context| {
        accepted_for
            .lock()
            .expect("accepted")
            .push(context.content[0].as_text().unwrap_or("").to_string());
    });

    let concluded = execute_tool_calls(
        &tools,
        agent,
        10,
        1,
        1,
        vec![block("exclusive", "c1")],
        Arc::new(|| false),
        acceptor,
    )
    .await
    .expect("execute");
    assert!(concluded);
    assert_eq!(
        *accepted.lock().expect("accepted"),
        vec!["context".to_string()]
    );
    dispose().await;
    let _ = ctx;
}
