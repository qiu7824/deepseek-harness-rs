//! Lifecycle-edge publication for both subagent shapes: the contained
//! emitter and the one-shot run observer. Rust port of
//! `packages/subagent/subagent/src/lifecycle.ts` (the continuation
//! Activation observer arrives with the continuation manager).
//!
//! # Deviations
//!
//! - Scoped dispatch keys the carrier by the delegating parent's scope key
//!   (the TS carrier is `scopeTarget(service, parent)`; the Rust base filter
//!   is the plain service context filter).
//! - The TS emitter dispatches `(carrier, name, info)` through the events
//!   registry; the Rust emitter builds the filtered dispatch context with
//!   [`ScopeCarrier::filter`] and collects listeners through it.

use std::sync::Arc;

use cordis::{ArcValue, Context, DispatchMode, Listener, arc};
use dsh_agent::Agent;
use dsh_scope::{ScopeCarrier, scope_target};

use crate::types::{SubagentRun, SubagentRunEndInfo, SubagentRunInfo, SubagentStopReason, subagent_run_id};

/// Publish one lifecycle edge with per-listener exception containment.
pub enum LifecycleEdge {
    Start(SubagentRunInfo, Arc<dyn Agent>),
    End(SubagentRunEndInfo, Arc<dyn Agent>),
    ProviderRemoved(String),
}

/// Build the contained lifecycle emitter this seam publishes every edge
/// through.
pub fn emit_lifecycle_edge(ctx: &Context, edge: LifecycleEdge) {
    let (name, info, parent): (&str, Option<ArcValue>, Option<Arc<dyn Agent>>) = match &edge {
        LifecycleEdge::Start(info, parent) => ("subagent/start", Some(arc(info.clone())), Some(parent.clone())),
        LifecycleEdge::End(info, parent) => ("subagent/end", Some(arc(info.clone())), Some(parent.clone())),
        LifecycleEdge::ProviderRemoved(info) => ("subagent/provider-removed", Some(arc(info.clone())), None),
    };
    let args: Vec<ArcValue> = match &info {
        Some(info) => vec![info.clone()],
        None => Vec::new(),
    };
    let listeners: Vec<(Context, Arc<Listener>)> = match parent {
        None => ctx.collect(DispatchMode::Emit, name, &args),
        Some(parent) => {
            let carrier: ScopeCarrier = scope_target(None, Some(parent.scope_key().clone()));
            let dispatch_ctx = ctx.with_filter(carrier.filter);
            ctx.events.collect(DispatchMode::Emit, Some(&dispatch_ctx), name, &args)
        }
    };
    for (listener_ctx, callback) in listeners {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(callback(&listener_ctx, args.clone()))
        }));
        match outcome {
            Ok(Some(_)) | Ok(None) => {}
            Err(_) => {
                ctx.logger.warn(
                    ctx,
                    vec![arc(format!("subagent: {name} listener threw: unknown"))],
                );
            }
        }
    }
}

/// Emit the start/end lifecycle pair for one accepted one-shot run.
pub fn observe_run(
    ctx: &Context,
    provider: &str,
    parent: Arc<dyn Agent>,
    run: Arc<dyn SubagentRun>,
) -> Arc<dyn SubagentRun> {
    let identity = SubagentRunInfo {
        run_id: subagent_run_id(uuid::Uuid::new_v4().to_string()),
        provider: provider.to_string(),
        id: run.id().clone(),
        local: run.local_agent().is_some(),
    };
    // Attach the terminal observer before dispatching start, preserving
    // start -> end ordering.
    let end_ctx = ctx.clone();
    let end_identity = SubagentRunEndInfo {
        run_id: identity.run_id.clone(),
        provider: identity.provider.clone(),
        id: identity.id.clone(),
        local: identity.local,
        stop_reason: SubagentStopReason::Error,
        last_assistant_message: None,
    };
    let end_parent = parent.clone();
    let end_run = run.clone();
    tokio::spawn(async move {
        match end_run.result().await {
            Ok(result) => {
                let info = SubagentRunEndInfo {
                    stop_reason: result.stop_reason,
                    last_assistant_message: if result.output.is_empty() {
                        None
                    } else {
                        Some(result.output)
                    },
                    ..end_identity
                };
                emit_lifecycle_edge(&end_ctx, LifecycleEdge::End(info, end_parent));
            }
            Err(_) => {
                let info = SubagentRunEndInfo {
                    stop_reason: SubagentStopReason::Error,
                    last_assistant_message: None,
                    ..end_identity
                };
                emit_lifecycle_edge(&end_ctx, LifecycleEdge::End(info, end_parent));
            }
        }
    });
    emit_lifecycle_edge(ctx, LifecycleEdge::Start(identity, parent));
    run
}
