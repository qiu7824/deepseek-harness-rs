//! Tests for the spill-policy PLUGIN (the non-code-mode subset): Rust port
//! of `packages/spill/spill-policy/tests/spill-policy.spec.ts`. The policy
//! registers no service, only the `tools/post-execute` transformer; we drive
//! real tools through `ctx.tools.execute(...)` and assert: disabled mode is
//! a true no-op, an oversized plain-text result is spilled and replaced with
//! a preview + locator within the cap, a small result and a non-text result
//! pass through, `read` is skipped, and a `saveText` failure / missing
//! backend / missing owner all preserve the original result without an
//! `isError`.
//!
//! Deviations:
//!
//! - The durable `tools/code-dispatch-log` arm and the code-mode tests wait
//!   for the dsh-code-runtime milestone.
//! - The TS load-time config validation cases (negative/fractional
//!   `maxInlineBytes`) are inexpressible: the Rust config field is `u64`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, Context, Disposer, Listener, NextFn, arc, downcast_arc};
use dsh_agent::{AgentOptions, AgentStatus, Inbox, InboxNotifications, InboxTarget};
use dsh_llm::{ContentBlock, Message, MessageSource, Role, call_id};
use dsh_scope::ScopeKey;
use dsh_session::{Session, session_id};
use dsh_spill::{SaveTextSpill, SpillOwner, SpillRef, SpillStore, spill_locator};
use dsh_spill_policy::{Config, apply};
use dsh_tools::{
    PostToolDecision, ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime,
};
use parking_lot::Mutex;
use serde_json::{Value as JsonValue, json};

// ---------------------------------------------------------------------------
// fakes

/// A stub spill backend recording its saves; `fail` exercises the
/// best-effort fallback.
struct StubStore {
    saves: Mutex<Vec<SaveTextSpill>>,
    fail: AtomicBool,
}

impl StubStore {
    fn install(ctx: &Context) -> Arc<Self> {
        let store = Arc::new(Self {
            saves: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        });
        let erased: Arc<dyn SpillStore> = store.clone();
        ctx.register_service(erased);
        store
    }
}

#[async_trait::async_trait]
impl SpillStore for StubStore {
    async fn save_text(&self, input: &SaveTextSpill) -> Result<SpillRef, String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("disk full".to_string());
        }
        self.saves.lock().push(input.clone());
        Ok(SpillRef {
            locator: spill_locator(format!("/spill/{}", input.suggested_name)),
            bytes: input.content.len() as u64,
            retrieval_hint: "Use the stub retrieval path.".to_string(),
        })
    }
}

/// A minimal live agent for policy tests (the session header id is the only
/// field the policy reads).
struct TestAgent {
    id: dsh_session::SessionId,
    options: AgentOptions,
    session: Session,
    inbox: Inbox,
    status: Mutex<AgentStatus>,
    ctx: Context,
    scope_key: ScopeKey,
}

impl dsh_agent::Agent for TestAgent {
    fn id(&self) -> &dsh_session::SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        *self.status.lock()
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(
        &self,
        _cause: dsh_session::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        task()
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, message: dsh_session::UserMessage) {
        self.send(message, InboxTarget::NextTurn, true);
    }

    fn steer(&self, message: dsh_session::UserMessage) {
        self.send(message, InboxTarget::NextStep, true);
    }

    fn inject(&self, message: dsh_session::UserMessage) {
        self.send(message, InboxTarget::NextStep, false);
    }
}

fn test_agent(ctx: &Context, id: &str) -> Arc<TestAgent> {
    let session = Session::create(session_id(id), None, None).expect("session");
    let inbox = Inbox::new(&session, InboxNotifications::default()).expect("inbox");
    Arc::new(TestAgent {
        id: session_id(id),
        options: AgentOptions::default(),
        session,
        inbox,
        status: Mutex::new(AgentStatus::Idle),
        ctx: ctx.clone(),
        scope_key: ScopeKey::new(),
    })
}

// ---------------------------------------------------------------------------
// tools

/// A tool returning `text` verbatim (name configurable so we can register
/// `read`).
fn text_tool(name: &str, text: &str) -> ToolDefinition {
    let text = text.to_string();
    let output = dsh_tools::schema::value_schema_spec_to_json_schema(
        &dsh_tools::schema::ValueSchemaSpec::String(dsh_tools::schema::StringValueSchemaSpec {
            annotations: dsh_tools::schema::ValueSchemaAnnotations::default(),
            enum_: None,
            const_: None,
        }),
    )
    .expect("output schema");
    ToolDefinition {
        name: name.to_string(),
        description: name.to_string(),
        parameters: json!({}),
        output: ToolOutputDefinition {
            schema: output,
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().expect("string value").to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |_args, _run_ctx| {
            let text = text.clone();
            Box::pin(async move { Ok(JsonValue::String(text)) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

/// A tool returning a mixed text + reasoning content (flatten declines).
fn mixed_tool() -> ToolDefinition {
    let output = dsh_tools::schema::value_schema_spec_to_json_schema(
        &dsh_tools::schema::ValueSchemaSpec::String(dsh_tools::schema::StringValueSchemaSpec {
            annotations: dsh_tools::schema::ValueSchemaAnnotations::default(),
            enum_: None,
            const_: None,
        }),
    )
    .expect("output schema");
    ToolDefinition {
        name: "mixed".to_string(),
        description: "mixed".to_string(),
        parameters: json!({}),
        output: ToolOutputDefinition {
            schema: output,
            render: Arc::new(|_args, _value| {
                Ok(vec![
                    ContentBlock::Text {
                        text: "x".repeat(100),
                    },
                    ContentBlock::Reasoning {
                        text: "why".to_string(),
                    },
                ])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|_args, _run_ctx| {
            Box::pin(async move { Ok(JsonValue::String("x".to_string())) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

fn input(name: &str, agent: Option<Arc<dyn dsh_agent::Agent>>) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id(format!("call-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments: json!({}),
        agent,
        parent: None,
        signal: Arc::new(|| false),
    }
}

/// Flatten a result's text blocks.
fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// setup

struct Setup {
    ctx: Context,
    spill: Option<Arc<StubStore>>,
    disposer: Disposer,
}

async fn setup(config: Config, with_spill: bool) -> Setup {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let _ = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let spill = if with_spill {
        Some(StubStore::install(&ctx))
    } else {
        None
    };
    let disposer = apply(&ctx, config).expect("policy");
    Setup {
        ctx,
        spill,
        disposer,
    }
}

/// Run one tool through the real pipeline with a session owner (the TS
/// `exec(name)` default); no-owner cases drive `runtime.execute` directly.
async fn execute(
    setup: &Setup,
    name: &str,
    tool: ToolDefinition,
) -> Arc<dsh_tools::ToolExecutionResult> {
    let agent = test_agent(&setup.ctx, "s1");
    let runtime: Arc<Arc<ToolRuntime>> =
        setup.ctx.get_typed("tools", false).expect("tools service");
    runtime.register(&setup.ctx, tool).expect("register");
    runtime.execute(input(name, Some(agent))).await
}

// ---------------------------------------------------------------------------
// disabled mode

#[tokio::test(flavor = "current_thread")]
async fn disabled_mode_registers_no_post_execute_listener() {
    let setup = setup(Config::default(), true).await;
    let result = execute(&setup, "big", text_tool("big", &"x".repeat(1000))).await;
    assert_eq!(text_of(&result.content), "x".repeat(1000));
    assert!(!result.is_error);
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

// ---------------------------------------------------------------------------
// oversized plain-text replacement

#[tokio::test(flavor = "current_thread")]
async fn spills_the_full_text_and_replaces_the_result_with_a_preview_and_locator_within_the_cap() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(200),
        },
        true,
    )
    .await;
    let body = format!("{}{}", "HEAD".repeat(200), "TAIL".repeat(200)); // 1600 bytes > 200
    let result = execute(&setup, "big", text_tool("big", &body)).await;

    assert!(!result.is_error);
    let saves = setup.spill.expect("stub").saves.lock().clone();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].content, body);
    assert_eq!(saves[0].source.tool_name, "big");
    assert_eq!(saves[0].suggested_name, "big.txt");
    assert_eq!(saves[0].owner.session_id.to_string(), "s1");

    let text = text_of(&result.content);
    assert_ne!(text, body);
    assert!(text.starts_with("HEAD"), "{text}");
    assert!(text.contains("Full formatted result stored at: /spill/big.txt"));
    assert!(text.contains("Use the stub retrieval path."));
    assert!(text.contains("Omitted"));
    assert!(
        text.len() <= 200,
        "replacement {} bytes over cap",
        text.len()
    );
    assert!(text.len() < body.len());
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_inline_result_when_the_notice_only_replacement_would_exceed_the_cap() {
    // A body just over a tiny cap: the notice alone is larger than the cap,
    // so there is no within-cap replacement — the policy keeps the inline
    // result.
    let setup = setup(
        Config {
            max_inline_bytes: Some(4),
        },
        true,
    )
    .await;
    let body = "xxxxx"; // 5 bytes > 4, but far shorter than the notice
    let result = execute(&setup, "big", text_tool("big", body)).await;
    assert_eq!(text_of(&result.content), body);
}

#[tokio::test(flavor = "current_thread")]
async fn leaves_a_small_plain_text_result_unchanged() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(1000),
        },
        true,
    )
    .await;
    let result = execute(&setup, "small", text_tool("small", "tiny")).await;
    assert_eq!(text_of(&result.content), "tiny");
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn leaves_a_result_with_a_non_text_block_unchanged() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(5),
        },
        true,
    )
    .await;
    let result = execute(&setup, "mixed", mixed_tool()).await;
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
    assert_eq!(result.content.len(), 2);
}

// ---------------------------------------------------------------------------
// read skip

#[tokio::test(flavor = "current_thread")]
async fn never_spills_the_read_tool_result() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        true,
    )
    .await;
    let result = execute(&setup, "read", text_tool("read", &"x".repeat(1000))).await;
    assert_eq!(text_of(&result.content), "x".repeat(1000));
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

// ---------------------------------------------------------------------------
// nested-call skip

#[tokio::test(flavor = "current_thread")]
async fn leaves_nested_composite_results_complete() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        true,
    )
    .await;
    let body = "x".repeat(1000);
    let runtime: Arc<Arc<ToolRuntime>> = setup.ctx.get_typed("tools", false).expect("tools");
    runtime
        .register(&setup.ctx, text_tool("nested", &body))
        .expect("register");
    let mut nested = input("nested", None);
    nested.parent = Some(1);
    let result = runtime.execute(nested).await;
    assert_eq!(text_of(&result.content), body);
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

// ---------------------------------------------------------------------------
// best-effort fallback

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_original_result_when_save_text_fails() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        true,
    )
    .await;
    setup
        .spill
        .as_ref()
        .expect("stub")
        .fail
        .store(true, Ordering::SeqCst);
    let result = execute(&setup, "big", text_tool("big", &"x".repeat(1000))).await;
    assert_eq!(text_of(&result.content), "x".repeat(1000));
    assert!(!result.is_error);
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_original_result_when_no_spill_backend_is_loaded() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        false,
    )
    .await;
    let result = execute(&setup, "big", text_tool("big", &"x".repeat(1000))).await;
    assert_eq!(text_of(&result.content), "x".repeat(1000));
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_original_result_when_the_call_has_no_session_owner() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        true,
    )
    .await;
    let runtime: Arc<Arc<ToolRuntime>> = setup.ctx.get_typed("tools", false).expect("tools");
    runtime
        .register(&setup.ctx, text_tool("big", &"x".repeat(1000)))
        .expect("register");
    let result = runtime.execute(input("big", None)).await;
    assert_eq!(text_of(&result.content), "x".repeat(1000));
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn spills_for_a_call_with_a_session_owner() {
    // cap 200: the notice fits the cap, so an owned oversized result spills.
    let setup = setup(
        Config {
            max_inline_bytes: Some(200),
        },
        true,
    )
    .await;
    let result = execute(&setup, "big", text_tool("big", &"x".repeat(1000))).await;
    let text = text_of(&result.content);
    assert!(
        text.contains("Full formatted result stored at: /spill/big.txt"),
        "{text}"
    );
    let saves = setup.spill.expect("stub").saves.lock().clone();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].owner.session_id.to_string(), "s1");
}

// ---------------------------------------------------------------------------
// composition

#[tokio::test(flavor = "current_thread")]
async fn bounds_content_a_downstream_post_execute_listener_replaced() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(200),
        },
        true,
    )
    .await;
    // A later-registered listener replaces the (small) tool result with a
    // big one; the policy delegated via next(), so it bounds the
    // replacement.
    let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let next = downcast_arc::<NextFn>(&args[2]).expect("next").clone();
        Box::pin(async move {
            let _downstream = next.call().await;
            Some(arc(PostToolDecision::Accept {
                content: Some(vec![ContentBlock::Text {
                    text: "z".repeat(500),
                }]),
                value: None,
                additional_contexts: None,
            }))
        })
    });
    let _ = futures::executor::block_on(setup.ctx.on(
        "tools/post-execute",
        listener,
        cordis::EventOptions::default(),
    ));
    let result = execute(&setup, "small", text_tool("small", "tiny")).await;
    let saves = setup.spill.expect("stub").saves.lock().clone();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].content, "z".repeat(500));
    assert!(text_of(&result.content).contains("Full formatted result stored at"));
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_downstream_accept_decision_contexts_when_spilling() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(200),
        },
        true,
    )
    .await;
    let context = Message {
        id: dsh_llm::message_id("note"),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "note".to_string(),
        }],
        source: MessageSource::Plugin {
            plugin: "test".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    };
    let context_for_listener = context.clone();
    let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let next = downcast_arc::<NextFn>(&args[2]).expect("next").clone();
        let context = context_for_listener.clone();
        Box::pin(async move {
            let _downstream = next.call().await;
            Some(arc(PostToolDecision::Accept {
                content: None,
                value: None,
                additional_contexts: Some(vec![context]),
            }))
        })
    });
    let _ = futures::executor::block_on(setup.ctx.on(
        "tools/post-execute",
        listener,
        cordis::EventOptions::default(),
    ));
    let result = execute(&setup, "big", text_tool("big", &"x".repeat(1000))).await;
    assert!(text_of(&result.content).contains("Full formatted result stored at"));
    assert_eq!(result.additional_contexts.len(), 1);
    assert_eq!(result.additional_contexts[0].id, context.id);
}

#[tokio::test(flavor = "current_thread")]
async fn passes_a_downstream_value_replacement_through_for_registry_rendering() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(10),
        },
        true,
    )
    .await;
    // The Rust decision carries the lossless value (the text tool's output
    // schema is a string), not rendered blocks; the registry revalidates and
    // renders it.
    let replacement = JsonValue::String("z".repeat(500));
    let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let next = downcast_arc::<NextFn>(&args[2]).expect("next").clone();
        let replacement = replacement.clone();
        Box::pin(async move {
            let _downstream = next.call().await;
            Some(arc(PostToolDecision::Accept {
                content: None,
                value: Some(replacement),
                additional_contexts: None,
            }))
        })
    });
    let _ = futures::executor::block_on(setup.ctx.on(
        "tools/post-execute",
        listener,
        cordis::EventOptions::default(),
    ));
    let result = execute(&setup, "small", text_tool("small", "tiny")).await;
    assert!(!result.is_error);
    assert_eq!(result.value, Some(JsonValue::String("z".repeat(500))));
    assert_eq!(text_of(&result.content), "z".repeat(500));
    assert_eq!(setup.spill.expect("stub").saves.lock().len(), 0);
}

// ---------------------------------------------------------------------------
// cap invariant

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_inline_result_when_the_notice_alone_exceeds_the_cap_even_for_a_large_original() {
    // A large body (so it is well over the cap) but a cap smaller than the
    // notice itself: there is no within-cap replacement, so the policy must
    // keep the inline result rather than emit content over maxInlineBytes.
    let setup = setup(
        Config {
            max_inline_bytes: Some(8),
        },
        true,
    )
    .await;
    let body = "x".repeat(5000);
    let result = execute(&setup, "big", text_tool("big", &body)).await;
    assert_eq!(text_of(&result.content), body);
}

// ---------------------------------------------------------------------------
// disposal (HMR safety)

#[tokio::test(flavor = "current_thread")]
async fn stops_transforming_oversized_results_after_the_plugin_disposer_runs() {
    let setup = setup(
        Config {
            max_inline_bytes: Some(200),
        },
        true,
    )
    .await;
    let body = format!("{}{}", "HEAD".repeat(200), "TAIL".repeat(200));

    // Live: the listener spills and replaces.
    let before = execute(&setup, "big", text_tool("big", &body)).await;
    assert!(text_of(&before.content).contains("Full formatted result stored at"));
    assert_eq!(setup.spill.as_ref().expect("stub").saves.lock().len(), 1);

    // After disposal the listener is gone — the result passes through
    // untouched and nothing more is spilled (no leaked registration across
    // reload).
    (setup.disposer)().await;
    let runtime: Arc<Arc<ToolRuntime>> = setup.ctx.get_typed("tools", false).expect("tools");
    let after = runtime.execute(input("big", None)).await;
    assert_eq!(text_of(&after.content), body);
    assert_eq!(setup.spill.as_ref().expect("stub").saves.lock().len(), 1);
}

// The SpillOwner import keeps the request-shape documentation honest.
#[allow(dead_code)]
fn _spill_owner_shape() -> SpillOwner {
    SpillOwner {
        session_id: session_id("s"),
    }
}
