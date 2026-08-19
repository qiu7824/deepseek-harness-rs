//! Surface layer on top of the session event log: an ordered view of events
//! that produce LLM messages. Rust port of
//! `packages/core/session/src/surface.ts`.
//!
//! The append-only log remains the source of truth; the fold replays
//! `surfaceOp` markers into the ordered node list. The TS `SurfaceManager`
//! holds a live reference to the session's log array; Rust keeps the fold
//! state beside the log and receives the current log slice per call (the
//! session owns both under one lock, so no aliasing hazard exists).

use dsh_llm::Message;
use serde_json::Value as JsonValue;

use crate::types::{SessionEvent, SurfaceOp};

/// Runtime counterpart of the message-producing event union.
pub const SURFACE_EVENT_TYPES: [&str; 3] = ["user/message", "assistant/message", "tool/result"];

/// Whether an event type can join the model-visible surface.
pub fn is_surface_eligible_type(type_: &str) -> bool {
    SURFACE_EVENT_TYPES.contains(&type_)
}

/// Narrow an event to a surface-eligible event carrying its required marker.
pub fn is_surface_event(event: &SessionEvent) -> bool {
    if !is_surface_eligible_type(&event.type_) {
        return false;
    }
    event.surface_op.is_some()
}

/// Narrow an event to an append-origin surface event: one that entered the
/// surface at its own log position and was never itself a replacement copy.
pub fn is_append_surface_event(event: &SessionEvent) -> bool {
    is_surface_event(event) && matches!(event.surface_op, Some(SurfaceOp::Append))
}

/// Narrow an event to a surface replacement: a node that shadowed an
/// existing surface range instead of appending to the tail.
pub fn is_replacement_surface_event(event: &SessionEvent) -> bool {
    is_surface_event(event) && !matches!(event.surface_op, Some(SurfaceOp::Append))
}

/// Project a single event into the LLM message it derives to, or `None`
/// when it produces none (TS `deriveEventMessage`).
pub fn derive_event_message(event: &SessionEvent) -> Option<Message> {
    match event.type_.as_str() {
        "user/message" => serde_json::from_value::<Message>(event.data.clone()).ok(),
        "assistant/message" => {
            let message: Message =
                serde_json::from_value(event.data.get("message")?.clone()).ok()?;
            // Skip an empty-content assistant/message: it exists only to host
            // a max-tokens step's usage.
            if message.content.is_empty() {
                return None;
            }
            Some(message)
        }
        "tool/result" => serde_json::from_value::<Message>(event.data.get("message")?.clone()).ok(),
        _ => None,
    }
}

/// One replacement operation observed while folding a session surface.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceFoldReplacement {
    /// Seq of the event that replaced the prior surface range.
    pub seq: u64,
    /// Declared inclusive start seq of the replaced surface range.
    pub start: u64,
    /// Declared inclusive end seq of the replaced surface range.
    pub end: u64,
    /// Actual surface entries removed by the operation, in surface order.
    pub shadowed_seqs: Vec<u64>,
}

/// Complete result of replaying the surface operations in a session log.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceFoldResult {
    /// Current surface event sequences in model-visible order.
    pub nodes: Vec<u64>,
    /// Replacement operations in event order.
    pub replacements: Vec<SurfaceFoldReplacement>,
}

/// Readonly live projection of the message-producing session events.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSurface {
    /// Current surface event sequences in model-visible order.
    pub nodes: Vec<u64>,
    /// Monotonic count of committed positional replacements.
    pub replace_generation: u64,
}

/// Mutable state shared by complete and incremental folds.
#[derive(Debug, Clone, Default)]
struct SurfaceFoldState {
    nodes: Vec<u64>,
    replace_generation: u64,
}

/// A validated replacement transition that has not mutated fold state yet.
#[derive(Debug, Clone, PartialEq)]
struct SurfaceReplacePlan {
    seq: u64,
    start: u64,
    end: u64,
    start_idx: usize,
    end_idx: usize,
    shadowed_seqs: Vec<u64>,
}

/// One validated surface transition that has not mutated fold state yet.
#[derive(Debug, Clone, PartialEq)]
enum SurfacePlan {
    Append { seq: u64 },
    Replace(SurfaceReplacePlan),
}

/// Validate event-local surface eligibility and return its operation.
fn surface_op_of(event: &SessionEvent) -> Result<Option<SurfaceOp>, String> {
    if !is_surface_eligible_type(&event.type_) {
        if event.surface_op.is_some() {
            return Err(format!(
                "session event \"{}\" is not surface-eligible and cannot carry surfaceOp",
                event.type_
            ));
        }
        if event.source_event_seqs.is_some() {
            return Err(format!(
                "session event \"{}\" is not surface-eligible and cannot carry sourceEventSeqs",
                event.type_
            ));
        }
        return Ok(None);
    }
    match &event.surface_op {
        None => Err(format!(
            "session event \"{}\" is surface-eligible and requires a surfaceOp marker",
            event.type_
        )),
        Some(op) => Ok(Some(op.clone())),
    }
}

/// Validate cited source-event seqs against prior log entries and the
/// replacement range (TS `assertProvenance`).
fn assert_provenance(event: &SessionEvent, shadowed_seqs: &[u64]) -> Result<(), String> {
    let mut sources = std::collections::HashSet::new();
    if let Some(raw) = &event.source_event_seqs {
        if raw.is_empty() && event.type_ != "assistant/message" {
            return Err(
                "sourceEventSeqs must not be empty except on assistant/message".to_string(),
            );
        }
        let mut non_earlier_source: Option<u64> = None;
        for source in raw {
            if non_earlier_source.is_none() && *source >= event.seq {
                non_earlier_source = Some(*source);
            }
            if !sources.insert(*source) {
                return Err("sourceEventSeqs must not contain duplicates".to_string());
            }
        }
        if let Some(non_earlier) = non_earlier_source {
            return Err(format!(
                "sourceEventSeqs must reference earlier events: {non_earlier} >= current seq {}",
                event.seq
            ));
        }
    }
    let missing: Vec<u64> = shadowed_seqs
        .iter()
        .copied()
        .filter(|seq| !sources.contains(seq))
        .collect();
    if !missing.is_empty() {
        let rendered: Vec<String> = missing.iter().map(u64::to_string).collect();
        return Err(format!(
            "surface replace: sourceEventSeqs must include every shadowed surface node; missing {}",
            rendered.join(", ")
        ));
    }
    Ok(())
}

/// Locate one replacement range without mutating the current fold state.
fn replacement_range(
    state: &SurfaceFoldState,
    op: &SurfaceOp,
) -> Result<(usize, usize, Vec<u64>), String> {
    let SurfaceOp::Replace { start, end } = op else {
        return Err("surface replace: expected a replace operation".to_string());
    };
    let start_idx = state
        .nodes
        .iter()
        .position(|node| node == start)
        .ok_or_else(|| format!("surface replace: start seq {start} not found in surface"))?;
    let end_idx = state
        .nodes
        .iter()
        .position(|node| node == end)
        .ok_or_else(|| format!("surface replace: end seq {end} not found in surface"))?;
    if start_idx > end_idx {
        return Err(format!(
            "surface replace: start seq {start} (index {start_idx}) is after end seq {end} (index {end_idx})"
        ));
    }
    let shadowed_seqs = state.nodes[start_idx..=end_idx].to_vec();
    Ok((start_idx, end_idx, shadowed_seqs))
}

/// Restrict a tool-result replacement to one current result's content
/// (TS `assertToolResultRewrite`).
fn assert_tool_result_rewrite(
    event: &SessionEvent,
    shadowed_seqs: &[u64],
    events: &[SessionEvent],
    base_seq: u64,
) -> Result<(), String> {
    if event.type_ != "tool/result" {
        return Ok(());
    }
    if shadowed_seqs.len() != 1 {
        return Err(
            "tool/result surface replacement must rewrite exactly one current node".to_string(),
        );
    }
    let original_seq = shadowed_seqs[0];
    if original_seq < base_seq {
        return Err(
            "tool/result surface replacement must target a current tool/result".to_string(),
        );
    }
    let Some(original) = events.get((original_seq - base_seq) as usize) else {
        return Err(
            "tool/result surface replacement must target a current tool/result".to_string(),
        );
    };
    if original.type_ != "tool/result" {
        return Err(
            "tool/result surface replacement must target a current tool/result".to_string(),
        );
    }
    let original_rest = with_nulled_result_content(&original.data);
    let replacement_rest = with_nulled_result_content(&event.data);
    if original_rest.is_none()
        || replacement_rest.is_none()
        || !crate::json::is_deep_equal_json(
            original_rest.as_ref().unwrap(),
            replacement_rest.as_ref().unwrap(),
        )
    {
        return Err("tool/result surface replacement may change only content".to_string());
    }
    Ok(())
}

/// Copy `data` with `message.content[0].content` set to `null`
/// (TS `assertToolResultRewrite`'s comparison projection).
fn with_nulled_result_content(data: &JsonValue) -> Option<JsonValue> {
    let mut copy = data.clone();
    let message = copy.as_object_mut()?.get_mut("message")?.as_object_mut()?;
    let first = message.get_mut("content")?.as_array_mut()?.first_mut()?;
    let block = first.as_object_mut()?;
    block.insert("content".to_string(), JsonValue::Null);
    Some(copy)
}

/// Validate one event at its replay boundary and prepare its atomic fold
/// transition.
fn plan_surface_event(
    state: &SurfaceFoldState,
    event: &SessionEvent,
    expected_seq: u64,
    events: &[SessionEvent],
    base_seq: u64,
) -> Result<Option<SurfacePlan>, String> {
    if event.seq != expected_seq {
        return Err(format!(
            "session event seq {} is not contiguous; expected {expected_seq}",
            event.seq
        ));
    }
    let Some(op) = surface_op_of(event)? else {
        return Ok(None);
    };
    match op {
        SurfaceOp::Append => {
            assert_provenance(event, &[])?;
            Ok(Some(SurfacePlan::Append { seq: event.seq }))
        }
        SurfaceOp::Replace { .. } => {
            let (start_idx, end_idx, shadowed_seqs) = replacement_range(state, &op)?;
            assert_provenance(event, &shadowed_seqs)?;
            assert_tool_result_rewrite(event, &shadowed_seqs, events, base_seq)?;
            let (start, end) = match &op {
                SurfaceOp::Replace { start, end } => (*start, *end),
                SurfaceOp::Append => unreachable!(),
            };
            Ok(Some(SurfacePlan::Replace(SurfaceReplacePlan {
                seq: event.seq,
                start,
                end,
                start_idx,
                end_idx,
                shadowed_seqs,
            })))
        }
    }
}

/// Commit one previously validated surface transition.
fn apply_surface_plan(
    state: &mut SurfaceFoldState,
    plan: Option<SurfacePlan>,
) -> Option<SurfaceFoldReplacement> {
    match plan {
        Some(SurfacePlan::Append { seq }) => {
            state.nodes.push(seq);
            None
        }
        Some(SurfacePlan::Replace(plan)) => {
            let SurfaceReplacePlan {
                seq,
                start,
                end,
                start_idx,
                end_idx,
                shadowed_seqs,
            } = plan;
            state
                .nodes
                .splice(start_idx..=end_idx, std::iter::once(seq));
            state.replace_generation += 1;
            Some(SurfaceFoldReplacement {
                seq,
                start,
                end,
                shadowed_seqs,
            })
        }
        None => None,
    }
}

/// Apply one event and return replacement metadata only when one occurred.
fn apply_surface_event(
    state: &mut SurfaceFoldState,
    event: &SessionEvent,
    expected_seq: u64,
    events: &[SessionEvent],
    base_seq: u64,
) -> Result<Option<SurfaceFoldReplacement>, String> {
    let plan = plan_surface_event(state, event, expected_seq, events, base_seq)?;
    Ok(apply_surface_plan(state, plan))
}

/// Replay a complete session log through the canonical surface fold
/// (TS `foldSurface`).
pub fn fold_surface(events: &[SessionEvent]) -> Result<SurfaceFoldResult, String> {
    let mut state = SurfaceFoldState::default();
    let mut replacements = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if let Some(replacement) = apply_surface_event(&mut state, event, index as u64, events, 0)?
        {
            replacements.push(replacement);
        }
    }
    Ok(SurfaceFoldResult {
        nodes: state.nodes,
        replacements,
    })
}

/// Incremental ordered surface view and append-boundary validator
/// (TS `SurfaceManager`).
#[derive(Debug, Default)]
pub struct SurfaceManager {
    state: SurfaceFoldState,
    /// Last processed absolute seq.
    last_processed_seq: Option<u64>,
    /// Absolute seq of the window's first event.
    base_seq: u64,
    /// Candidate already validated by `validate_next`, pending exact log
    /// admission.
    pending: Option<PendingPlan>,
}

#[derive(Debug, Clone, PartialEq)]
struct PendingPlan {
    event: SessionEvent,
    expected_seq: u64,
    plan: Option<SurfacePlan>,
}

impl SurfaceManager {
    /// Create a manager for a contiguous complete log or loaded event
    /// window starting at `base_seq`.
    pub fn new(base_seq: u64) -> Self {
        Self {
            state: SurfaceFoldState::default(),
            last_processed_seq: None,
            base_seq,
            pending: None,
        }
    }

    fn needs_process(&self, log: &[SessionEvent]) -> bool {
        if log.is_empty() {
            return false;
        }
        match self.last_processed_seq {
            Some(last) => last < self.base_seq + (log.len() as u64) - 1,
            None => true,
        }
    }

    /// Fold events appended since the previous access.
    fn process_delta(&mut self, log: &[SessionEvent]) -> Result<(), String> {
        if log.is_empty() {
            return Ok(());
        }
        let tail_seq = self.base_seq + (log.len() as u64) - 1;
        let mut seq = match self.last_processed_seq {
            Some(last) => last + 1,
            None => self.base_seq,
        };
        while seq <= tail_seq {
            let index = (seq - self.base_seq) as usize;
            let event = &log[index];
            let pending_matches = self
                .pending
                .as_ref()
                .is_some_and(|pending| &pending.event == event && pending.expected_seq == seq);
            if pending_matches {
                let pending = self.pending.take().unwrap();
                apply_surface_plan(&mut self.state, pending.plan);
            } else {
                apply_surface_event(&mut self.state, event, seq, log, self.base_seq)?;
                if let Some(pending) = &self.pending {
                    if pending.expected_seq <= seq {
                        self.pending = None;
                    }
                }
            }
            self.last_processed_seq = Some(seq);
            seq += 1;
        }
        Ok(())
    }

    /// Validate the next candidate without mutating the committed surface.
    pub fn validate_next(
        &mut self,
        log: &[SessionEvent],
        event: &SessionEvent,
    ) -> Result<(), String> {
        if self.needs_process(log) {
            self.process_delta(log)?;
        }
        let expected_seq = self.base_seq + log.len() as u64;
        let plan = plan_surface_event(&self.state, event, expected_seq, log, self.base_seq)?;
        self.pending = Some(PendingPlan {
            event: event.clone(),
            expected_seq,
            plan,
        });
        Ok(())
    }

    /// Monotonic count of folded positional replacements.
    pub fn replace_generation(&mut self, log: &[SessionEvent]) -> Result<u64, String> {
        if self.needs_process(log) {
            self.process_delta(log)?;
        }
        Ok(self.state.replace_generation)
    }

    /// Surface event sequences in model-visible order.
    pub fn nodes(&mut self, log: &[SessionEvent]) -> Result<Vec<u64>, String> {
        if self.needs_process(log) {
            self.process_delta(log)?;
        }
        Ok(self.state.nodes.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::JsonValue;

    fn event(type_: &str, seq: u64, data: JsonValue, op: Option<SurfaceOp>) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: 0,
            data,
            ignorable: None,
            surface_op: op,
            source_event_seqs: None,
        }
    }

    fn user_message_data(id: &str) -> JsonValue {
        serde_json::json!({
            "id": id,
            "role": "user",
            "content": [{"type": "text", "text": id}],
            "source": {"kind": "user"},
        })
    }

    #[test]
    fn eligibility_and_guards() {
        assert!(is_surface_eligible_type("user/message"));
        assert!(is_surface_eligible_type("assistant/message"));
        assert!(is_surface_eligible_type("tool/result"));
        assert!(!is_surface_eligible_type("turn/start"));

        let surface = event(
            "user/message",
            0,
            user_message_data("a"),
            Some(SurfaceOp::Append),
        );
        assert!(is_surface_event(&surface));
        assert!(is_append_surface_event(&surface));
        assert!(!is_replacement_surface_event(&surface));

        let replace = event(
            "user/message",
            1,
            user_message_data("b"),
            Some(SurfaceOp::Replace { start: 0, end: 0 }),
        );
        assert!(is_replacement_surface_event(&replace));

        let log_only = event("turn/start", 0, serde_json::json!({"turn": 1}), None);
        assert!(!is_surface_event(&log_only));
    }

    #[test]
    fn derive_event_message_projects_messages() {
        let user = event(
            "user/message",
            0,
            user_message_data("a"),
            Some(SurfaceOp::Append),
        );
        let message = derive_event_message(&user).unwrap();
        assert_eq!(message.role, dsh_llm::Role::User);

        let assistant = event(
            "assistant/message",
            1,
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {
                    "id": "m1", "role": "assistant",
                    "content": [{"type": "text", "text": "hi"}],
                    "source": {"kind": "model", "provider": "p", "model": "m"},
                },
                "usage": {"inputTokens": 1, "outputTokens": 1},
            }),
            Some(SurfaceOp::Append),
        );
        assert!(derive_event_message(&assistant).is_some());

        // Empty-content assistant message derives to null.
        let empty = event(
            "assistant/message",
            1,
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {
                    "id": "m2", "role": "assistant",
                    "content": [],
                    "source": {"kind": "model", "provider": "p", "model": "m"},
                },
            }),
            Some(SurfaceOp::Append),
        );
        assert!(derive_event_message(&empty).is_none());

        let boundary = event("turn/start", 0, serde_json::json!({"turn": 1}), None);
        assert!(derive_event_message(&boundary).is_none());
    }

    #[test]
    fn fold_append_only() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1}), None),
            event(
                "user/message",
                1,
                user_message_data("a"),
                Some(SurfaceOp::Append),
            ),
            event(
                "assistant/message",
                2,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {"id": "m1", "role": "assistant", "content": [{"type": "text", "text": "hi"}],
                        "source": {"kind": "model", "provider": "p", "model": "m"}},
                }),
                Some(SurfaceOp::Append),
            ),
            event(
                "turn/end",
                3,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
                None,
            ),
        ];
        let result = fold_surface(&events).unwrap();
        assert_eq!(result.nodes, vec![1, 2]);
        assert!(result.replacements.is_empty());
    }

    #[test]
    fn fold_replacement_shadows_range() {
        let mut events = Vec::new();
        for (index, id) in ["a", "b", "c"].iter().enumerate() {
            events.push(event(
                "user/message",
                index as u64,
                user_message_data(id),
                Some(SurfaceOp::Append),
            ));
        }
        let replacement = SessionEvent {
            type_: "user/message".to_string(),
            seq: 3,
            time: 0,
            data: user_message_data("summary"),
            ignorable: None,
            surface_op: Some(SurfaceOp::Replace { start: 0, end: 1 }),
            source_event_seqs: Some(vec![0, 1]),
        };
        events.push(replacement);
        let result = fold_surface(&events).unwrap();
        assert_eq!(result.nodes, vec![3, 2]);
        assert_eq!(result.replacements.len(), 1);
        assert_eq!(result.replacements[0].shadowed_seqs, vec![0, 1]);
    }

    #[test]
    fn fold_rejects_missing_marker_and_provenance() {
        let events = vec![event("user/message", 0, user_message_data("a"), None)];
        assert!(
            fold_surface(&events)
                .unwrap_err()
                .contains("requires a surfaceOp marker")
        );

        let events = vec![
            event(
                "user/message",
                0,
                user_message_data("a"),
                Some(SurfaceOp::Append),
            ),
            SessionEvent {
                type_: "user/message".to_string(),
                seq: 1,
                time: 0,
                data: user_message_data("summary"),
                ignorable: None,
                surface_op: Some(SurfaceOp::Replace { start: 0, end: 0 }),
                source_event_seqs: None,
            },
        ];
        assert!(
            fold_surface(&events)
                .unwrap_err()
                .contains("must include every shadowed surface node")
        );
    }

    #[test]
    fn fold_rejects_marker_on_log_only_event() {
        let events = vec![event(
            "turn/start",
            0,
            serde_json::json!({"turn": 1}),
            Some(SurfaceOp::Append),
        )];
        assert!(
            fold_surface(&events)
                .unwrap_err()
                .contains("not surface-eligible")
        );
    }

    #[test]
    fn manager_tracks_incremental_fold() {
        let log: Vec<SessionEvent> = Vec::new();
        let mut manager = SurfaceManager::new(0);
        assert_eq!(manager.nodes(&log).unwrap(), Vec::<u64>::new());

        // simulate session flow: validate candidate, then admit into log
        let mut log = log;
        let candidate = event(
            "user/message",
            0,
            user_message_data("a"),
            Some(SurfaceOp::Append),
        );
        manager.validate_next(&log, &candidate).unwrap();
        log.push(candidate);
        assert_eq!(manager.nodes(&log).unwrap(), vec![0]);
        assert_eq!(manager.replace_generation(&log).unwrap(), 0);
    }

    #[test]
    fn manager_rejects_invalid_candidate_without_mutation() {
        let mut log = vec![event(
            "user/message",
            0,
            user_message_data("a"),
            Some(SurfaceOp::Append),
        )];
        let mut manager = SurfaceManager::new(0);
        assert_eq!(manager.nodes(&log).unwrap(), vec![0]);

        // candidate without surface marker: validation fails, surface intact
        let bad = event("user/message", 1, user_message_data("b"), None);
        assert!(manager.validate_next(&log, &bad).is_err());
        assert_eq!(manager.nodes(&log).unwrap(), vec![0]);

        // candidate with seq gap: validation fails
        let gap = event(
            "user/message",
            5,
            user_message_data("c"),
            Some(SurfaceOp::Append),
        );
        assert!(manager.validate_next(&log, &gap).is_err());
        assert_eq!(manager.nodes(&log).unwrap(), vec![0]);

        // valid candidate admitted
        let good = event(
            "user/message",
            1,
            user_message_data("d"),
            Some(SurfaceOp::Append),
        );
        manager.validate_next(&log, &good).unwrap();
        log.push(good);
        assert_eq!(manager.nodes(&log).unwrap(), vec![0, 1]);
    }
}
