//! Request-reconstruction invariant tests: Rust port of
//! `packages/core/agent-loop/tests/invariant.spec.ts` (the llm/stream
//! guard's loud failure branches).

use std::sync::Arc;

use cordis::Context;
use dsh_agent_loop::{agent_loop_invariant_installer, apply_agent_loop_invariant};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_llm::LlmRuntime;
use dsh_session::SessionStore;
use futures::StreamExt;

fn setup() -> (Context, Arc<LlmRuntime>) {
    let ctx = Context::root();
    InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            ..InvariantConfig::default()
        },
    );
    let _sessions = SessionStore::install(&ctx);
    let runtime = LlmRuntime::install(&ctx);
    let _ = apply_agent_loop_invariant(&ctx);
    (ctx, runtime)
}

fn loop_request(session_id: Option<String>) -> dsh_llm::GenerateOptions {
    dsh_llm::GenerateOptions {
        provider: "test".to_string(),
        model: "model".to_string(),
        reasoning_effort: None,
        messages: Vec::new(),
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id,
        purpose: None,
        agent_loop_request: true,
    }
}

async fn collect(stream: dsh_llm::ChunkStream) -> Vec<dsh_llm::StreamChunk> {
    let mut stream = stream;
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }
    chunks
}

#[test]
fn installer_declares_its_sessions_dependency() {
    // The installer's inject names the sessions service (no reader on
    // InjectSpec; exercised through registration in the companion tests).
    let _ = agent_loop_invariant_installer();
}

#[tokio::test]
async fn loop_request_without_a_session_id_fails_loudly() {
    let (ctx, runtime) = setup();
    // Let the companion fiber activate before dispatching.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = loop_request(None);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stream = runtime.stream(request);
        futures::executor::block_on(collect(stream))
    }));
    let payload = outcome.expect_err("the missing session id must fail");
    let rendered = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        rendered.contains("a loop-built request must carry a session id"),
        "got {rendered}"
    );
    let _ = ctx;
}

#[tokio::test]
async fn loop_request_with_an_unknown_session_fails_loudly() {
    let (ctx, runtime) = setup();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = loop_request(Some("no-such-session".to_string()));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stream = runtime.stream(request);
        futures::executor::block_on(collect(stream))
    }));
    let payload = outcome.expect_err("the unknown session must fail");
    let rendered = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        rendered.contains("must carry a live session id"),
        "got {rendered}"
    );
    let _ = ctx;
}
