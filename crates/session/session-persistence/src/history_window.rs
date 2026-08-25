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
