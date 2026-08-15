//! Cooperative tool-call timeout enforcer. A tool declares `timeoutMs` and
//! promises to honor `exec.signal`; this wrapper arms that deadline and maps
//! its own expiry to `TOOL_TIMEOUT` without racing or abandoning the tool
//! promise.
//! Rust port of `packages/guard/timeout-policy/src/index.ts`.
//!
//! # Deviations
//!
//! - The Rust execution signal seam is an abort PREDICATE
//!   (`dsh_tools::AbortPredicate`), not the TS `AbortSignal` object: the
//!   wrapper bridges the upstream predicate onto a `DeadlineSignal` through
//!   a 15 ms poller (the workspace convention for predicate bridging) and
//!   swaps a predicate that reads the fused signal.

pub mod invariant;

use std::sync::Arc;
use std::time::Duration;

use cordis::{ArcValue, Context, Disposer, Listener, Plugin, PluginError, arc, downcast_arc};
use dsh_llm::ContentBlock;
use dsh_timeout::{DeadlineSignal, deadline, timeout_of};
use dsh_tools::{ToolErrorInfo, ToolExecution, ToolExecutionResult, ToolFailure, ToolRuntime};

/// The code owned by this plugin, used BOTH as the internal deadline
/// classification code AND as the structured error `code` on the replacement
/// tool result.
pub const TOOL_TIMEOUT: &str = "TOOL_TIMEOUT";

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "timeout-policy";

/// The tool registry service this plugin wraps (`tools/execute`) and reads
/// (`get`).
pub const INJECT: [&str; 1] = ["tools"];

/// The structured result substituted when this plugin's deadline wins.
pub fn tool_timeout_result(timeout_ms: u64) -> ToolExecutionResult {
    let message = format!("tool call timed out after {timeout_ms}ms");
    ToolExecutionResult {
        is_error: true,
        error: Some(ToolFailure {
            message: message.clone(),
            info: Some(ToolErrorInfo {
                name: "ToolTimeoutError".to_string(),
                code: TOOL_TIMEOUT.to_string(),
            }),
        }),
        value: None,
        content: vec![ContentBlock::Text {
            text: format!("Error: {message}"),
        }],
        meta: None,
        additional_contexts: Vec::new(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

/// Register the timeout wrapper (TS `apply`). The returned disposer installs
/// the listener on its first run.
pub fn apply(ctx: &Context) -> Disposer {
    let ctx_for_listener = ctx.clone();
    let disposer_ctx = ctx.clone();
    let listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let ctx = ctx_for_listener.clone();
        Box::pin(async move {
            let exec = args
                .first()
                .and_then(|value| value.downcast_ref::<Arc<ToolExecution>>())
                .cloned()
                .expect("tools/execute exec");
            let next = downcast_arc::<cordis::NextFn>(args.last().expect("tools/execute next"))
                .expect("tools/execute next");
            let tools = ctx
                .get_typed::<Arc<ToolRuntime>>("tools", false)
                .map(|slot| slot.as_ref().clone())
                .expect("tools service (the TS inject requirement)");
            // The TS `ctx.tools.get(exec.name, exec.agent)` resolves the
            // global definition even for direct (agent-less) executes, so
            // the scope lookup must run with `None`, not short-circuit.
            let timeout_ms = {
                let scope = exec.agent.as_ref().map(|agent| agent.scope_key());
                tools
                    .get(&exec.name, scope)
                    .and_then(|definition| definition.timeout_ms)
            };
            // A tool that declares no budget: no deadline, delegate unchanged.
            let Some(timeout_ms) = timeout_ms else {
                return Some(next.call().await);
            };

            // Fuse the caller's abort predicate with this plugin's own
            // timer (the TS `deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)`).
            let upstream = DeadlineSignal::never();
            let mut deadline = deadline(Some(&upstream), timeout_ms, TOOL_TIMEOUT);
            let fused = Arc::new(std::mem::replace(
                &mut deadline.signal,
                DeadlineSignal::never(),
            ));
            let upstream_predicate = exec.signal.lock().clone();
            let poller = {
                let fused = Arc::clone(&fused);
                let upstream_for_poller = upstream_predicate.clone();
                tokio::spawn(async move {
                    loop {
                        if upstream_for_poller() {
                            fused.cancel(None);
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(15)).await;
                    }
                })
            };
            // Swap the fused predicate onto exec for dispatch; the upstream
            // predicate is restored before returning so post-execute
            // listeners never see this plugin's (possibly already-aborted)
            // timeout signal.
            *exec.signal.lock() = {
                let fused = Arc::clone(&fused);
                Arc::new(move || fused.is_cancelled())
            };
            let result = next.call().await;
            poller.abort();
            *exec.signal.lock() = upstream_predicate;
            // If OUR timer fired (scoped by code — a nested outer deadline
            // reads as None here), replace whatever the tool returned with
            // the structured TOOL_TIMEOUT the model sees.
            if timeout_of(fused.reason().as_ref(), Some(TOOL_TIMEOUT)).is_some() {
                return Some(arc(Arc::new(tool_timeout_result(timeout_ms))));
            }
            Some(result)
        })
    });

    let installed: Arc<std::sync::OnceLock<()>> = Arc::new(std::sync::OnceLock::new());
    cordis::make_disposer(move || {
        let ctx = disposer_ctx.clone();
        let listener = listener.clone();
        let installed = installed.clone();
        Box::pin(async move {
            if installed.set(()).is_ok() {
                ctx.on("tools/execute", listener, Default::default()).await;
            }
        })
    })
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `apply`).
pub struct TimeoutPolicyPlugin;

#[async_trait::async_trait]
impl Plugin for TimeoutPolicyPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx);
        (disposer)().await;
        Ok(())
    }
}
