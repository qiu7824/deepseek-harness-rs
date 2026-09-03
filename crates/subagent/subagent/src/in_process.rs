//! Shared driver for in-process ONE-SHOT subagent providers. Rust port of
//! `packages/subagent/subagent-in-process-driver/src/index.ts` (the
//! structured-output runtime in `structured.ts` arrives later).
//!
//! # Deviations
//!
//! - Structured output (`outputSchema` capture) is not ported: providers
//!   advertise `output_schema: false` until the capture-tool runtime lands.
//! - The abort predicate replaces `AbortSignal`; the pre-publication abort
//!   check happens before depth resolution, like the TS flow.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dsh_agent::{Agent, fold_consumed_work};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_session::{SessionEvent, SessionId, TurnEndReason, session_id};

use crate::assistant_output::final_assistant_output;
use crate::child_agent::{
    ChildComposition, append_delegated_policy_overrides, apply_child_composition,
    capture_delegated_policy_overrides, child_session_meta, resolve_child_agent_options,
    resolve_child_depth,
};
use crate::error::SubagentError;
use crate::types::{ResolvedSubagentStartRequest, SubagentResult, SubagentRun, SubagentStopReason};

/// Map a session turn outcome to the subagent seam's terminal vocabulary.
pub fn to_stop_reason(reason: Option<&TurnEndReason>) -> SubagentStopReason {
    match reason {
        None => SubagentStopReason::Error,
        Some(reason) => match reason.kind() {
            "completed" => SubagentStopReason::Completed,
            "max-tokens" => SubagentStopReason::MaxTokens,
            "aborted" => SubagentStopReason::Aborted,
            "blocked" => SubagentStopReason::Refusal,
            _ => SubagentStopReason::Error,
        },
    }
}

/// Extra inputs the spawn and fork providers supply to the shared driver.
#[derive(Debug, Clone, Default)]
pub struct InProcessRunOptions {
    /// Completed-turn seed for fork, or `None` for a fresh spawn.
    pub seed: Option<Vec<SessionEvent>>,
}

/// Establish and drive one in-process one-shot child.
pub async fn start_in_process_run(
    request: &ResolvedSubagentStartRequest,
    options: InProcessRunOptions,
) -> Result<Arc<dyn SubagentRun>, SubagentError> {
    crate::depth::assert_subagent_max_depth(request.request.max_depth)
        .map_err(|message| SubagentError::new("INVALID_MAX_DEPTH", message))?;
    if (request.request.signal)() {
        return Err(SubagentError::new(
            "CANCELLED",
            "subagent request was aborted before child publication",
        ));
    }
    let parent = request.request.parent.clone();
    let child_depth = resolve_child_depth(parent.as_ref(), request.request.max_depth)
        .map_err(|error| SubagentError::new("DEPTH_EXCEEDED", error.message))?;

    let child_id: SessionId = session_id(uuid::Uuid::new_v4().to_string());
    let activation_boundary = options.seed.as_ref().map(|seed| seed.len()).unwrap_or(0);

    // Capture before the first await: a later parent switch belongs to the
    // parent's future.
    let inherited = capture_delegated_policy_overrides(parent.as_ref());

    let registry = parent
        .ctx()
        .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            SubagentError::new(
                "CONTINUATION_UNAVAILABLE",
                "subagent requires the agents service",
            )
        })?;

    let inherited_event_count = options
        .seed
        .as_ref()
        .map(|_| dsh_session::SessionLogOffset::new(activation_boundary as u64))
        .transpose()
        .map_err(|error| SubagentError::new("CHILD_CREATE_FAILED", error))?;
    let handle = registry
        .create(dsh_agent::CreateAgentOptions {
            session_id: Some(child_id.clone()),
            meta: Some(child_session_meta(
                parent.as_ref(),
                child_depth,
                activation_boundary as u64,
            )),
            seed: options.seed,
            inherited_event_count,
            agent_options: Some(resolve_child_agent_options(
                parent.as_ref(),
                request.request.agent_options.as_ref(),
                child_depth,
            )),
            setup: None,
        })
        .await
        .map_err(|error| SubagentError::new("CHILD_CREATE_FAILED", error))?;

    if let Err(error) = handle.agent.session().append(
        "subagent/descriptor",
        serde_json::to_value(&request.descriptor).expect("descriptor json"),
        None,
    ) {
        handle.dispose.await;
        return Err(SubagentError::new("CHILD_COMPOSE_FAILED", error));
    }

    let structured = if let Some(schema) = request.request.output_schema.clone() {
        match crate::structured::attach_structured_runtime(handle.agent.ctx(), schema).await {
            Ok(attachment) => Some(attachment),
            Err(error) => {
                handle.dispose.await;
                return Err(SubagentError::new("CHILD_COMPOSE_FAILED", error));
            }
        }
    } else {
        None
    };

    // Compose the child right after publication but before its first turn:
    // the delegation policy seeds and the scoped persona/restriction land
    // between the fork seed and the child's own events (the TS driver runs
    // them inside the unpublished creation window — deviation).
    {
        let child = handle.agent.clone();
        if let Err(error) = append_delegated_policy_overrides(child.session(), &inherited) {
            handle.dispose.await;
            return Err(SubagentError::new("CHILD_COMPOSE_FAILED", error));
        }
        apply_child_composition(
            child.ctx(),
            parent.as_ref(),
            &ChildComposition {
                persona: request.request.persona.clone(),
                tool_filter: request.request.tool_filter.clone(),
            },
        );
    }

    Ok(drive_published_run(
        handle,
        request.request.signal.clone(),
        &request.request.prompt,
        child_id,
        activation_boundary,
        structured,
    ))
}

/// Wrap a published child in the single run lifecycle that owns signal
/// handoff, one turn, result settlement, and quiescent disposal.
fn drive_published_run(
    handle: dsh_agent::AgentHandle,
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
    prompt: &[ContentBlock],
    child_id: SessionId,
    boundary: usize,
    structured: Option<crate::structured::StructuredAttachment>,
) -> Arc<dyn SubagentRun> {
    let child = handle.agent.clone();
    let cancelled = Arc::new(AtomicBool::new(false));

    let result_child = child.clone();
    let result_cancelled = cancelled.clone();
    let result_signal = signal.clone();
    let prompt = prompt.to_vec();
    let result_future = tokio::spawn(async move {
        if result_signal() {
            result_cancelled.store(true, Ordering::SeqCst);
        }
        if !result_cancelled.load(Ordering::SeqCst) {
            result_child.followup(create_user_message(
                prompt,
                MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            ));
            result_child.when_idle().await;
        }
        read_result(
            &result_child,
            boundary,
            result_cancelled.load(Ordering::SeqCst),
            structured.as_ref(),
        )
    });

    struct PublishedRun {
        id: SessionId,
        child: Arc<dyn Agent>,
        cancelled: Arc<AtomicBool>,
        result: parking_lot::Mutex<Option<tokio::task::JoinHandle<SubagentResult>>>,
        handle: parking_lot::Mutex<Option<dsh_agent::AgentHandle>>,
    }

    #[async_trait::async_trait]
    impl SubagentRun for PublishedRun {
        fn id(&self) -> &SessionId {
            &self.id
        }

        fn local_agent(&self) -> Option<Arc<dyn Agent>> {
            Some(self.child.clone())
        }

        async fn result(&self) -> Result<SubagentResult, String> {
            let handle = self.result.lock().take();
            match handle {
                Some(handle) => handle
                    .await
                    .map_err(|error| format!("subagent run task failed: {error}")),
                None => Err("subagent run result was already consumed".to_string()),
            }
        }

        async fn dispose(&self) -> Result<(), String> {
            self.cancelled.store(true, Ordering::SeqCst);
            self.child
                .cancel(dsh_session::AgentCancelCause::Parent, None);
            let handle = { self.handle.lock().take() };
            if let Some(handle) = handle {
                handle.dispose.await;
            }
            let _ = { self.result.lock().take() };
            Ok(())
        }
    }

    Arc::new(PublishedRun {
        id: child_id,
        child,
        cancelled,
        result: parking_lot::Mutex::new(Some(result_future)),
        handle: parking_lot::Mutex::new(Some(handle)),
    })
}

/// Read one settled child's result from events after its activation
/// boundary.
fn read_result(
    child: &Arc<dyn Agent>,
    boundary: usize,
    cancelled: bool,
    structured: Option<&crate::structured::StructuredAttachment>,
) -> SubagentResult {
    let events = child.session().events();
    let own: &[SessionEvent] = &events[boundary.min(events.len())..];
    let last_end = fold_consumed_work(own).end;
    let output = final_assistant_output(own).unwrap_or_default();
    let recorded = to_stop_reason(
        last_end
            .as_ref()
            .and_then(|event| {
                serde_json::from_value::<TurnEndReason>(
                    event.data.get("reason").cloned().unwrap_or_default(),
                )
                .ok()
            })
            .as_ref(),
    );
    let stop_reason = if cancelled && recorded != SubagentStopReason::Completed {
        SubagentStopReason::Aborted
    } else {
        recorded
    };
    let captured = structured.and_then(crate::structured::StructuredAttachment::captured);
    let stop_reason = if structured.is_some()
        && captured.is_none()
        && stop_reason == SubagentStopReason::Completed
    {
        SubagentStopReason::Error
    } else {
        stop_reason
    };
    SubagentResult {
        output,
        structured: captured,
        stop_reason,
    }
}
