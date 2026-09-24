use super::*;
use dsh_agent::{AgentFactory, AgentOptions, CreateAgentOptions};
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter, StreamChunk, call_id};
use dsh_tools::{ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct Shell;
impl dsh_shell::ShellExecutor for Shell {
    fn sandbox_mode(&self) -> Option<dsh_sandbox::SandboxMode> {
        Some(dsh_sandbox::SandboxMode::WorkspaceWrite)
    }
    fn resolve(&self, _: dsh_shell::ShellExecRequest) -> dsh_shell::ShellExecSpec {
        panic!("unused shell")
    }
    fn run(
        &self,
        _: dsh_shell::ShellExecSpec,
    ) -> cordis::BoxFuture<'static, Result<dsh_shell::ShellRunResult, String>> {
        panic!("unused shell")
    }
    fn start(&self, _: dsh_shell::ShellExecSpec) -> Arc<dyn dsh_shell::ShellProcess> {
        panic!("unused shell")
    }
}
struct Code;
impl dsh_code_runtime::CodeRuntime for Code {
    fn language(&self) -> String {
        "typescript".into()
    }
    fn isolation(&self) -> String {
        "fixture".into()
    }
    fn run(
        &self,
        request: dsh_code_runtime::CodeRunRequest,
    ) -> cordis::BoxFuture<'static, Result<dsh_code_runtime::CodeRunResult, String>> {
        Box::pin(async move {
            let function = &request.bindings[0]
                .functions
                .iter()
                .find(|(name, _)| name == "effect")
                .unwrap()
                .1;
            Ok(match function(json!({})).await {
                Ok(value) => dsh_code_runtime::CodeRunResult {
                    value: Some(value),
                    ..Default::default()
                },
                Err(message) => dsh_code_runtime::CodeRunResult {
                    error: Some(dsh_code_runtime::CodeRunFailure {
                        kind: dsh_code_runtime::CodeRunFailureKind::Exception,
                        message,
                    }),
                    ..Default::default()
                },
            })
        })
    }
}
struct Adapter {
    reply: Mutex<String>,
    review_requests: Mutex<Vec<String>>,
    main_requests: Mutex<Vec<String>>,
    main_calls: AtomicUsize,
    entered: tokio::sync::Notify,
    code: bool,
}
fn response(blocks: Vec<ContentBlock>, reason: FinishReason) -> ChunkStream {
    let mut chunks = Vec::new();
    for (index, block) in blocks.into_iter().enumerate() {
        let kind = if matches!(block, ContentBlock::ToolCall { .. }) {
            "tool-call"
        } else {
            "text"
        };
        chunks.push(StreamChunk::BlockStart {
            index: index as u64,
            block_type: kind.into(),
        });
        chunks.push(StreamChunk::BlockEnd {
            index: index as u64,
            block,
        });
    }
    chunks.push(StreamChunk::Finish {
        reason,
        replay_state: None,
    });
    Box::pin(futures::stream::iter(chunks))
}
impl LlmAdapter for Adapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        let body = serde_json::to_string(&options.messages).unwrap();
        if options.purpose.as_deref() == Some("auto-review") {
            assert_eq!(options.system.as_deref(), Some(POLICY));
            assert!(options.max_tokens.is_none());
            assert!(options.tools.is_none());
            self.review_requests.lock().push(body);
            self.entered.notify_one();
            let reply = self.reply.lock().clone();
            if reply == "pending" {
                return Box::pin(futures::stream::pending());
            }
            return response(vec![ContentBlock::Text { text: reply }], FinishReason::Stop);
        }
        self.main_requests.lock().push(body);
        let n = self.main_calls.fetch_add(1, Ordering::SeqCst);
        if n % 2 == 0 {
            response(
                vec![
                    ContentBlock::Text {
                        text: "ASSISTANT_CANNOT_GRANT".into(),
                    },
                    ContentBlock::ToolCall {
                        id: call_id(format!("root-{n}")),
                        name: if self.code { "run_code" } else { "effect" }.into(),
                        arguments: if self.code {
                            json!({"code":"return await tools.effect({});","description":"Run scoped effect"}).to_string()
                        } else {
                            "{}".into()
                        },
                    },
                ],
                FinishReason::ToolCalls,
            )
        } else {
            response(
                vec![ContentBlock::Text {
                    text: "complete".into(),
                }],
                FinishReason::Stop,
            )
        }
    }
}
fn effect(runs: Arc<AtomicUsize>) -> ToolDefinition {
    ToolDefinition {
        name: "effect".into(),
        description: "Write the fixture counter".into(),
        parameters: json!({"type":"object","additionalProperties":false}),
        output: ToolOutputDefinition {
            schema: json!({"type":"string"}),
            render: Arc::new(|_, _| {
                Ok(vec![ContentBlock::Text {
                    text: "TOOL_RESULT_CANNOT_GRANT".into(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |_, _| {
            runs.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("done")) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}
struct Fixture {
    ctx: Context,
    permissions: Arc<PermissionPresetService>,
    review: Arc<AutoReview>,
    agent: dsh_agent::AgentHandle,
    adapter: Arc<Adapter>,
    runs: Arc<AtomicUsize>,
    tools: Arc<ToolRuntime>,
}
impl Fixture {
    async fn new(code: bool) -> Self {
        let ctx = Context::root();
        let prompt = dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        prompt.section(
            &ctx,
            dsh_system_prompt::PromptSection {
                name: "test-policy".into(),
                order: 0.0,
                text: dsh_system_prompt::PromptText::Static("MAIN_SYSTEM_CANNOT_GRANT".into()),
                complete: None,
            },
        );
        let shell: Arc<dyn dsh_shell::ShellExecutor> = Arc::new(Shell);
        ctx.register_service(shell);
        if code {
            let runtime: Arc<dyn dsh_code_runtime::CodeRuntime> = Arc::new(Code);
            ctx.register_service(runtime);
        }
        let llm = LlmRuntime::install(&ctx);
        let adapter = Arc::new(Adapter {
            reply: Mutex::new(r#"{"risk":"low","decision":"allow"}"#.into()),
            review_requests: Default::default(),
            main_requests: Default::default(),
            main_calls: AtomicUsize::new(0),
            entered: Default::default(),
            code,
        });
        llm.register_adapter(&ctx, vec!["fixture".into()], adapter.clone())
            .unwrap();
        let tools = ToolRuntime::install(
            &ctx,
            dsh_tools::Config {
                mode: code.then_some(dsh_tools::ToolPresentationMode::Code),
                ..Default::default()
            },
        )
        .unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        tools.register(&ctx, effect(runs.clone())).unwrap();
        SessionStore::install(&ctx);
        dsh_agent::AgentRegistry::install(&ctx);
        dsh_user_approval::ApprovalService::install(&ctx, Default::default());
        dsh_sandbox_policy::SandboxPolicyService::install(&ctx, Default::default());
        let permissions = PermissionPresetService::install(&ctx, Default::default()).unwrap();
        permissions.ready().await.unwrap();
        assert!(!permissions.names().contains(&AUTO_PRESET));
        let review = AutoReview::install(&ctx).await.unwrap();
        assert!(permissions.names().contains(&AUTO_PRESET));
        assert_ne!(permissions.default_preset(), AUTO_PRESET);
        let loops = dsh_agent_loop::AgentLoop::install(&ctx, Default::default()).unwrap();
        let agent = loops
            .create_agent(
                &ctx,
                CreateAgentOptions {
                    meta: Some(dsh_session::CreateSessionMeta {
                        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
                        ..Default::default()
                    }),
                    agent_options: Some(AgentOptions {
                        provider: Some("fixture".into()),
                        model: Some("model".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        permissions.set(agent.agent.session(), AUTO_PRESET).unwrap();
        Self {
            ctx,
            permissions,
            review,
            agent,
            adapter,
            runs,
            tools,
        }
    }
    fn send(&self) {
        self.agent.agent.followup(dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "HUMAN_TASK: run the fixture effect".into(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: Some("human-request".into()),
                client_time_zone: None,
            },
        ));
    }
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(4), self.agent.agent.when_idle())
            .await
            .unwrap();
    }
    async fn close(self) {
        self.review.shutdown().await;
        self.agent.dispose.await;
        for d in self.ctx.fiber.disposables.clear() {
            d().await;
        }
    }
}
#[tokio::test]
async fn native_review_executes_only_allowed_body_and_keeps_reason_ui_only() {
    let f = Fixture::new(false).await;
    f.send();
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 1);
    *f.adapter.reply.lock() =
        r#"{"risk":"medium","decision":"deny","reason":"UI_ONLY_REASON"}"#.into();
    f.send();
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 1);
    let requests = f.adapter.review_requests.lock().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("human-instruction"));
    assert!(requests[0].contains("HUMAN_TASK"));
    for request in requests {
        assert!(!request.contains("ASSISTANT_CANNOT_GRANT"));
        assert!(!request.contains("MAIN_SYSTEM_CANNOT_GRANT"));
        assert!(!request.contains("TOOL_RESULT_CANNOT_GRANT"));
    }
    assert!(
        f.agent
            .agent
            .session()
            .events()
            .iter()
            .any(|e| e.type_ == "tool/result"
                && e.data["meta"]["autoReview"]["reason"] == "UI_ONLY_REASON")
    );
    assert!(
        !f.adapter
            .main_requests
            .lock()
            .iter()
            .any(|body| body.contains("UI_ONLY_REASON"))
    );
    f.close().await;
}
#[tokio::test]
async fn nested_calls_are_reviewed_once_and_have_durable_start_and_result() {
    let f = Fixture::new(true).await;
    *f.adapter.reply.lock() =
        r#"{"risk":"high","decision":"deny","reason":"NESTED_UI_REASON"}"#.into();
    f.send();
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 0);
    assert_eq!(f.adapter.review_requests.lock().len(), 1);
    let events = f.agent.agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.type_ == "tool/ptc-dispatch-start")
            .count(),
        1
    );
    validate_native(f.agent.agent.session());
    let end = events
        .iter()
        .find(|e| e.type_ == "tool/ptc-dispatch")
        .unwrap();
    assert_eq!(end.data["error"]["code"], "AUTO_REVIEW_DENIED");
    assert_eq!(end.data["meta"]["autoReview"]["reason"], "NESTED_UI_REASON");
    assert!(
        !f.adapter
            .main_requests
            .lock()
            .iter()
            .any(|body| body.contains("NESTED_UI_REASON"))
    );
    f.close().await;
}
#[tokio::test]
async fn unload_cancels_stalled_review_and_cold_auto_requires_integration() {
    let f = Fixture::new(false).await;
    *f.adapter.reply.lock() = "pending".into();
    f.send();
    tokio::time::timeout(Duration::from_secs(3), f.adapter.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.review.shutdown())
        .await
        .unwrap();
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.agent
            .agent
            .session()
            .with_events(|e| f.permissions.current(e)),
        "danger-full-access"
    );
    assert!(!f.permissions.names().contains(&AUTO_PRESET));
    f.send();
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 1);
    f.agent
        .agent
        .session()
        .append("permission/preset", json!({"preset":"auto"}), None)
        .unwrap();
    assert!(
        f.permissions
            .pin_initial_permission(f.agent.agent.session())
            .is_err()
    );
    let rejected = f
        .tools
        .execute(dsh_tools::ToolExecutionInput {
            call_id: call_id("unavailable-auto"),
            root_call_id: None,
            name: "effect".into(),
            arguments: json!({}),
            agent: Some(f.agent.agent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    assert!(rejected.is_error);
    assert_eq!(f.runs.load(Ordering::SeqCst), 1);
    let reenabled = AutoReview::install(&f.ctx).await.unwrap();
    reenabled.shutdown().await;
    f.close().await;
}
#[tokio::test]
async fn changed_tool_body_cannot_use_a_prepared_authorization() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let dispose = tools.register(&ctx, effect(runs.clone())).unwrap();
    let prepared = tools
        .prepare_scheduled(dsh_tools::ToolExecutionInput {
            call_id: call_id("binding"),
            root_call_id: None,
            name: "effect".into(),
            arguments: json!({}),
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    let dsh_tools::Preparation::Dispatch { run_ctx } = prepared else {
        panic!("expected prepared call")
    };
    dispose().await;
    tools.register(&ctx, effect(runs.clone())).unwrap();
    let result = match tools.dispatch_scheduled(run_ctx).await {
        dsh_tools::DispatchOutcome::PostResult(result)
        | dsh_tools::DispatchOutcome::FinalResult(result) => result,
    };
    assert!(result.is_error);
    assert_eq!(
        result.error.as_ref().unwrap().info.as_ref().unwrap().code,
        "TOOL_BINDING_CHANGED"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn in_process_child_inherits_auto_and_reviews_its_own_action() {
    let f = Fixture::new(false).await;
    *f.adapter.reply.lock() = r#"{"risk":"medium","decision":"deny"}"#.into();
    let runtime = dsh_subagent::SubagentRuntime::install(&f.ctx);
    dsh_subagent_spawn_in_process::apply(&f.ctx, &Default::default()).unwrap();
    let run = runtime
        .start(
            "spawn",
            dsh_subagent::SubagentStartRequest {
                label: Some("review child".into()),
                prompt: vec![ContentBlock::Text {
                    text: "DIRECT_PARENT_TASK: run the effect".into(),
                }],
                parent: f.agent.agent.clone(),
                signal: Arc::new(|| false),
                agent_options: None,
                output_schema: None,
                max_depth: Some(1),
                tool_filter: None,
                persona: None,
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(4), run.result())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 0);
    let requests = f.adapter.review_requests.lock().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("direct-parent-instruction"));
    assert!(requests[0].contains("DIRECT_PARENT_TASK"));
    run.dispose().await.unwrap();
    f.close().await;
}
#[tokio::test]
async fn user_stop_drops_an_uncooperative_reviewer_without_running_the_body() {
    let f = Fixture::new(false).await;
    *f.adapter.reply.lock() = "pending".into();
    f.send();
    tokio::time::timeout(Duration::from_secs(3), f.adapter.entered.notified())
        .await
        .unwrap();
    f.agent
        .agent
        .cancel(dsh_agent::AgentCancelCause::User, None);
    f.idle().await;
    assert_eq!(f.runs.load(Ordering::SeqCst), 0);
    assert_eq!(f.review.state.activity.lock().count, 0);
    assert_eq!(
        f.agent
            .agent
            .session()
            .with_events(|events| f.permissions.current(events)),
        AUTO_PRESET
    );
    f.close().await;
}

struct EarlyReturnCode {
    adapter: Arc<Adapter>,
    done: Arc<tokio::sync::Notify>,
}
impl dsh_code_runtime::CodeRuntime for EarlyReturnCode {
    fn language(&self) -> String {
        "typescript".into()
    }
    fn isolation(&self) -> String {
        "fixture".into()
    }
    fn run(
        &self,
        request: dsh_code_runtime::CodeRunRequest,
    ) -> cordis::BoxFuture<'static, Result<dsh_code_runtime::CodeRunResult, String>> {
        let adapter = self.adapter.clone();
        let done = self.done.clone();
        Box::pin(async move {
            let function = request.bindings[0]
                .functions
                .iter()
                .find(|(name, _)| name == "effect")
                .unwrap()
                .1
                .clone();
            tokio::spawn(async move {
                let _ = function(json!({})).await;
                done.notify_one();
            });
            adapter.entered.notified().await;
            Ok(dsh_code_runtime::CodeRunResult {
                value: Some(json!("returned before awaiting child")),
                ..Default::default()
            })
        })
    }
}
#[tokio::test]
async fn completed_code_transport_fences_unawaited_review_and_settles_before_turn_end() {
    let f = Fixture::new(true).await;
    *f.adapter.reply.lock() = "pending".into();
    let done = Arc::new(tokio::sync::Notify::new());
    let code: Arc<dyn dsh_code_runtime::CodeRuntime> = Arc::new(EarlyReturnCode {
        adapter: f.adapter.clone(),
        done: done.clone(),
    });
    f.ctx.set("codeRuntime", arc(code)).unwrap();
    f.send();
    f.idle().await;
    let cut = f.agent.agent.session().seq().get();
    tokio::time::timeout(Duration::from_secs(3), done.notified())
        .await
        .unwrap();
    assert_eq!(
        f.agent.agent.session().seq().get(),
        cut,
        "late nested result must not append after the parent turn ended"
    );
    assert_eq!(f.runs.load(Ordering::SeqCst), 0);
    let events = f.agent.agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.type_ == "tool/ptc-dispatch")
            .count(),
        1
    );
    let end = events
        .iter()
        .find(|e| e.type_ == "tool/ptc-dispatch")
        .unwrap();
    assert_eq!(end.data["error"]["code"], "ABORTED");
    validate_native(f.agent.agent.session());
    f.close().await;
}

fn validate_native(session: &dsh_session::Session) {
    use dsh_session::format_v4::*;
    let mut header = serde_json::to_value(session.header()).unwrap();
    header["version"] = json!(4);
    let mut decoder = V4Decoder::new(
        encode_v4_header(header.clone(), 0).unwrap(),
        V4Recovery::Strict,
    )
    .unwrap();
    let mut validator = V4Validator::new(header, 0).unwrap();
    for event in session.events().iter() {
        let wire =
            encode_v4_event(serde_json::to_value(event).unwrap(), &Default::default()).unwrap();
        let row = decoder.decode_row(wire).unwrap().unwrap();
        validator.push(&row).unwrap();
    }
    decoder.finish().unwrap();
    validator.finish().unwrap();
}
