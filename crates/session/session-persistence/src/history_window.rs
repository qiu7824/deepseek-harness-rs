//! Allocation-free history window selection.
//!
//! This module computes safe message-aligned event bounds. It deliberately
//! does not clone events; persistence backends use the resulting seq/index
//! range to perform bounded reads.

use dsh_session::SessionEvent;

/// A contiguous, message-aligned event window inside the supplied slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryWindowSelection {
    pub start: usize,
    pub end: usize,
    pub has_more: bool,
}

impl HistoryWindowSelection {
    pub fn event_count(self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

fn is_closed_failed_stream_tail(events: &[SessionEvent]) -> bool {
    let Some(last) = events.last() else {
        return false;
    };
    let failed_turn = last.data.get("turn").and_then(serde_json::Value::as_u64);
    last.type_ == "turn/end"
        && last
            .data
            .get("reason")
            .and_then(|reason| reason.get("kind"))
            .and_then(serde_json::Value::as_str)
            == Some("error")
        && failed_turn.is_some()
        && events.iter().any(|event| {
            event.type_ == "assistant/chunk"
                && event.data.get("turn").and_then(serde_json::Value::as_u64) == failed_turn
        })
}

/// A safe message window exists, but materializing it would exceed the
/// configured event budget. Callers must not silently cut through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryWindowTooLarge {
    pub selection: HistoryWindowSelection,
    pub max_events: usize,
}

/// Select one backwards page without allocating or cloning events.
pub fn select_history_window(
    events: &[SessionEvent],
    before_seq: Option<u64>,
    max_messages: u64,
    max_events: usize,
) -> Result<HistoryWindowSelection, HistoryWindowTooLarge> {
    let end = before_seq
        .map(|before| events.partition_point(|event| event.seq < before))
        .unwrap_or(events.len());
    let mut count = 0_u64;
    let mut cut_seq = 0_u64;
    for event in events[..end].iter().rev() {
        if !matches!(event.type_.as_str(), "user/message" | "assistant/message")
            || !event.surface_op.as_ref().is_none_or(|op| op.is_append())
        {
            continue;
        }
        count += 1;
        let group_start = event
            .source_event_seqs
            .as_ref()
            .and_then(|sources| sources.iter().copied().min())
            .unwrap_or(event.seq)
            .min(event.seq);
        if count >= max_messages {
            cut_seq = group_start;
            break;
        }
    }
    let start = events[..end].partition_point(|event| event.seq < cut_seq);
    let selection = HistoryWindowSelection {
        start,
        end,
        has_more: cut_seq > 0,
    };
    if selection.event_count() > max_events {
        // A streamed assistant message cites every chunk that produced its
        // final, self-contained message. When that single provenance group is
        // larger than the page budget, retain its bounded contiguous tail and
        // the final message instead of making the whole history unreadable.
        // The omitted prefix remains reachable through `has_more`.
        if max_messages <= 1
            && max_events > 0
            && (events[..end].last().is_some_and(|event| {
                event.type_ == "assistant/message"
                    && event.surface_op.as_ref().is_some_and(|op| op.is_append())
                    && event
                        .source_event_seqs
                        .as_ref()
                        .is_some_and(|seqs| !seqs.is_empty())
            }) || is_closed_failed_stream_tail(&events[..end]))
        {
            return Ok(HistoryWindowSelection {
                start: end.saturating_sub(max_events),
                end,
                has_more: end > max_events,
            });
        }
        if max_messages <= 1
            && max_events > 0
            && before_seq.is_some()
            && !events[end.saturating_sub(max_events)..end]
                .iter()
                .any(|event| {
                    matches!(event.type_.as_str(), "user/message" | "assistant/message")
                        && event.surface_op.as_ref().is_none_or(|op| op.is_append())
                })
        {
            return Ok(HistoryWindowSelection {
                start: end.saturating_sub(max_events),
                end,
                has_more: end > max_events,
            });
        }
        return Err(HistoryWindowTooLarge {
            selection,
            max_events,
        });
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, type_: &str, append_surface: bool) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: seq as i64,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: append_surface.then_some(dsh_session::SurfaceOp::Append),
            source_event_seqs: None,
        }
    }

    #[test]
    fn rejects_sparse_message_window_above_event_budget_without_cutting_it() {
        let mut events = Vec::with_capacity(2_002);
        events.push(event(0, "user/message", true));
        for seq in 1..=2_000 {
            events.push(event(seq, "assistant/chunk", false));
        }
        events.push(event(2_001, "assistant/message", true));

        let error = select_history_window(&events, None, 2, 512).unwrap_err();

        assert_eq!(error.selection.start, 0);
        assert_eq!(error.selection.end, 2_002);
        assert_eq!(error.selection.event_count(), 2_002);
        assert_eq!(error.max_events, 512);
    }

    #[test]
    fn bounds_one_oversized_streaming_message_to_its_renderable_tail() {
        let mut events = Vec::with_capacity(4_097);
        for seq in 0..4_096 {
            events.push(event(seq, "assistant/chunk", false));
        }
        let mut message = event(4_096, "assistant/message", true);
        message.source_event_seqs = Some((0..4_096).collect());
        events.push(message);

        let selection = select_history_window(&events, None, 1, 4_096).unwrap();

        assert_eq!(selection.start, 1);
        assert_eq!(selection.end, 4_097);
        assert_eq!(selection.event_count(), 4_096);
        assert!(selection.has_more);
        assert_eq!(events[selection.end - 1].type_, "assistant/message");
    }

    #[test]
    fn bounds_one_oversized_failed_turn_without_final_message() {
        let mut events = Vec::with_capacity(4_099);
        for seq in 0..4_096 {
            let mut chunk = event(seq, "assistant/chunk", false);
            chunk.data = serde_json::json!({"turn": 1, "step": 1});
            events.push(chunk);
        }
        let mut chunk = event(4_096, "assistant/chunk", false);
        chunk.data = serde_json::json!({"turn": 1, "step": 1});
        events.push(chunk);
        events.push(event(4_097, "step/end", false));
        let mut turn_end = event(4_098, "turn/end", false);
        turn_end.data = serde_json::json!({
            "turn": 1,
            "reason": {"kind": "error", "error": {"code": "TRANSPORT"}}
        });
        events.push(turn_end);

        let selection = select_history_window(&events, None, 1, 4_096).unwrap();

        assert_eq!(selection.start, 3);
        assert_eq!(selection.end, 4_099);
        assert_eq!(selection.event_count(), 4_096);
        assert!(selection.has_more);
        assert_eq!(events[selection.end - 1].type_, "turn/end");
    }

    #[test]
    fn pages_backwards_inside_one_oversized_stream() {
        let mut events = Vec::with_capacity(10_002);
        events.push(event(0, "user/message", true));
        for seq in 1..=10_000 {
            events.push(event(seq, "assistant/chunk", false));
        }
        let mut message = event(10_001, "assistant/message", true);
        message.source_event_seqs = Some((1..=10_000).collect());
        events.push(message);

        let tail = select_history_window(&events, None, 1, 4_096).unwrap();
        assert_eq!(tail.start, 5_906);
        assert!(tail.has_more);

        let previous = select_history_window(&events, Some(5_906), 1, 4_096).unwrap();
        assert_eq!(previous.start, 1_810);
        assert_eq!(previous.end, 5_906);
        assert_eq!(previous.event_count(), 4_096);
        assert!(previous.has_more);

        let oldest = select_history_window(&events, Some(1_810), 1, 4_096).unwrap();
        assert_eq!(oldest.start, 0);
        assert_eq!(oldest.end, 1_810);
        assert!(!oldest.has_more);
    }

    #[test]
    fn selects_normal_message_aligned_window_by_index_without_cloning() {
        let events = vec![
            event(0, "user/message", true),
            event(1, "assistant/chunk", false),
            event(2, "assistant/message", true),
            event(3, "user/message", true),
            event(4, "assistant/message", true),
        ];

        let selection = select_history_window(&events, Some(5), 2, 10).unwrap();

        assert_eq!(selection.start, 3);
        assert_eq!(selection.end, 5);
        assert_eq!(selection.event_count(), 2);
        assert!(selection.has_more);
    }
}
