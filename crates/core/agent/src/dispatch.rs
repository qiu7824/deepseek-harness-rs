//! Agent-scoped dispatch and prompt assembly helpers. Rust port of
//! `packages/core/agent/src/dispatch.ts`.
//!
//! The fused dispatcher couples the agent subject to its scope carrier: the
//! payload builder receives the dispatcher's exact agent, so the scope key
//! and the payload's `agent` cannot diverge (the TS compile-time
//! `PayloadRest<K>` contract, enforced here by construction).

use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use cordis::{ArcValue, BoxFuture, Context, DispatchMode, arc};
use dsh_scope::{ScopeCarrier, scope_target};
use dsh_system_prompt::AssembleContext;
use futures::FutureExt;

use crate::runtime_types::Agent;

/// Build the fused scope carrier for one agent subject (TS `agentCarrier`).
pub fn agent_carrier(agent: &Arc<dyn Agent>) -> ScopeCarrier {
    scope_target(None, Some(agent.scope_key().clone()))
}

/// A dispatcher that couples the agent subject to its scope carrier (TS
/// `agentEvents`). Build it once per agent and reuse it.
#[derive(Clone)]
pub struct AgentEventDispatch {
    ctx: Context,
    agent: Arc<dyn Agent>,
    carrier: ScopeCarrier,
}

impl AgentEventDispatch {
    pub fn new(ctx: &Context, agent: Arc<dyn Agent>) -> Self {
        Self {
            ctx: ctx.clone(),
            carrier: agent_carrier(&agent),
            agent,
        }
    }

    fn dispatch_ctx(&self) -> Context {
        self.ctx.with_filter(self.carrier.filter.clone())
    }

    /// Fire-and-forget notification in the agent's scope, with per-listener
    /// containment (TS `emit`). The builder receives the subject agent.
    pub fn emit(&self, name: &str, build: impl FnOnce(&Arc<dyn Agent>) -> ArcValue) {
        let payload = build(&self.agent);
        let dispatch_ctx = self.dispatch_ctx();
        let listeners = dispatch_ctx.events.collect(
            DispatchMode::Emit,
            Some(&dispatch_ctx),
            name,
            std::slice::from_ref(&payload),
        );
        let logger = self.ctx.named_logger(Some("agents"));
        let mut pending = Vec::new();
        for (listener_ctx, callback) in &listeners {
            let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(listener_ctx, vec![payload.clone()])
            }));
            let mut future = match future {
                Ok(future) => future,
                Err(error) => {
                    logger.warn(vec![arc(format!(
                        "agent event \"{name}\" listener threw: {}",
                        crate::registry::render_panic(error)
                    ))]);
                    continue;
                }
            };
            let waker = futures::task::noop_waker();
            let mut task_context = TaskContext::from_waker(&waker);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(&mut task_context)
            })) {
                Ok(Poll::Ready(_)) => {}
                Ok(Poll::Pending) => {
                    pending.push(future);
                }
                Err(error) => {
                    logger.warn(vec![arc(format!(
                        "agent event \"{name}\" listener threw: {}",
                        crate::registry::render_panic(error)
                    ))]);
                }
            }
        }
        for future in pending {
            let logger = logger.clone();
            let name = name.to_string();
            let task = async move {
                if let Err(error) = std::panic::AssertUnwindSafe(future).catch_unwind().await {
                    logger.warn(vec![arc(format!(
                        "agent event \"{name}\" listener threw: {}",
                        crate::registry::render_panic(error)
                    ))]);
                }
            };
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(task);
                }
                Err(_) => {
                    std::thread::spawn(move || futures::executor::block_on(task));
                }
            }
        }
    }

    /// Awaited in-order dispatch (Cordis `serial`) in the agent's scope.
    pub fn serial(
        &self,
        name: &str,
        build: impl FnOnce(&Arc<dyn Agent>) -> ArcValue,
    ) -> BoxFuture<'static, Option<ArcValue>> {
        let payload = build(&self.agent);
        let dispatch_ctx = self.dispatch_ctx();
        let name = name.to_string();
        let events = dispatch_ctx.events.clone();
        Box::pin(async move {
            events
                .serial(Some(&dispatch_ctx), &name, vec![payload])
                .await
        })
    }

    /// Around-middleware dispatch (Cordis `waterfall`) in the agent's scope.
    pub fn waterfall(
        &self,
        name: &str,
        build: impl FnOnce(&Arc<dyn Agent>) -> ArcValue,
        fallback: BoxFuture<'static, ArcValue>,
    ) -> BoxFuture<'static, ArcValue> {
        let payload = build(&self.agent);
        let dispatch_ctx = self.dispatch_ctx();
        dispatch_ctx.waterfall(name, vec![payload], fallback)
    }
}

/// Emit one contained agent notification without allocating a retained
/// dispatcher (TS `emitAgentEvent`).
pub fn emit_agent_event(
    ctx: &Context,
    agent: &Arc<dyn Agent>,
    name: &str,
    build: impl FnOnce(&Arc<dyn Agent>) -> ArcValue,
) {
    AgentEventDispatch::new(ctx, agent.clone()).emit(name, build)
}

/// Build the prompt assembly context with the agent's scope set, so
/// agent-scoped prompt and tool contributions cannot be silently omitted
/// (TS `assembleContextFor`; the `agent` field itself rides the
/// merge-extensible context fields).
pub fn assemble_context_for(agent: &Arc<dyn Agent>) -> AssembleContext {
    let mut fields = serde_json::Map::new();
    fields.insert(
        "sessionId".to_string(),
        serde_json::Value::String(agent.session().id().to_string()),
    );
    if let Some(cwd) = &agent.session().header().cwd {
        fields.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
    }
    AssembleContext {
        scope: Some(agent.scope_key().clone()),
        fields,
    }
}
