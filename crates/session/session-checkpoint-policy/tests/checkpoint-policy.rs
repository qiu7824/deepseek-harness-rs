//! Rust port of the core `session-checkpoint-policy.spec.ts` behaviors: the
//! canonical pre-dispatch abort result, the top-level tool guard, and the
//! flush-before-first-chunk stream wrapper (fail-open delegation and
//! fail-closed terminal chunk).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

use futures::StreamExt;

use dsh_llm::{ContentBlock, FinishReason, GenerateOptions, StreamChunk, StreamFactory};
use dsh_session_checkpoint_policy::{
    aborted_before_dispatch_result, after_checkpoint, needs_tool_checkpoint,
};
use dsh_tools::{TOOL_ABORTED_BEFORE_DISPATCH, ToolExecution};

#[test]
fn aborted_result_carries_the_canonical_shape() {
    let result = aborted_before_dispatch_result();
    assert!(result.is_error);
    let error = result.error.expect("error present");
    assert_eq!(error.message, "tool call aborted before dispatch");
    let info = error.info.expect("info present");
    assert_eq!(info.name, "AbortError");
    assert_eq!(info.code, TOOL_ABORTED_BEFORE_DISPATCH);
    assert!(matches!(
        result.content.as_slice(),
        [ContentBlock::Text { text }] if text == "Error: tool call aborted before dispatch"
    ));
    assert!(result.value.is_none());
}

fn execution_with(agent: bool, parent: bool) -> ToolExecution {
    ToolExecution {
        token: 1,
        call_id: dsh_llm::call_id("call-1"),
        root_call_id: dsh_llm::call_id("call-1"),
        name: "probe".to_string(),
        arguments: serde_json::Value::Null,
        agent: if agent {
            Some(Arc::new(ProbeAgent))
        } else {
            None
        },
        parent: if parent { Some(1) } else { None },
        signal: parking_lot::Mutex::new(Arc::new(|| false)),
    }
}

struct ProbeAgent;

impl dsh_agent::Agent for ProbeAgent {
    fn id(&self) -> &dsh_session::SessionId {
        static ID: std::sync::OnceLock<dsh_session::SessionId> = std::sync::OnceLock::new();
        ID.get_or_init(|| dsh_session::session_id("probe"))
    }

    fn options(&self) -> &dsh_agent::AgentOptions {
        static OPTIONS: std::sync::OnceLock<dsh_agent::AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(dsh_agent::AgentOptions::default)
    }

    fn session(&self) -> &dsh_session::Session {
        static SESSION: std::sync::OnceLock<dsh_session::Session> = std::sync::OnceLock::new();
        SESSION.get_or_init(|| {
            dsh_session::Session::create(dsh_session::session_id("probe"), None, None)
                .expect("session")
        })
    }

    fn inbox(&self) -> &dsh_agent::Inbox {
        static INBOX: std::sync::OnceLock<dsh_agent::Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            dsh_agent::Inbox::new(
                &dsh_session::Session::create(dsh_session::session_id("probe"), None, None)
                    .expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Idle
    }

    fn ctx(&self) -> &cordis::Context {
        static CTX: std::sync::OnceLock<cordis::Context> = std::sync::OnceLock::new();
        CTX.get_or_init(cordis::Context::root)
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        static KEY: std::sync::OnceLock<dsh_scope::ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(dsh_scope::ScopeKey::new)
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

    fn send(
        &self,
        _message: dsh_session::UserMessage,
        _target: dsh_agent::InboxTarget,
        _wakeup: bool,
    ) {
    }

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

#[test]
fn tool_checkpoint_applies_to_top_level_owned_calls_only() {
    assert!(needs_tool_checkpoint(&execution_with(true, false)));
    assert!(
        !needs_tool_checkpoint(&execution_with(true, true)),
        "nested"
    );
    assert!(
        !needs_tool_checkpoint(&execution_with(false, false)),
        "unowned"
    );
}

fn options() -> GenerateOptions {
    GenerateOptions {
        provider: "stub".to_string(),
        model: "stub".to_string(),
        reasoning_effort: None,
        messages: Vec::new(),
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    }
}

fn tail_factory(text: &'static str) -> StreamFactory {
    Arc::new(move |_options| {
        Box::pin(futures::stream::iter(vec![
            StreamChunk::TextDelta {
                index: 0,
                text: text.to_string(),
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    })
}

#[tokio::test(flavor = "current_thread")]
async fn stream_wrapper_flushes_before_the_first_chunk() {
    let order: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let flush_order = order.clone();
    let flushed = Arc::new(AtomicBool::new(false));
    let flushed_for_check = flushed.clone();
    let wrapped: StreamFactory = after_checkpoint(
        move || {
            let order = flush_order.clone();
            let flushed = flushed_for_check.clone();
            Box::pin(async move {
                order.fetch_add(1, SeqCst);
                flushed.store(true, SeqCst);
                Ok(())
            })
        },
        tail_factory("tail"),
    );
    let mut stream = wrapped(options());
    let first = stream.next().await.expect("first chunk");
    assert!(flushed.load(SeqCst), "flush must precede the first chunk");
    assert!(matches!(first, StreamChunk::TextDelta { text, .. } if text == "tail"));
    // The downstream stream still reaches its finish.
    let mut finish_seen = false;
    while let Some(chunk) = stream.next().await {
        if matches!(chunk, StreamChunk::Finish { .. }) {
            finish_seen = true;
        }
    }
    assert!(finish_seen);
}

#[tokio::test(flavor = "current_thread")]
async fn stream_wrapper_fails_closed_on_a_checkpoint_rejection() {
    let downstream_called = Arc::new(AtomicBool::new(false));
    let downstream_called_for_factory = downstream_called.clone();
    let downstream: StreamFactory = {
        let downstream_called = downstream_called.clone();
        let inner = tail_factory("should-not-run");
        Arc::new(move |options| {
            downstream_called.store(true, SeqCst);
            inner(options)
        })
    };
    let _ = downstream_called_for_factory;
    let wrapped: StreamFactory = after_checkpoint(
        move || Box::pin(async move { Err("flush blew up".to_string()) }),
        downstream,
    );
    let mut stream = wrapped(options());
    let first = stream.next().await.expect("terminal chunk");
    match first {
        StreamChunk::Finish {
            reason: FinishReason::Error { failure },
            ..
        } => {
            assert!(
                failure.message.contains("flush blew up"),
                "{}",
                failure.message
            );
        }
        other => panic!("expected an error finish, got {other:?}"),
    }
    assert!(stream.next().await.is_none(), "no downstream chunks");
    assert!(
        !downstream_called.load(SeqCst),
        "adapter dispatch must be prevented"
    );
}
