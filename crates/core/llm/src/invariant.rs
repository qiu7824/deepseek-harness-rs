//! Package-owned LLM stream-protocol invariants. Rust port of
//! `packages/llm/llm/src/invariant.ts`.
//!
//! # Deviation
//!
//! - `validateIndex` collapses to the type system: chunk indices are `u64`
//!   in the Rust stream vocabulary, so non-negative safe integers are the
//!   only representable values.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, NextFn, Plugin, PluginError, arc, downcast_arc};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use futures::StreamExt;

use crate::runtime::{ChunkStream, LlmRuntime, StreamFactory};
use crate::types::{FinishReason, StreamChunk};

/// Full package name owning these invariants.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-llm";

/// Cordis companion plugin name.
pub const NAME: &str = "llm-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Require a delta to address an open block of its matching type.
fn validate_delta(
    open: &HashMap<u64, String>,
    index: u64,
    expected: &str,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    let actual = open.get(&index).map(String::as_str);
    if actual != Some(expected) {
        (fail)(&format!(
            "{expected} delta at index {index} requires an open {expected} block, got {}",
            actual.unwrap_or("undefined")
        ));
    }
}

struct ValidateState {
    source: ChunkStream,
    open: HashMap<u64, String>,
    usage_seen: bool,
    finished: bool,
    fail: Arc<dyn Fn(&str) + Send + Sync>,
}

/// Wrap one provider stream and enforce its grammar as chunks are consumed.
pub fn validate_stream(
    source: ChunkStream,
    fail: Arc<dyn Fn(&str) + Send + Sync>,
) -> ChunkStream {
    Box::pin(futures::stream::unfold(
        ValidateState {
            source,
            open: HashMap::new(),
            usage_seen: false,
            finished: false,
            fail,
        },
        |mut state| async move {
            let chunk = match state.source.next().await {
                Some(chunk) => chunk,
                None => {
                    if !state.finished {
                        (state.fail)("LLM stream ended without a terminal finish chunk");
                    }
                    return None;
                }
            };
            if state.finished {
                (state.fail)(&format!(
                    "LLM stream emitted {} after terminal finish",
                    chunk.type_tag()
                ));
            }
            match &chunk {
                StreamChunk::BlockStart { index, block_type } => {
                    if state.open.contains_key(index) {
                        (state.fail)(&format!("LLM stream repeated block-start index {index}"));
                    }
                    state.open.insert(*index, block_type.clone());
                }
                StreamChunk::TextDelta { index, .. } => {
                    validate_delta(&state.open, *index, "text", &state.fail)
                }
                StreamChunk::ReasoningDelta { index, .. } => {
                    validate_delta(&state.open, *index, "reasoning", &state.fail)
                }
                StreamChunk::ToolCallDelta { index, .. } => {
                    validate_delta(&state.open, *index, "tool-call", &state.fail)
                }
                StreamChunk::BlockEnd { index, block } => {
                    let Some(block_type) = state.open.get(index).cloned() else {
                        (state.fail)(&format!(
                            "LLM stream block-end index {index} has no open block"
                        ));
                        unreachable!()
                    };
                    if block.type_tag() != block_type {
                        (state.fail)(&format!(
                            "LLM stream block-end index {index} closes {}, expected {block_type}",
                            block.type_tag()
                        ));
                    }
                    state.open.remove(index);
                }
                StreamChunk::Usage { .. } => {
                    if state.usage_seen {
                        (state.fail)("LLM stream emitted usage more than once");
                    }
                    state.usage_seen = true;
                }
                StreamChunk::Finish { reason, .. } => {
                    if !state.open.is_empty()
                        && !matches!(reason, FinishReason::Error { .. } | FinishReason::Aborted { .. })
                    {
                        (state.fail)(&format!(
                            "LLM stream finished with {} open block(s)",
                            state.open.len()
                        ));
                    }
                    state.finished = true;
                }
            }
            Some((chunk, state))
        },
    ))
}

/// Build the installer registered under [`PACKAGE_NAME`].
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // Wrap every provider stream in grammar validation, before
                // any other `llm/stream` listener can observe it.
                let stream_fail = fail.clone();
                let stream_listener: Arc<cordis::Listener> =
                    Arc::new(move |_listener_ctx: &Context, args: Vec<ArcValue>| {
                        let Some(next) = args.get(1).and_then(|value| downcast_arc::<NextFn>(value)) else {
                            return Box::pin(async { None });
                        };
                        let fail = stream_fail.clone();
                        Box::pin(async move {
                            let value = next.call().await;
                            let Some(inner) = downcast_arc::<StreamFactory>(&value) else {
                                return None;
                            };
                            let inner = inner.as_ref().clone();
                            let wrapped: StreamFactory = Arc::new(move |options| {
                                validate_stream(inner(options), fail.clone())
                            });
                            Some(arc(wrapped))
                        })
                    });
                ctx.on(
                    "llm/stream",
                    stream_listener,
                    EventOptions::default().global(true).prepend(true),
                )
                .await;

                // The notification promises a readable registry; only that
                // broken promise can make the retry-policy lookup fail.
                let updated_fail = fail.clone();
                let updated_listener: Arc<cordis::Listener> =
                    Arc::new(move |listener_ctx: &Context, _args: Vec<ArcValue>| {
                        let fail = updated_fail.clone();
                        let llm = listener_ctx.get_typed::<Arc<LlmRuntime>>("llm", false);
                        Box::pin(async move {
                            let Some(llm) = llm else {
                                return None;
                            };
                            for provider in llm.list_providers() {
                                if llm.provider_retry_policy(&provider.id).is_err() {
                                    (fail)(&format!(
                                        "llm/adapters-updated fired while provider \"{}\" has no readable registration",
                                        provider.id
                                    ));
                                }
                            }
                            None
                        })
                    });
                ctx.on(
                    "llm/adapters-updated",
                    updated_listener,
                    EventOptions::default().global(true),
                )
                .await;
            })
        }),
    }
}

/// Register the LLM invariant companion against the `invariants` service.
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the llm invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion (`name = "llm-invariant"`,
/// `inject = ["invariants"]`).
pub struct LlmInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for LlmInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT.iter().copied())
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
