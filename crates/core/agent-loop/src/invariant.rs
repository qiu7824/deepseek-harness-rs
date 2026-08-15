//! Package-owned request-reconstruction invariant for loop-built LLM
//! calls. Rust port of `packages/core/agent-loop/src/invariant.ts`.
//!
//! # Deviations
//!
//! - `Object.isFrozen` checks collapse to the type system (owned Rust
//!   values are frozen by construction).
//! - The `llm/stream` waterfall carries the request in a shared cell
//!   (see dsh-llm §19), so the listener reads `cell.lock()`.

use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, NextFn, Plugin, PluginError, downcast_arc};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_llm::{GenerateOptions, is_agent_loop_request};
use dsh_session::{SessionStore, fold_request_header, session_id};
use parking_lot::Mutex;

/// Full package name owning these invariants.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-agent-loop";

/// Cordis companion plugin name.
pub const NAME: &str = "agent-loop-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the installer registered under [`PACKAGE_NAME`].
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // Prepend prevents a short-circuiting replay listener from
                // silencing the check.
                let listener_fail = fail.clone();
                let listener: Arc<cordis::Listener> = Arc::new(move |listener_ctx, args| {
                    let Some(next) = args.last().and_then(|value| downcast_arc::<NextFn>(value)) else {
                        return Box::pin(async { None });
                    };
                    let Some(cell) = args.first().and_then(|value| {
                        downcast_arc::<Arc<Mutex<GenerateOptions>>>(value)
                    }) else {
                        return Box::pin(async { None });
                    };
                    let cell = cell.as_ref().clone();
                    let fail = listener_fail.clone();
                    let sessions = listener_ctx.get_typed::<Arc<SessionStore>>("sessions", false);
                    Box::pin(async move {
                        {
                            let options = cell.lock().clone();
                            if is_agent_loop_request(&options) {
                                if options.session_id.is_none() {
                                    (fail)("a loop-built request must carry a session id");
                                }
                                let session_id = session_id(options.session_id.expect("checked"));
                                let Some(sessions) = &sessions else {
                                    (fail)("a loop-built request must carry a live session id, got \"<no sessions service>\"");
                                    unreachable!()
                                };
                                let Some(session) = sessions.get(&session_id) else {
                                    (fail)(&format!(
                                        "a loop-built request must carry a live session id, got \"{}\"",
                                        session_id.as_str()
                                    ));
                                    unreachable!()
                                };
                                let events = session.events();
                                if !events.iter().any(|event| event.type_ == "step/start") {
                                    (fail)("a loop-built request with no step/start in its session log");
                                }
                                let Some(header) = fold_request_header(&events, None) else {
                                    (fail)("a loop-built request with no request/header event in its session log");
                                    unreachable!()
                                };
                                let expected = session.derive_messages().expect("deriveMessages");
                                if serde_json::to_value(&options.messages).expect("messages")
                                    != serde_json::to_value(&expected).expect("expected")
                                {
                                    (fail)(&format!(
                                        "llm request for session \"{}\" diverges from the dispatch-time durable derivation (log-reconstruction desync)",
                                        session.id().as_str()
                                    ));
                                }
                                let header_matches = options.model == header.config.model
                                    && options.system == header.system
                                    && options.temperature == header.config.temperature
                                    && options.max_tokens == header.config.max_tokens
                                    && serde_json::to_value(&options.stop).expect("stop")
                                        == serde_json::to_value(&header.config.stop).expect("stop")
                                    && serde_json::to_value(&options.tools.clone().unwrap_or_default())
                                        .expect("tools")
                                        == serde_json::to_value(&header.tools.clone().unwrap_or_default())
                                            .expect("header tools");
                                if !header_matches {
                                    (fail)(&format!(
                                        "llm request for session \"{}\" diverges from the folded request header",
                                        session.id().as_str()
                                    ));
                                }
                            }
                        }
                        Some(next.call().await)
                    })
                });
                ctx.on(
                    "llm/stream",
                    listener,
                    EventOptions::default().global(true).prepend(true),
                )
                .await;
            })
        }),
    }
}

/// Register the agent-loop invariant companion against the `invariants`
/// service.
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the agent-loop invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion (`name = "agent-loop-invariant"`,
/// `inject = ["invariants"]`).
pub struct LlmLoopInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for LlmLoopInvariantPlugin {
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
