//! Semantic durability checkpoints for model requests, top-level tool
//! dispatch, and completed agent steps. Rust port of
//! `packages/session/session-checkpoint-policy/src/index.ts`.
//!
//! # Deviations
//!
//! - A checkpoint failure collapses into a terminal `finish` chunk on the
//!   model stream (the TS thrown error's fail-closed equivalent), and into a
//!   panic at the tool boundary (the repo-wide throw channel).

use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, NextFn, arc, downcast_arc, make_disposer};
use dsh_agent::AgentPreStepPayload;
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmFailure, StreamChunk,
    StreamFactory,
};
use dsh_session::{SessionStore, session_id};
use dsh_tools::{
    TOOL_ABORTED_BEFORE_DISPATCH, ToolErrorInfo, ToolExecution, ToolExecutionResult, ToolFailure,
};
use futures::future::BoxFuture;
use futures::stream::{StreamExt, once};

/// Cordis plugin name used by Loader diagnostics (TS `name`).
pub const NAME: &str = "session-checkpoint-policy";

/// Services whose request, tool, session, and persistence boundaries this
/// policy joins (TS `inject`).
pub const INJECT: [&str; 4] = ["llm", "sessionPersistence", "sessions", "tools"];

/// Materialize the canonical result for a call cancelled before tool
/// dispatch (TS `abortedBeforeDispatchResult`).
pub fn aborted_before_dispatch_result() -> ToolExecutionResult {
    ToolExecutionResult {
        is_error: true,
        error: Some(ToolFailure {
            message: "tool call aborted before dispatch".to_string(),
            info: Some(ToolErrorInfo {
                name: "AbortError".to_string(),
                code: TOOL_ABORTED_BEFORE_DISPATCH.to_string(),
            }),
        }),
        value: None,
        content: vec![ContentBlock::Text {
            text: "Error: tool call aborted before dispatch".to_string(),
        }],
        meta: None,
        additional_contexts: Vec::new(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

/// Delay construction of the downstream model stream until the complete
/// logged request prefix is durable. A checkpoint rejection yields a
/// terminal error chunk instead of adapter dispatch (TS `afterCheckpoint`).
pub fn after_checkpoint(
    flush: impl Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync + 'static,
    next: StreamFactory,
) -> StreamFactory {
    Arc::new(move |options: GenerateOptions| {
        let flush = flush();
        let next = next.clone();
        let prefix = once(async move { flush.await });
        let stream = prefix.flat_map(move |result| {
            let next = next.clone();
            let options = options.clone();
            match result {
                Ok(()) => next(options),
                Err(error) => Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
                    reason: FinishReason::Error {
                        failure: LlmFailure {
                            message: format!("durability checkpoint failed: {error}"),
                            code: "CHECKPOINT_FAILED".to_string(),
                            status: None,
                            provider_retry_after_ms: None,
                            request_id: None,
                        },
                    },
                    replay_state: None,
                }])) as ChunkStream,
            }
        });
        Box::pin(stream)
    })
}

/// Whether a tool execution needs its own checkpoint: top-level
/// agent-owned calls only (TS the `tools/execute` guard).
pub fn needs_tool_checkpoint(execution: &ToolExecution) -> bool {
    execution.agent.is_some() && execution.parent.is_none()
}

/// Install semantic checkpoint listeners (TS `apply`). Model calls
/// checkpoint the logged request before adapter dispatch; top-level tool
/// calls checkpoint their recorded call before the tool body; the next
/// request boundary checkpoints the preceding response/result batch.
pub fn apply(ctx: &Context) -> Disposer {
    // `llm/stream`: wrap the downstream StreamFactory so the first chunk
    // waits for the request-prefix flush. The waterfall hands every listener
    // a NextFn as the final argument.
    let llm_listener: Arc<cordis::Listener> = Arc::new({
        let ctx = ctx.clone();
        move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let cell = args
                    .first()
                    .and_then(|value| {
                        downcast_arc::<Arc<parking_lot::Mutex<GenerateOptions>>>(value)
                    })
                    .map(|arc| arc.as_ref().clone());
                let next = args.get(1).and_then(|value| downcast_arc::<NextFn>(value));
                let Some(cell) = cell else {
                    return None;
                };
                let Some(next) = next else {
                    return None;
                };
                let session_identity = { cell.lock().session_id.clone() };
                let Some(session_identity) = session_identity else {
                    return Some(next.call().await);
                };
                let store = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone());
                let session = store
                    .as_ref()
                    .and_then(|store| store.get(&session_id(session_identity.as_str())));
                let Some(session) = session else {
                    return Some(next.call().await);
                };
                let fallback_value = next.call().await;
                let Some(factory) =
                    downcast_arc::<StreamFactory>(&fallback_value).map(|arc| arc.as_ref().clone())
                else {
                    return None;
                };
                let session_for_flush = session.clone();
                let store_for_flush = store.expect("store resolved");
                let wrapped: StreamFactory = after_checkpoint(
                    move || {
                        let store = store_for_flush.clone();
                        let session = session_for_flush.clone();
                        Box::pin(async move { store.flush(&session).await.map(|_| ()) })
                    },
                    factory,
                );
                Some(arc(wrapped))
            })
        }
    });

    // `tools/execute`: checkpoint top-level owned calls, then fail closed on
    // a pre-dispatch abort.
    let tool_listener: Arc<cordis::Listener> = Arc::new({
        let ctx = ctx.clone();
        move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let execution = args
                    .first()
                    .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
                    .map(|arc| arc.as_ref().clone());
                let next = args.get(1).and_then(|value| downcast_arc::<NextFn>(value));
                let Some(execution) = execution else {
                    return None;
                };
                let Some(next) = next else {
                    return None;
                };
                if !needs_tool_checkpoint(&execution) {
                    return Some(next.call().await);
                }
                let agent = execution.agent.clone().expect("top-level owned");
                let store = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone());
                if let Some(store) = store {
                    let _ = store.flush(agent.session()).await;
                }
                if execution.signal.lock()() {
                    return Some(arc(Arc::new(aborted_before_dispatch_result())));
                }
                Some(next.call().await)
            })
        }
    });

    // `agent/pre-step`: flush everything committed by the preceding step
    // before the next request begins.
    let pre_step_listener: Arc<cordis::Listener> = Arc::new({
        let ctx = ctx.clone();
        move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<AgentPreStepPayload>())
                    .map(|payload| payload.agent.clone());
                let next = args.get(1).and_then(|value| downcast_arc::<NextFn>(value));
                let Some(agent) = payload else {
                    return None;
                };
                let Some(next) = next else {
                    return None;
                };
                let store = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone());
                if let Some(store) = store {
                    let _ = store.flush(agent.session()).await;
                }
                Some(next.call().await)
            })
        }
    });

    let ctx_for_llm = ctx.clone();
    let ctx_for_tool = ctx.clone();
    let ctx_for_step = ctx.clone();
    make_disposer(move || {
        let ctx_for_llm = ctx_for_llm.clone();
        let ctx_for_tool = ctx_for_tool.clone();
        let ctx_for_step = ctx_for_step.clone();
        let llm_listener = llm_listener.clone();
        let tool_listener = tool_listener.clone();
        let pre_step_listener = pre_step_listener.clone();
        Box::pin(async move {
            let _ = ctx_for_llm
                .on("llm/stream", llm_listener, Default::default())
                .await;
            let _ = ctx_for_tool
                .on("tools/execute", tool_listener, Default::default())
                .await;
            let _ = ctx_for_step
                .on("agent/pre-step", pre_step_listener, Default::default())
                .await;
        })
    })
}

// Re-exported vocabulary anchors (the TS type-only imports).
pub use dsh_agent::PreStepDecision as PreStepDecisionType;
pub use dsh_session::Session as SessionType;
