//! Schedules one assistant step's tool calls. Exclusive calls form
//! barriers; parallel calls use a bounded rolling pool and are reclassified
//! before start. Rust port of `packages/core/agent-loop/src/tool-calls.ts`.
//!
//! # Deviations
//!
//! - The initiating agent and the parallel cap are explicit parameters
//!   (the TS reads `ctx.agents.requireInitiator()` and
//!   `ctx.agentLoop.config.maxParallelToolCalls` from the loop service,
//!   which lands with the loop scheduler).
//! - The overlapping dispatch pool is a `FuturesUnordered` of staged
//!   `dispatch_scheduled` futures (the TS promise pool); ordered
//!   pre-execute stays sequential and the model-order commit barrier is
//!   identical.

use std::sync::Arc;

use dsh_agent::Agent;
use dsh_llm::{ToolCallBlock, ToolResultMessageInput, create_tool_result_message};
use dsh_session::{Session, SurfaceIntent, SurfaceOp};
use dsh_tools::{
    AbortPredicate, DispatchOutcome, Preparation, TOOL_ABORTED_BEFORE_DISPATCH,
    ToolExecutionInput, ToolExecutionMode, ToolExecutionResult, ToolRunContext, ToolRuntime,
};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::Value as JsonValue;

/// One tool call after argument parsing, ready to schedule.
struct PlannedCall {
    block: ToolCallBlock,
    exec: ToolExecutionInput,
}

/// Settled dispatch awaiting model-order finalization.
struct Slot {
    exec: Arc<ToolRunContext>,
    result: Arc<ToolExecutionResult>,
    needs_post: bool,
}

/// One scheduler group outcome, including a drained cancellation.
struct GroupOutcome {
    consumed: usize,
    aborted: bool,
    concluded: bool,
}

/// Accept committed result context for the next step boundary.
pub type ContextAcceptor = Arc<dyn Fn(dsh_llm::UserMessage) + Send + Sync>;

/// The overlapping dispatch pool: settled in completion order, committed in
/// model order.
type InFlight = FuturesUnordered<
    futures::future::BoxFuture<'static, (usize, Arc<ToolRunContext>, DispatchOutcome)>,
>;

/// Schedule one assistant step's tool calls by their live concurrency
/// mode. Ordinary completion and abort commit started-call results in
/// order; abort drains them and records synthetic results for unstarted
/// calls. An internal scheduler failure stops new dispatches, drains
/// already-started dispatches, and returns the first failure without
/// fabricating tool results.
///
/// Returns whether any committed result carried `concludesTurn`.
pub async fn execute_tool_calls(
    tools: &Arc<ToolRuntime>,
    agent: Arc<dyn Agent>,
    max_parallel_tool_calls: usize,
    turn: u64,
    step: u64,
    tool_calls: Vec<ToolCallBlock>,
    signal: AbortPredicate,
    accept_context: ContextAcceptor,
) -> Result<bool, String> {
    let session = agent.session().clone();
    let planned: Vec<PlannedCall> = tool_calls
        .into_iter()
        .map(|block| PlannedCall {
            exec: ToolExecutionInput {
                call_id: block.id.clone(),
                root_call_id: None,
                name: block.name.clone(),
                arguments: parse_arguments(&block.arguments),
                agent: Some(Arc::clone(&agent)),
                parent: None,
                signal: Arc::clone(&signal),
            },
            block,
        })
        .collect();

    let mut next = 0;
    let mut concluded = false;
    while next < planned.len() {
        let first = &planned[next];
        let mode = tools.execution_mode(&first.exec);
        let group = if mode == ToolExecutionMode::Parallel {
            &planned[next..]
        } else {
            &planned[next..next + 1]
        };
        let outcome = run_group(
            tools,
            &session,
            max_parallel_tool_calls,
            turn,
            step,
            group,
            mode,
            Arc::clone(&signal),
            Arc::clone(&accept_context),
        )
        .await?;
        next += outcome.consumed;
        concluded |= outcome.concluded;
        if outcome.aborted {
            for call in &planned[next..] {
                append_skipped_tool_call(&session, turn, step, &call.block)?;
            }
            return Ok(concluded);
        }
    }
    Ok(concluded)
}

/// Parse model arguments, preserving invalid JSON as text and mapping
/// empty input to `{}`.
fn parse_arguments(raw: &str) -> JsonValue {
    if raw.is_empty() {
        return JsonValue::Object(serde_json::Map::new());
    }
    match serde_json::from_str::<JsonValue>(raw) {
        Ok(value) => value,
        Err(_) => JsonValue::String(raw.to_string()),
    }
}

/// Run one exclusive barrier or parallel pool. Later calls are
/// reclassified before start; results and contexts commit in model order.
#[allow(clippy::too_many_arguments)]
async fn run_group(
    tools: &Arc<ToolRuntime>,
    session: &Session,
    max_parallel_tool_calls: usize,
    turn: u64,
    step: u64,
    group: &[PlannedCall],
    mode: ToolExecutionMode,
    signal: AbortPredicate,
    accept_context: ContextAcceptor,
) -> Result<GroupOutcome, String> {
    let mut state = GroupState {
        slots: (0..group.len()).map(|_| None).collect(),
        call_seqs: (0..group.len()).map(|_| None).collect(),
        next_to_start: 0,
        committed: 0,
        started: 0,
        aborted: signal(),
        concluded: false,
        scheduler_failure: None,
        in_flight: InFlight::new(),
    };

    fill_pool(
        tools,
        session,
        max_parallel_tool_calls,
        turn,
        step,
        group,
        mode,
        &signal,
        &accept_context,
        &mut state,
    )
    .await?;
    while !state.in_flight.is_empty() {
        if let Some(failure) = &state.scheduler_failure {
            drain(&mut state.in_flight).await;
            return Err(failure.clone());
        }
        let (index, exec, outcome) = state.in_flight.next().await.expect("in-flight pool");
        match outcome {
            DispatchOutcome::PostResult(result) => {
                state.slots[index] = Some(Slot { exec, result, needs_post: true });
            }
            DispatchOutcome::FinalResult(result) => {
                state.slots[index] = Some(Slot { exec, result, needs_post: false });
            }
        }
        commit_ready(tools, session, turn, step, group, &accept_context, &mut state).await?;
        if signal() {
            state.aborted = true;
        }
        fill_pool(
            tools,
            session,
            max_parallel_tool_calls,
            turn,
            step,
            group,
            mode,
            &signal,
            &accept_context,
            &mut state,
        )
        .await?;
    }

    if state.aborted {
        for call in &group[state.started..] {
            append_skipped_tool_call(session, turn, step, &call.block)?;
        }
        return Ok(GroupOutcome {
            consumed: group.len(),
            aborted: true,
            concluded: state.concluded,
        });
    }
    if state.committed != state.started {
        return Err("tool-call scheduler: uncommitted settled calls".to_string());
    }
    Ok(GroupOutcome {
        consumed: state.started,
        aborted: false,
        concluded: state.concluded,
    })
}

struct GroupState {
    slots: Vec<Option<Slot>>,
    call_seqs: Vec<Option<u64>>,
    next_to_start: usize,
    committed: usize,
    started: usize,
    aborted: bool,
    concluded: bool,
    scheduler_failure: Option<String>,
    in_flight: InFlight,
}

/// Start as many calls as the bounded pool admits. Ordered pre-execute
/// stays sequential; only dispatch/body overlaps. Later calls are
/// reclassified after ordered commits so registry changes can create a
/// barrier.
#[allow(clippy::too_many_arguments)]
async fn fill_pool(
    tools: &Arc<ToolRuntime>,
    session: &Session,
    max_parallel_tool_calls: usize,
    turn: u64,
    step: u64,
    group: &[PlannedCall],
    mode: ToolExecutionMode,
    signal: &AbortPredicate,
    accept_context: &ContextAcceptor,
    state: &mut GroupState,
) -> Result<(), String> {
    loop {
        if state.aborted
            || state.next_to_start >= group.len()
            || state.in_flight.len() >= max_parallel_tool_calls
        {
            break;
        }
        if state.next_to_start > 0
            && mode == ToolExecutionMode::Parallel
            && tools.execution_mode(&group[state.next_to_start].exec)
                != ToolExecutionMode::Parallel
        {
            break;
        }
        let index = state.next_to_start;
        state.next_to_start += 1;
        let call_seq = append_tool_call(session, turn, step, &group[index].block)?;
        state.call_seqs[index] = Some(call_seq);
        state.started += 1;
        let prepared = tools.prepare_scheduled(group[index].exec.clone()).await;
        match prepared {
            Preparation::Dispatch { run_ctx } => {
                let tools_for_dispatch = Arc::clone(tools);
                state.in_flight.push(Box::pin(async move {
                    let outcome = tools_for_dispatch
                        .dispatch_scheduled(Arc::clone(&run_ctx))
                        .await;
                    (index, run_ctx, outcome)
                }));
            }
            Preparation::PostResult { run_ctx, result } => {
                state.slots[index] = Some(Slot { exec: run_ctx, result, needs_post: true });
            }
            Preparation::FinalResult { run_ctx, result } => {
                state.slots[index] = Some(Slot { exec: run_ctx, result, needs_post: false });
            }
        }
        commit_ready(tools, session, turn, step, group, accept_context, state).await?;
        if signal() {
            state.aborted = true;
        }
    }
    Ok(())
}

/// Commit every contiguous settled slot in model order.
#[allow(clippy::too_many_arguments)]
async fn commit_ready(
    tools: &Arc<ToolRuntime>,
    session: &Session,
    turn: u64,
    step: u64,
    group: &[PlannedCall],
    accept_context: &ContextAcceptor,
    state: &mut GroupState,
) -> Result<(), String> {
    while state.committed < group.len() {
        if state.slots[state.committed].is_none() {
            break;
        }
        let slot = state.slots[state.committed].take().expect("checked");
        let result = if slot.needs_post {
            tools
                .finalize_scheduled(Arc::clone(&slot.exec), Arc::clone(&slot.result))
                .await
        } else {
            tools.finish_scheduled(Arc::clone(&slot.exec), Arc::clone(&slot.result))
        };
        append_tool_result(
            session,
            turn,
            step,
            &group[state.committed].block,
            &result,
            state.call_seqs[state.committed].expect("started call seq"),
        )?;
        for context in &result.additional_contexts {
            (accept_context)(context.clone());
        }
        state.concluded |= result.concludes_turn;
        state.committed += 1;
    }
    Ok(())
}

/// Drain in-flight dispatches without committing their results (the TS
/// `Promise.allSettled` on scheduler failure).
async fn drain(in_flight: &mut InFlight) {
    while in_flight.next().await.is_some() {}
}

/// Append the durable call/result pair for a model call skipped after
/// cancellation.
fn append_skipped_tool_call(
    session: &Session,
    turn: u64,
    step: u64,
    block: &ToolCallBlock,
) -> Result<(), String> {
    let call_seq = append_tool_call(session, turn, step, block)?;
    append_tool_result(
        session,
        turn,
        step,
        block,
        &Arc::new(ToolExecutionResult {
            content: vec![dsh_llm::ContentBlock::Text {
                text: "Error: tool call aborted before dispatch".to_string(),
            }],
            is_error: true,
            error: Some(dsh_tools::ToolFailure {
                message: "tool call aborted before dispatch".to_string(),
                info: Some(dsh_tools::ToolErrorInfo {
                    name: "AbortError".to_string(),
                    code: TOOL_ABORTED_BEFORE_DISPATCH.to_string(),
                }),
            }),
            value: None,
            meta: None,
            additional_contexts: Vec::new(),
            concludes_turn: false,
            canonical_token: 0,
        }),
        call_seq,
    )
}

/// Append a started call and return the event seq that its result must
/// cite.
fn append_tool_call(
    session: &Session,
    turn: u64,
    step: u64,
    block: &ToolCallBlock,
) -> Result<u64, String> {
    let data = serde_json::json!({
        "turn": turn,
        "step": step,
        "callId": block.id,
        "name": block.name,
        "arguments": block.arguments,
    });
    let event = session.append("tool/call", data, None)?;
    Ok(event.seq)
}

/// Append a model-ordered result linked to its call event.
fn append_tool_result(
    session: &Session,
    turn: u64,
    step: u64,
    block: &ToolCallBlock,
    result: &Arc<ToolExecutionResult>,
    call_seq: u64,
) -> Result<(), String> {
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id: block.id.clone(),
        content: result.content.clone(),
        is_error: result.is_error,
    });
    let mut data = serde_json::json!({
        "turn": turn,
        "step": step,
        "message": message,
    });
    if let Some(info) = result.error.as_ref().and_then(|error| error.info.as_ref()) {
        data["error"] = serde_json::json!({ "name": info.name, "code": info.code });
    }
    if let Some(meta) = &result.meta {
        data["meta"] = meta.clone();
    }
    session.append(
        "tool/result",
        data,
        Some(SurfaceIntent {
            surface_op: SurfaceOp::Append,
            source_event_seqs: Some(vec![call_seq]),
        }),
    )?;
    Ok(())
}
