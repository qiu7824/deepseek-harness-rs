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
use dsh_session::SessionEvent;
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

/// How one Activation's residency epoch ended.
#[derive(Debug, Clone, Default)]
pub struct ActivationTerminal {
    /// Why this epoch's last ordinary turn ended, or `error` when teardown
    /// failed.
    pub stop_reason: SubagentStopReason,
    /// The epoch's final assistant content, absent when it produced none or
    /// failed.
    pub output: Option<Vec<dsh_llm::ContentBlock>>,
}

impl Default for SubagentStopReason {
    fn default() -> Self {
        SubagentStopReason::Completed
    }
}

/// Lifecycle observer for one Activation's residency epoch.
#[derive(Clone)]
pub struct ActivationObserver {
    emit_ctx: Context,
    identity: SubagentRunInfo,
    parent: Arc<dyn Agent>,
    boundary: Arc<parking_lot::Mutex<usize>>,
    captured: Arc<parking_lot::Mutex<ActivationTerminal>>,
}

impl ActivationObserver {
    /// Publish the start edge once the epoch is resident.
    pub fn start(&self, child: &Arc<dyn Agent>) {
        *self.boundary.lock() = child.session().events().len();
        emit_lifecycle_edge(
            &self.emit_ctx,
            LifecycleEdge::Start(self.identity.clone(), self.parent.clone()),
        );
    }

    /// Snapshot the child-dependent terminal facts while the child is still
    /// registered.
    pub fn capture(&self, child: &Arc<dyn Agent>) {
        let boundary = *self.boundary.lock();
        let events = child.session().events();
        let own: &[SessionEvent] = &events[boundary.min(events.len())..];
        let output = crate::assistant_output::final_assistant_output(own);
        let captured = ActivationTerminal {
            stop_reason: epoch_stop_reason(own),
            output,
        };
        *self.captured.lock() = captured;
    }

    /// Resolve the terminal facts `settle` will publish.
    pub fn terminal(&self, failure: Option<&str>) -> ActivationTerminal {
        match failure {
            None => self.captured.lock().clone(),
            Some(_) => ActivationTerminal {
                stop_reason: SubagentStopReason::Error,
                output: None,
            },
        }
    }

    /// Publish the terminal edge exactly once.
    pub fn settle(&self, failure: Option<&str>) {
        let terminal = self.terminal(failure);
        emit_lifecycle_edge(
            &self.emit_ctx,
            LifecycleEdge::End(
                SubagentRunEndInfo {
                    run_id: self.identity.run_id.clone(),
                    provider: self.identity.provider.clone(),
                    id: self.identity.id.clone(),
                    local: self.identity.local,
                    stop_reason: terminal.stop_reason,
                    last_assistant_message: terminal.output,
                },
                self.parent.clone(),
            ),
        );
    }
}

/// Build the observer for one continuable Activation's residency epoch.
pub fn create_activation_observer(
    ctx: &Context,
    provider: &str,
    child_id: &dsh_session::SessionId,
    parent: Arc<dyn Agent>,
) -> ActivationObserver {
    let identity = SubagentRunInfo {
        run_id: subagent_run_id(uuid::Uuid::new_v4().to_string()),
        provider: provider.to_string(),
        id: child_id.clone(),
        local: true,
    };
    ActivationObserver {
        emit_ctx: ctx.clone(),
        identity,
        parent,
        boundary: Arc::new(parking_lot::Mutex::new(0)),
        captured: Arc::new(parking_lot::Mutex::new(ActivationTerminal {
            stop_reason: SubagentStopReason::Completed,
            output: None,
        })),
    }
}

/// Why this child's epoch ended, for the terminal lifecycle edge and the
/// manager's own parent delivery.
fn epoch_stop_reason(events: &[SessionEvent]) -> SubagentStopReason {
    let consumed = dsh_agent::fold_consumed_work(events);
    let reason_kind = consumed
        .end
        .as_ref()
        .and_then(|event| event.data.get("reason"))
        .and_then(|reason| reason.get("kind"))
        .and_then(|kind| kind.as_str());
    match reason_kind {
        Some("max-tokens") => SubagentStopReason::MaxTokens,
        Some("aborted") | Some("interrupted") => SubagentStopReason::Aborted,
        Some("error") => SubagentStopReason::Error,
        Some("blocked") => SubagentStopReason::Refusal,
        Some("completed") | None => {
            if consumed.dropped_unrun {
                SubagentStopReason::Aborted
            } else {
                SubagentStopReason::Completed
            }
        }
        Some(_) => SubagentStopReason::Error,
    }
}
