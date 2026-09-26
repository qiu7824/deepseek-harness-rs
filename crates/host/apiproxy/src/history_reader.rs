//! Bounded history pagination over resident and archived Session events.

use std::collections::VecDeque;
use std::sync::Arc;

use dsh_session::{SessionEvent, SessionEventReader};
use dsh_session_persistence::HistoryWindowSink;

use super::{
    HISTORY_SCAN_EVENT_LIMIT, HISTORY_SOURCE_BYTE_LIMIT, HISTORY_TRANSPORT_BYTE_LIMIT,
    HISTORY_TRANSPORT_EVENT_LIMIT,
};
use crate::history_transport::{Direction, TransportSink};

fn is_message(event: &SessionEvent) -> bool {
    matches!(event.type_.as_str(), "user/message" | "assistant/message")
        && event.surface_op.as_ref().is_none_or(|op| op.is_append())
}

fn compact(
    reader: &SessionEventReader<'_>,
    first: u64,
    end: u64,
    direction: Direction,
    cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<Option<(Vec<SessionEvent>, bool)>, String> {
    let mut sink = Box::new(
        TransportSink::new(
            direction,
            HISTORY_TRANSPORT_EVENT_LIMIT,
            HISTORY_TRANSPORT_BYTE_LIMIT,
            HISTORY_SOURCE_BYTE_LIMIT,
        )
        .with_cleanup(cleanup),
    );
    let mut oversized = false;
    reader.visit(first, Some(end), |event| {
        if sink.inspect(event).is_err() {
            oversized = true;
            return Ok(false);
        }
        Ok(true)
    })?;
    if oversized {
        return Ok(None);
    }
    reader.visit(first, Some(end), |event| {
        sink.push(crate::public_event::clone_for_browser(event))?;
        Ok(true)
    })?;
    sink.finish().map(Some)
}

pub(super) fn forward(
    reader: &SessionEventReader<'_>,
    after_seq: i64,
    max_messages: u64,
    cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<(Vec<SessionEvent>, bool), String> {
    if let Some(cleanup) = &cleanup {
        cleanup();
    }
    let first = u64::try_from(after_seq).unwrap_or(0).min(reader.len());
    let bound = first
        .saturating_add(HISTORY_SCAN_EVENT_LIMIT as u64)
        .min(reader.len());
    let mut end = first;
    let mut messages = 0_u64;
    reader.visit(first, Some(bound), |event| {
        end = event.seq.get() + 1;
        messages += u64::from(is_message(event));
        Ok(messages < max_messages.max(1))
    })?;
    let (page, reduced) = compact(reader, first, end, Direction::Forward, cleanup)?
        .ok_or("one safe history group exceeds the 64 MiB source budget")?;
    Ok((page, end < reader.len() || reduced))
}

pub(super) fn backward(
    reader: &SessionEventReader<'_>,
    before_seq: Option<i64>,
    max_messages: u64,
    cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<(Vec<SessionEvent>, bool), String> {
    if let Some(cleanup) = &cleanup {
        cleanup();
    }
    let before_seq = before_seq.and_then(|value| u64::try_from(value).ok());
    let end = before_seq.unwrap_or(reader.len()).min(reader.len());
    let capacity = max_messages.max(1).min(HISTORY_SCAN_EVENT_LIMIT as u64 + 1) as usize;
    let mut starts = VecDeque::new();
    let mut message_count = 0_u64;
    let mut last_message = None;
    let mut completed_tail = false;
    let mut failed_turn = None;
    reader.visit(0, Some(end), |event| {
        if is_message(event) {
            message_count += 1;
            last_message = Some(event.seq.get());
            if starts.len() == capacity {
                starts.pop_front();
            }
            starts.push_back(
                event
                    .source_event_seqs
                    .as_ref()
                    .and_then(|sources| sources.iter().copied().min())
                    .unwrap_or(event.seq.get())
                    .min(event.seq.get()),
            );
        }
        completed_tail = event.type_ == "assistant/message"
            && event.surface_op.as_ref().is_some_and(|op| op.is_append())
            && event
                .source_event_seqs
                .as_ref()
                .is_some_and(|sources| !sources.is_empty());
        match event.type_.as_str() {
            "turn/end" => {
                failed_turn = (event
                    .data
                    .pointer("/reason/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("error"))
                .then(|| event.data.get("turn").and_then(serde_json::Value::as_u64))
                .flatten();
            }
            "turn/start" | "step/start" | "assistant/chunk" | "assistant/message"
            | "user/message" | "tool/call" | "tool/result" => failed_turn = None,
            _ => {}
        }
        Ok(true)
    })?;
    let mut failed_stream_tail = None;
    let mut messages = max_messages.max(1);
    loop {
        // More messages than the retained boundary ring necessarily exceed
        // the raw event budget; preserve the existing halving retry policy.
        if messages <= message_count && messages > starts.len() as u64 {
            messages = (messages / 2).max(1);
            continue;
        }
        let cut = if messages > message_count {
            0
        } else {
            starts[starts.len() - messages as usize]
        };
        let mut first = cut;
        let mut has_more = cut > 0;
        if end - first > HISTORY_SCAN_EVENT_LIMIT as u64 {
            if messages > 1 {
                messages = (messages / 2).max(1);
                continue;
            }
            let failed = if let Some(failed) = failed_stream_tail {
                failed
            } else {
                let mut found = false;
                if let Some(turn) = failed_turn {
                    reader.visit(0, Some(end), |event| {
                        found = event.type_ == "assistant/chunk"
                            && event.data.get("turn").and_then(serde_json::Value::as_u64)
                                == Some(turn);
                        Ok(!found)
                    })?;
                }
                failed_stream_tail = Some(found);
                found
            };
            let tail = end.saturating_sub(HISTORY_SCAN_EVENT_LIMIT as u64);
            if completed_tail
                || failed
                || (before_seq.is_some() && last_message.is_none_or(|seq| seq < tail))
            {
                first = tail;
                has_more = end > HISTORY_SCAN_EVENT_LIMIT as u64;
            } else {
                return Err(format!(
                    "one safe history group requires {} events, above the {} event scan budget",
                    end - first,
                    HISTORY_SCAN_EVENT_LIMIT
                ));
            }
        }
        if let Some((page, reduced)) =
            compact(reader, first, end, Direction::Backward, cleanup.clone())?
        {
            return Ok((page, has_more || reduced));
        }
        if messages > 1 {
            messages = (messages / 2).max(1);
        } else {
            return Err("one safe history group exceeds the 64 MiB source budget".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{Session, SessionLogOffset, SessionSeq, SurfaceOp};
    use serde_json::json;

    fn event(
        seq: u64,
        kind: &str,
        data: serde_json::Value,
        sources: Option<Vec<u64>>,
    ) -> SessionEvent {
        SessionEvent {
            type_: kind.into(),
            seq: SessionSeq::new(seq).unwrap(),
            time: seq as i64,
            data,
            ignorable: None,
            surface_op: matches!(kind, "user/message" | "assistant/message")
                .then_some(SurfaceOp::Append),
            source_event_seqs: sources,
        }
    }

    fn session(seed: &[SessionEvent], archived: bool) -> Session {
        let header =
            dsh_session::snapshot_session_header(&dsh_session::session_id("history-reader"), None)
                .unwrap();
        if archived {
            let mut archive =
                dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir())
                    .unwrap();
            for event in seed {
                archive.push(event).unwrap();
            }
            Session::from_event_archive(
                header.id.clone(),
                archive.finish().unwrap(),
                &header,
                SessionLogOffset::ZERO,
                vec![],
            )
            .unwrap()
        } else {
            Session::from_restore(
                header.id.clone(),
                seed.to_vec(),
                &header,
                SessionLogOffset::ZERO,
            )
            .unwrap()
        }
    }

    #[test]
    fn indexed_history_matches_slice_pages_for_resident_and_archived_sessions() {
        let mut seed = Vec::new();
        for turn in 0..28 {
            let seq = seed.len() as u64;
            seed.push(event(seq, "user/message", json!({"id":format!("user-{turn}"), "role":"user", "source":{"kind":"user"}, "content":[{"type":"text", "text":"prompt"}]}), None));
            let first_chunk = seed.len() as u64;
            for _ in 0..3 {
                seed.push(event(seed.len() as u64, "assistant/chunk", json!({"turn":turn,"step":1,"chunk":{"type":"text-delta","index":0,"text":"answer"}}), None));
            }
            let seq = seed.len() as u64;
            seed.push(event(seq, "assistant/message", json!({"turn":turn,"step":1,"message":{"id":format!("assistant-{turn}"),"role":"assistant","source":{"kind":"model","provider":"fixture","model":"fixture"},"content":[{"type":"text","text":"answeransweranswer"}]}}), Some((first_chunk..seq).collect())));
            seed.push(event(
                seed.len() as u64,
                "turn/end",
                json!({"turn":turn,"reason":{"kind":"stop"}}),
                None,
            ));
        }
        for archived in [false, true] {
            let session = session(&seed, archived);
            let events = session.events();
            for messages in [1, 12, 80, 100, u64::MAX] {
                for before in [
                    None,
                    Some(-1),
                    Some(0),
                    Some(47),
                    Some(events.len() as i64 + 1),
                ] {
                    let expected =
                        super::super::ApiProxyService::paginate(&events, before, messages);
                    let actual = session
                        .with_event_reader(|reader| backward(reader, before, messages, None));
                    assert_eq!(
                        actual, expected,
                        "backward archived={archived}, before={before:?}, messages={messages}"
                    );
                }
                for after in [-1, 0, 1, 47, events.len() as i64 + 1] {
                    let expected =
                        super::super::ApiProxyService::paginate_forward(&events, after, messages);
                    let actual =
                        session.with_event_reader(|reader| forward(reader, after, messages, None));
                    assert_eq!(
                        actual, expected,
                        "forward archived={archived}, after={after}, messages={messages}"
                    );
                }
            }
        }
    }

    #[test]
    fn indexed_history_preserves_long_failed_stream_and_scan_limit_boundaries() {
        let mut seed: Vec<_> = (0..HISTORY_SCAN_EVENT_LIMIT as u64 + 5)
            .map(|seq| {
                event(
                    seq,
                    "assistant/chunk",
                    json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"x"}}),
                    None,
                )
            })
            .collect();
        seed.push(event(
            seed.len() as u64,
            "turn/end",
            json!({"turn":1,"reason":{"kind":"error"}}),
            None,
        ));
        let session = session(&seed, true);
        let events = session.events();
        for messages in [1, 12, 100] {
            for before in [None, Some(65_537), Some(events.len() as i64)] {
                assert_eq!(
                    session.with_event_reader(|reader| backward(reader, before, messages, None)),
                    super::super::ApiProxyService::paginate(&events, before, messages)
                );
            }
        }
        for after in [0, 65_536] {
            assert_eq!(
                session.with_event_reader(|reader| forward(reader, after, 100, None)),
                super::super::ApiProxyService::paginate_forward(&events, after, 100)
            );
        }
    }
}
