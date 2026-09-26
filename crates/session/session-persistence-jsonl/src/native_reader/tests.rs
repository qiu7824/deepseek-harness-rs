use super::*;
use crate::JsonlCompression;
use dsh_session::{SessionSeq, SurfaceOp};
use dsh_session_persistence::{SessionReadWindowRequest, select_history_window};
use serde_json::json;
use std::io::Write;

fn event(seq: u64, kind: &str) -> SessionEvent {
    SessionEvent {
        type_: kind.into(),
        seq: SessionSeq::new(seq).unwrap(),
        time: seq as i64,
        data: json!({ "turn": 1 }),
        ignorable: None,
        surface_op: matches!(kind, "user/message" | "assistant/message")
            .then_some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}

fn assert_same_selection(events: &[SessionEvent]) {
    for before in (0..=events.len() as u64).map(Some).chain([None]) {
        let projected = events
            .iter()
            .filter(|event| before.is_none_or(|before| event.seq.get() < before))
            .map(HistoryBoundary::new)
            .collect::<Vec<_>>();
        for messages in 0..=4 {
            for max_events in [0, 1, 3, events.len()] {
                assert_eq!(
                    select_history_window(events, before, messages, max_events),
                    select_history_window(&projected, before, messages, max_events),
                    "before={before:?}, messages={messages}, max_events={max_events}",
                );
            }
        }
    }
}

#[test]
fn compact_boundaries_preserve_message_provenance_and_replacement_selection() {
    let mut events = vec![event(0, "user/message")];
    for seq in 1..=7 {
        events.push(event(seq, "assistant/chunk"));
    }
    let mut message = event(8, "assistant/message");
    message.source_event_seqs = Some(vec![7, 1, 5]);
    events.push(message);
    let mut replacement = event(9, "assistant/message");
    replacement.surface_op = Some(SurfaceOp::Replace { start: 8, end: 8 });
    replacement.source_event_seqs = Some(vec![8]);
    events.push(replacement);
    let mut user = event(10, "user/message");
    user.surface_op = None;
    events.push(user);
    assert_same_selection(&events);
    events.truncate(9);
    assert_same_selection(&events);
}

#[test]
fn compact_boundaries_preserve_failed_streams_and_reopened_turns() {
    let mut events = (0..8)
        .map(|seq| event(seq, "assistant/chunk"))
        .collect::<Vec<_>>();
    let mut end = event(8, "turn/end");
    end.data = json!({ "turn": 1, "reason": { "kind": "error" } });
    events.push(end);
    events.push(event(9, "session/end-seed"));
    events.push(event(10, "model/selection"));
    assert_same_selection(&events);
    events.push(event(11, "turn/start"));
    assert_same_selection(&events);
    let mut end = event(12, "turn/end");
    end.data = json!({ "turn": 2, "reason": { "kind": "error" } });
    events.push(end);
    assert_same_selection(&events);
}

#[test]
fn compact_boundaries_have_no_payload_or_provenance_sized_allocation() {
    assert!(std::mem::size_of::<HistoryBoundary>() <= 40);
    let mut message = event(50_000, "assistant/message");
    message.data = json!({ "text": "x".repeat(1024 * 1024) });
    message.source_event_seqs = Some((0..50_000).collect());
    let boundary = HistoryBoundary::new(&message);
    assert_eq!(boundary.source, 0);
    assert!(boundary.has_source);
    assert!(!std::mem::needs_drop::<HistoryBoundary>());
}

#[test]
fn physical_references_preserve_all_logical_sources_and_metadata() {
    for references in [json!([[0, 3], 5, 6, 9]), json!([7, 1, 2, 3]), json!([])] {
        let row = json!({"seq": 10, "time": 0, "type": "assistant/message", "data": {"turn": 2}, "surfaceOp": "append", "sourceEventSeqs": references});
        let boundary = HistoryBoundary::from_physical(&row);
        let event = logical_event(row).unwrap();
        let expected = HistoryBoundary::new(&event);
        assert_eq!(boundary.seq, expected.seq);
        assert_eq!(boundary.source, expected.source);
        assert_eq!(boundary.has_source, expected.has_source);
        assert_eq!(boundary.turn, expected.turn);
        assert_eq!(boundary.surface, expected.surface);
        if event.source_event_seqs.as_ref().unwrap().len() == 7 {
            assert_eq!(event.source_event_seqs, Some(vec![0, 1, 2, 3, 5, 6, 9]));
        }
    }
}

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn fixture(events: &[SessionEvent], compression: JsonlCompression) -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "native-history-{}{}",
        uuid::Uuid::new_v4(),
        crate::format::log_suffix(compression),
    ));
    let header = format!(
        "{}\n",
        json!({
            "type": "session", "version": 4, "id": "fixture", "createdAt": 0,
            "isSeeded": false, "delegationDepth": 0,
        })
    );
    let mut file = File::create(&path).unwrap();
    let write_events = |writer: &mut dyn Write| {
        for event in events {
            serde_json::to_writer(&mut *writer, event).unwrap();
            writer.write_all(b"\n").unwrap();
        }
    };
    match compression {
        JsonlCompression::None => {
            file.write_all(header.as_bytes()).unwrap();
            write_events(&mut file);
        }
        JsonlCompression::Zstd => {
            file.write_all(&crate::compress_zstd_frame(header.as_bytes()).unwrap())
                .unwrap();
            let mut encoder = zstd::stream::Encoder::new(&mut file, 0).unwrap();
            write_events(&mut encoder);
            encoder.finish().unwrap();
        }
    }
    Fixture(path)
}

#[test]
fn native_window_reads_only_the_selected_payload_and_pages_back_to_its_sources() {
    let mut events = vec![event(0, "user/message")];
    for seq in 1..=8 {
        let mut chunk = event(seq, "assistant/chunk");
        chunk.data = json!({ "turn": 1, "text": "x".repeat(128 * 1024) });
        events.push(chunk);
    }
    let mut message = event(9, "assistant/message");
    message.source_event_seqs = Some((1..=8).collect());
    events.push(message);
    events.push(event(10, "user/message"));
    events.push(event(11, "assistant/message"));
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let fixture = fixture(&events, compression);
        let tail = window(
            &fixture.0,
            "fixture",
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 2,
                max_events: 65_536,
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(tail.events, events[10..]);
        assert!(tail.has_more);
        assert_eq!(tail.oversized_event_count, None);
        let previous = window(
            &fixture.0,
            "fixture",
            SessionReadWindowRequest {
                before_seq: Some(10),
                max_messages: 1,
                max_events: 65_536,
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(previous.events, events[1..10]);
        assert!(previous.has_more);
    }
}

#[test]
fn native_window_preserves_oversized_tail_and_refuses_an_unbounded_open_stream() {
    let mut events = vec![event(0, "user/message")];
    events.extend((1..=12).map(|seq| event(seq, "assistant/chunk")));
    let fixture = fixture(&events, JsonlCompression::None);
    let oversized = window(
        &fixture.0,
        "fixture",
        SessionReadWindowRequest {
            before_seq: None,
            max_messages: 1,
            max_events: 4,
        },
        &|| false,
    )
    .unwrap();
    assert!(oversized.events.is_empty());
    assert!(oversized.has_more);
    assert_eq!(oversized.oversized_event_count, Some(5));
    let mut message = event(13, "assistant/message");
    message.source_event_seqs = Some((1..=12).collect());
    events.push(message);
    let fixture = self::fixture(&events, JsonlCompression::None);
    let tail = window(
        &fixture.0,
        "fixture",
        SessionReadWindowRequest {
            before_seq: None,
            max_messages: 1,
            max_events: 4,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(tail.events, events[10..]);
    assert!(tail.has_more);
    assert_eq!(tail.oversized_event_count, None);
    let previous = window(
        &fixture.0,
        "fixture",
        SessionReadWindowRequest {
            before_seq: Some(10),
            max_messages: 1,
            max_events: 4,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(previous.events, events[6..10]);
    assert!(previous.has_more);
}

struct CheckedSink {
    expected: Vec<SessionEvent>,
    inspected: usize,
    output: Vec<SessionEvent>,
}

impl dsh_session_persistence::HistoryWindowSink for CheckedSink {
    fn inspect(&mut self, event: &SessionEvent) -> Result<(), String> {
        assert!(self.output.is_empty());
        assert_eq!(event, &self.expected[self.inspected]);
        self.inspected += 1;
        Ok(())
    }
    fn push(&mut self, event: SessionEvent) -> Result<(), String> {
        assert_eq!(self.inspected, self.expected.len());
        assert_eq!(event, self.expected[self.output.len()]);
        self.output.push(event);
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<(Vec<SessionEvent>, bool), String> {
        assert_eq!(self.output, self.expected);
        Ok((self.output, false))
    }
}

fn checked_sink(events: &[SessionEvent]) -> Box<dyn dsh_session_persistence::HistoryWindowSink> {
    Box::new(CheckedSink {
        expected: events.to_vec(),
        inspected: 0,
        output: Vec::new(),
    })
}

#[test]
fn projected_native_windows_inspect_then_consume_the_unchanged_source_range() {
    let mut events = vec![event(0, "user/message")];
    events.extend((1..=8).map(|seq| event(seq, "assistant/chunk")));
    let mut message = event(9, "assistant/message");
    message.source_event_seqs = Some((1..=8).collect());
    events.extend([
        message,
        event(10, "user/message"),
        event(11, "assistant/message"),
    ]);
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let fixture = fixture(&events, compression);
        let request = SessionReadWindowRequest {
            before_seq: Some(10),
            max_messages: 1,
            max_events: 65_536,
        };
        let raw = window(&fixture.0, "fixture", request, &|| false).unwrap();
        let projected = window_with_sink(
            &fixture.0,
            "fixture",
            request,
            &|| false,
            Some(checked_sink(&raw.events)),
        )
        .unwrap();
        assert_eq!(projected, raw);
        let request = dsh_session_persistence::SessionReadForwardWindowRequest {
            after_seq: 2,
            max_messages: 2,
            max_events: 65_536,
        };
        let forward = forward_window_with_sink(
            &fixture.0,
            "fixture",
            request,
            &|| false,
            Some(checked_sink(&events[2..=10])),
        )
        .unwrap();
        assert_eq!(forward.events, events[2..=10]);
        assert!(forward.has_more);
        assert_eq!(
            dsh_session_persistence::index::project_history_window(
                raw.clone(),
                checked_sink(&raw.events)
            )
            .unwrap(),
            raw
        );
    }
}

#[test]
fn projected_native_window_rejects_append_between_inspection_and_consumption() {
    struct AppendingSink(std::path::PathBuf);
    impl dsh_session_persistence::HistoryWindowSink for AppendingSink {
        fn inspect(&mut self, _: &SessionEvent) -> Result<(), String> {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.0)
                .unwrap();
            serde_json::to_writer(&mut file, &event(1, "user/message")).unwrap();
            file.write_all(b"\n").unwrap();
            Ok(())
        }
        fn push(&mut self, _: SessionEvent) -> Result<(), String> {
            panic!("a changed source reached consumption")
        }
        fn finish(self: Box<Self>) -> Result<(Vec<SessionEvent>, bool), String> {
            panic!("a changed source published output")
        }
    }
    let fixture = fixture(&[event(0, "user/message")], JsonlCompression::None);
    let error = window_with_sink(
        &fixture.0,
        "fixture",
        SessionReadWindowRequest {
            before_seq: None,
            max_messages: 1,
            max_events: 8,
        },
        &|| false,
        Some(Box::new(AppendingSink(fixture.0.clone()))),
    )
    .unwrap_err();
    assert!(
        error.contains("changed between native history scans"),
        "{error}"
    );
}
