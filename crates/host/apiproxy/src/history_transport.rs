//! Bound transport pages at complete message and provenance group boundaries.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use dsh_session::SessionEvent;
use dsh_session_persistence::HistoryWindowSink;

#[derive(Clone, Copy)]
pub(crate) enum Direction {
    Backward,
    Forward,
}

#[derive(Clone, Copy)]
struct Boundary {
    first: u64,
    previous: u64,
}

struct Fact {
    seq: u64,
    source_first: u64,
    source_last: u64,
    message: bool,
}

fn boundaries(events: &[Fact], direction: Direction) -> Vec<Boundary> {
    let mut blocked = vec![0_i64; events.len() + 1];
    let mut candidates = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let start = events.partition_point(|item| item.seq < event.source_first);
        let end = events.partition_point(|item| item.seq <= event.source_last);
        if start + 1 < end {
            blocked[start + 1] += 1;
            blocked[end] -= 1;
        }
        if event.message {
            candidates.push(match direction {
                Direction::Backward => start,
                Direction::Forward => index + 1,
            });
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut active = 0_i64;
    for count in &mut blocked {
        active += *count;
        *count = active;
    }
    candidates
        .into_iter()
        .filter(|&index| index > 0 && index < events.len() && blocked[index] == 0)
        .map(|index| Boundary {
            first: events[index].seq,
            previous: events[index - 1].seq,
        })
        .collect()
}

#[derive(Default)]
struct Group {
    first: u64,
    last: u64,
    events: Vec<SessionEvent>,
    bytes: usize,
    oversized: bool,
    last_can_mark: bool,
}

#[derive(Default)]
struct ReadCleanup(Option<Arc<dyn Fn() + Send + Sync>>);

impl ReadCleanup {
    fn run(&self) {
        if let Some(cleanup) = &self.0 {
            cleanup();
        }
    }
}

impl Drop for ReadCleanup {
    fn drop(&mut self) {
        self.run();
    }
}

/// Inspect only small boundary facts, then consume owned events one at a time.
/// At most one retained page, its next complete group, and one pending delta
/// are resident; unselected groups are dropped before transport serialization.
pub(crate) struct TransportSink {
    direction: Direction,
    event_limit: usize,
    byte_limit: usize,
    source_limit: usize,
    source_bytes: usize,
    facts: Vec<Fact>,
    completed: HashSet<u64>,
    cuts: Vec<Boundary>,
    next_cut: usize,
    coalescer: Option<crate::api::sessions::HistoryTransportCoalescer>,
    first: Option<u64>,
    last: Option<u64>,
    current: Group,
    groups: VecDeque<Group>,
    kept_events: usize,
    kept_bytes: usize,
    reduced: bool,
    stopped: bool,
    // Fields are dropped in declaration order: release scan metadata and
    // discarded payloads before collecting on their owning reader thread.
    cleanup: ReadCleanup,
}

impl TransportSink {
    pub(crate) fn new(
        direction: Direction,
        event_limit: usize,
        byte_limit: usize,
        source_limit: usize,
    ) -> Self {
        Self {
            direction,
            event_limit,
            byte_limit,
            source_limit,
            source_bytes: 0,
            facts: vec![],
            completed: HashSet::new(),
            cuts: vec![],
            next_cut: 0,
            coalescer: None,
            first: None,
            last: None,
            current: Group::default(),
            groups: VecDeque::new(),
            kept_events: 0,
            kept_bytes: 0,
            reduced: false,
            stopped: false,
            cleanup: ReadCleanup::default(),
        }
    }

    pub(crate) fn with_cleanup(mut self, cleanup: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        self.cleanup = ReadCleanup(cleanup);
        self
    }

    fn too_large(&self) -> String {
        format!(
            "one safe history group exceeds the {} event or {} byte transport budget",
            self.event_limit, self.byte_limit
        )
    }

    fn prepare(&mut self) {
        if self.coalescer.is_none() {
            self.cuts = boundaries(&self.facts, self.direction);
            self.facts = Vec::new();
            self.current.first = self.first.unwrap_or(0);
            self.coalescer = Some(crate::api::sessions::HistoryTransportCoalescer::new(
                std::mem::take(&mut self.completed),
                true,
            ));
            self.cleanup.run();
        }
    }

    fn marked_len(event: &mut SessionEvent, first: Option<u64>, last: Option<u64>) -> usize {
        let was_null = event.data.is_null();
        let start = event.data.get("__historyStartSeq").cloned();
        let end = event.data.get("__historyEndSeq").cloned();
        if let Some(first) = first {
            event.data["__historyStartSeq"] = first.into();
        }
        if let Some(last) = last {
            event.data["__historyEndSeq"] = last.into();
        }
        let bytes = crate::api::sessions::serialized_event_len(event);
        for (key, previous) in [("__historyStartSeq", start), ("__historyEndSeq", end)] {
            if let Some(previous) = previous {
                event.data[key] = previous;
            } else {
                event.data.as_object_mut().unwrap().remove(key);
            }
        }
        if was_null {
            event.data = serde_json::Value::Null;
        }
        bytes
    }

    fn fits(&mut self) -> bool {
        if self.kept_events > self.event_limit
            || self.kept_bytes > self.byte_limit
            || self.groups.iter().any(|group| group.oversized)
        {
            return false;
        }
        let Some(head) = self.groups.front() else {
            return true;
        };
        let first = head.first;
        let last = self.groups.back().unwrap().last;
        if !head
            .events
            .first()
            .is_some_and(|event| event.data.is_null() || event.data.is_object())
            || !self
                .groups
                .back()
                .unwrap()
                .events
                .last()
                .is_some_and(|event| event.data.is_null() || event.data.is_object())
        {
            return false;
        }
        if self.kept_events == 1 {
            let event = self.groups.front_mut().unwrap().events.first_mut().unwrap();
            return Self::marked_len(event, Some(first), Some(last)) <= self.byte_limit;
        }
        let mut bytes = self.kept_bytes;
        let event = self.groups.front_mut().unwrap().events.first_mut().unwrap();
        bytes -= crate::api::sessions::serialized_event_len(event);
        bytes += Self::marked_len(event, Some(first), None);
        let event = self.groups.back_mut().unwrap().events.last_mut().unwrap();
        bytes -= crate::api::sessions::serialized_event_len(event);
        bytes += Self::marked_len(event, None, Some(last));
        bytes <= self.byte_limit
    }

    fn pop_front(&mut self) {
        let group = self.groups.pop_front().unwrap();
        self.kept_events -= group.events.len();
        self.kept_bytes -= group.bytes;
        self.reduced = true;
    }

    fn close_group(&mut self, last: u64, next: u64) -> Result<(), String> {
        if self.current.events.is_empty() && !self.current.oversized {
            return Ok(());
        }
        self.current.last = last;
        let group = std::mem::replace(
            &mut self.current,
            Group {
                first: next,
                ..Group::default()
            },
        );
        self.kept_events += group.events.len();
        self.kept_bytes += group.bytes;
        self.groups.push_back(group);
        let direction = self.direction;
        match direction {
            Direction::Forward if !self.fits() => {
                let group = self.groups.pop_back().unwrap();
                self.kept_events -= group.events.len();
                self.kept_bytes -= group.bytes;
                if self.groups.is_empty() {
                    return Err(self.too_large());
                }
                self.stopped = true;
                self.reduced = true;
            }
            Direction::Backward => {
                while self.groups.len() > 1 && !self.fits() {
                    self.pop_front();
                }
            }
            Direction::Forward => {}
        }
        Ok(())
    }

    fn accept(&mut self, event: SessionEvent) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        let can_mark = event.data.is_null() || event.data.is_object();
        while self.next_cut < self.cuts.len() && self.cuts[self.next_cut].first <= event.seq.get() {
            let cut = self.cuts[self.next_cut];
            self.next_cut += 1;
            // Keep scalar extension records inside a group: attaching a
            // cursor to their payload would replace user-visible data.
            if (!self.current.events.is_empty() || self.current.oversized)
                && (!self.current.last_can_mark || !can_mark)
            {
                continue;
            }
            self.close_group(cut.previous, cut.first)?;
            if self.stopped {
                return Ok(());
            }
        }
        // A coalesced delta can span a candidate boundary when completed
        // provenance chunks between two unfinished deltas were omitted.
        // Such a boundary cannot divide that transport event.
        let end = event
            .data
            .get("__historyEndSeq")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(event.seq.get());
        while self.next_cut < self.cuts.len() && self.cuts[self.next_cut].first <= end {
            self.next_cut += 1;
        }
        self.current.last_can_mark = can_mark;
        if !self.current.oversized {
            self.current.bytes = self
                .current
                .bytes
                .saturating_add(crate::api::sessions::serialized_event_len(&event));
            self.current.events.push(event);
            if self.current.bytes > self.byte_limit || self.current.events.len() > self.event_limit
            {
                self.current.events = Vec::new();
                self.current.bytes = 0;
                self.current.oversized = true;
                if matches!(self.direction, Direction::Backward) {
                    while !self.groups.is_empty() {
                        self.pop_front();
                    }
                }
            }
        }
        // Once the growing group cannot coexist with the retained page, its
        // remaining events cannot make the combined event/byte counts smaller.
        // Release groups now instead of holding two nearly full pages until
        // the next message boundary arrives.
        while !self.groups.is_empty()
            && (self.current.oversized
                || self.kept_events + self.current.events.len() > self.event_limit
                || self.kept_bytes.saturating_add(self.current.bytes) > self.byte_limit)
        {
            match self.direction {
                Direction::Forward => {
                    self.current = Group::default();
                    self.stopped = true;
                    self.reduced = true;
                    break;
                }
                Direction::Backward => self.pop_front(),
            }
        }
        Ok(())
    }
}

impl HistoryWindowSink for TransportSink {
    fn inspect(&mut self, event: &SessionEvent) -> Result<(), String> {
        if self.first.is_none() {
            // Previous pages may have been freed remotely by the HTTP worker
            // while this blocking reader was idle in the pool.
            self.cleanup.run();
        }
        self.source_bytes = self
            .source_bytes
            .saturating_add(crate::api::sessions::serialized_event_len(event));
        if self.source_bytes > self.source_limit {
            return Err("history scan exceeds the 64 MiB source budget".into());
        }
        self.first.get_or_insert_with(|| {
            event
                .data
                .get("__historyStartSeq")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(event.seq.get())
        });
        self.last = Some(
            event
                .data
                .get("__historyEndSeq")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(event.seq.get()),
        );
        let mut source_first = event.seq.get();
        let mut source_last = source_first;
        // A replacement cites the old surface it supersedes, which can be
        // arbitrarily far back in history. Its retained reference does not
        // require that surface to travel on the same page. Assistant messages
        // still own their omitted provenance chunks, including replacements.
        let binds_sources = event.type_ == "assistant/message"
            || event.surface_op.as_ref().is_none_or(|op| op.is_append());
        for seq in event.source_event_seqs.iter().flatten() {
            if binds_sources {
                source_first = source_first.min(*seq);
                source_last = source_last.max(*seq);
            }
            if event.type_ == "assistant/message" && *seq >= self.first.unwrap() {
                self.completed.insert(*seq);
            }
        }
        self.facts.push(Fact {
            seq: event.seq.get(),
            source_first,
            source_last,
            message: matches!(event.type_.as_str(), "user/message" | "assistant/message")
                && event.surface_op.as_ref().is_none_or(|op| op.is_append()),
        });
        Ok(())
    }

    fn push(&mut self, mut event: SessionEvent) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.prepare();
        crate::public_event::strip(&mut event);
        if let Some(event) = self.coalescer.as_mut().unwrap().push(event) {
            self.accept(event)?;
        }
        if self.stopped {
            // push leaves the next raw event pending while it emits the old
            // one. A finished forward page must not retain that unused event
            // (or its completed-source set) for the remaining validation scan.
            self.coalescer = None;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(Vec<SessionEvent>, bool), String> {
        self.prepare();
        if !self.stopped {
            if let Some(event) = self.coalescer.take().unwrap().finish() {
                self.accept(event)?;
            }
            if !self.stopped {
                self.close_group(self.last.unwrap_or(0), 0)?;
            }
        }
        if !self.fits() {
            return Err(self.too_large());
        }
        let first = self.groups.front().map(|group| group.first);
        let last = self.groups.back().map(|group| group.last);
        let mut events: Vec<_> = self
            .groups
            .into_iter()
            .flat_map(|group| group.events)
            .collect();
        if let (Some(event), Some(first)) = (events.first_mut(), first) {
            event.data["__historyStartSeq"] = first.into();
        }
        if let (Some(event), Some(last)) = (events.last_mut(), last) {
            event.data["__historyEndSeq"] = last.into();
        }
        events.shrink_to_fit();
        Ok((events, self.reduced))
    }
}

pub(crate) fn compact_page(
    events: Vec<SessionEvent>,
    direction: Direction,
    event_limit: usize,
    byte_limit: usize,
) -> Result<(Vec<SessionEvent>, bool), String> {
    let mut sink = Box::new(TransportSink::new(
        direction,
        event_limit,
        byte_limit,
        usize::MAX,
    ));
    for event in &events {
        sink.inspect(event)?;
    }
    for event in events {
        sink.push(event)?;
    }
    sink.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str, text: &str, sources: Option<Vec<u64>>) -> SessionEvent {
        SessionEvent {
            seq: dsh_session::SessionSeq::new(seq).unwrap(),
            time: seq as i64,
            type_: kind.into(),
            data: serde_json::json!({"text": text}),
            surface_op: is_message_kind(kind).then_some(dsh_session::SurfaceOp::Append),
            source_event_seqs: sources,
            ignorable: None,
        }
    }

    fn is_message_kind(kind: &str) -> bool {
        matches!(kind, "user/message" | "assistant/message")
    }

    fn fixture() -> Vec<SessionEvent> {
        vec![
            event(0, "user/message", "first", None),
            event(1, "tool/result", &"a".repeat(1500), None),
            event(2, "assistant/message", "answer", Some(vec![1])),
            event(3, "user/message", "second", None),
            event(4, "tool/result", &"b".repeat(1500), None),
            event(5, "assistant/message", "answer", Some(vec![4])),
            event(6, "turn/end", "", None),
        ]
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reader_cleanup_runs_on_the_blocking_owner_on_success_and_error() {
        let caller = std::thread::current().id();
        for fail in [false, true] {
            let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
            let observed = calls.clone();
            let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                observed.lock().unwrap().push(std::thread::current().id());
            });
            let (reader, result) = tokio::task::spawn_blocking(move || {
                let reader = std::thread::current().id();
                let source = fixture();
                let mut sink = Box::new(
                    TransportSink::new(
                        Direction::Forward,
                        4096,
                        8 * 1024 * 1024,
                        if fail { 0 } else { usize::MAX },
                    )
                    .with_cleanup(Some(cleanup)),
                );
                let result = (|| {
                    for event in &source {
                        sink.inspect(event)?;
                    }
                    for event in source {
                        sink.push(event)?;
                    }
                    sink.finish()
                })();
                (reader, result)
            })
            .await
            .unwrap();
            assert_ne!(reader, caller);
            let calls = calls.lock().unwrap();
            assert!(calls.len() >= if fail { 2 } else { 3 });
            assert!(calls.iter().all(|thread| *thread == reader));
            if fail {
                assert!(result.unwrap_err().contains("source budget"));
            } else {
                let (page, reduced) = result.unwrap();
                assert!(!reduced);
                assert_eq!(
                    page,
                    crate::api::sessions::coalesce_history_transport_events(fixture())
                );
            }
        }
    }

    #[test]
    fn successful_pages_keep_the_existing_compacted_body() {
        let source = fixture();
        let expected = crate::api::sessions::coalesce_history_transport_slice(&source);
        for direction in [Direction::Backward, Direction::Forward] {
            let (page, reduced) =
                compact_page(source.clone(), direction, 4096, 8 * 1024 * 1024).unwrap();
            assert!(!reduced);
            assert_eq!(page, expected);
        }
    }

    #[test]
    fn byte_limited_pages_keep_sources_and_cover_both_directions() {
        let source = fixture();
        for direction in [Direction::Backward, Direction::Forward] {
            let mut remaining = source.clone();
            let mut seen = Vec::new();
            while !remaining.is_empty() {
                let (page, more) = compact_page(remaining.clone(), direction, 4096, 2300).unwrap();
                let first = page.first().unwrap().data["__historyStartSeq"]
                    .as_u64()
                    .unwrap();
                let last = page.last().unwrap().data["__historyEndSeq"]
                    .as_u64()
                    .unwrap();
                assert!(
                    page.iter()
                        .map(crate::api::sessions::serialized_event_len)
                        .sum::<usize>()
                        <= 2300
                );
                assert_eq!(
                    page.iter().any(|event| event.seq.get() == 1),
                    page.iter().any(|event| event.seq.get() == 2)
                );
                assert_eq!(
                    page.iter().any(|event| event.seq.get() == 4),
                    page.iter().any(|event| event.seq.get() == 5)
                );
                seen.extend(first..=last);
                remaining.retain(|event| match direction {
                    Direction::Backward => event.seq.get() < first,
                    Direction::Forward => event.seq.get() > last,
                });
                assert_eq!(more, !remaining.is_empty());
            }
            seen.sort_unstable();
            assert_eq!(seen, (0..=6).collect::<Vec<_>>());
        }
    }

    #[test]
    fn growing_groups_release_pages_that_cannot_be_retained() {
        let source = vec![
            event(0, "tool/result", &"a".repeat(500), None),
            event(1, "assistant/message", "first", Some(vec![0])),
            event(2, "tool/result", &"b".repeat(350), None),
            event(3, "tool/result", &"c".repeat(350), None),
            event(4, "assistant/message", "second", Some(vec![2, 3])),
            event(5, "turn/end", "", None),
        ];
        let byte_limit = source[2..]
            .iter()
            .map(crate::api::sessions::serialized_event_len)
            .sum::<usize>()
            + 100;
        for direction in [Direction::Forward, Direction::Backward] {
            let mut sink = Box::new(TransportSink::new(direction, 4096, byte_limit, usize::MAX));
            for event in &source {
                sink.inspect(event).unwrap();
            }
            for event in &source[..5] {
                sink.push(event.clone()).unwrap();
            }
            match direction {
                Direction::Forward => {
                    assert!(sink.stopped);
                    assert!(sink.current.events.is_empty());
                    assert!(
                        sink.coalescer.is_none(),
                        "unused pending event must be released"
                    );
                }
                Direction::Backward => {
                    assert!(
                        sink.groups.is_empty(),
                        "old page must be released before group end"
                    );
                    assert_eq!(sink.current.events.len(), 2);
                }
            }
            sink.push(source[5].clone()).unwrap();
            let (page, more) = sink.finish().unwrap();
            assert!(more);
            let expected = match direction {
                Direction::Forward => &source[..2],
                Direction::Backward => &source[2..],
            };
            assert_eq!(
                page,
                crate::api::sessions::coalesce_history_transport_slice(expected)
            );
        }
    }

    #[test]
    fn event_budget_shortens_whole_groups_and_rejects_one_oversized_group() {
        let source = fixture();
        for direction in [Direction::Backward, Direction::Forward] {
            let (page, more) = compact_page(source.clone(), direction, 4, 10000).unwrap();
            assert!(more);
            assert!(page.len() <= 4);
            let error = compact_page(source[1..3].to_vec(), direction, 1, 10000).unwrap_err();
            assert!(error.contains("one safe history group"));
            assert!(compact_page(source[1..3].to_vec(), direction, 4096, 1000).is_err());
        }
    }

    #[test]
    fn overlapping_source_groups_are_never_split() {
        let source = vec![
            event(0, "tool/result", "a", None),
            event(1, "user/message", "b", None),
            event(2, "assistant/message", "c", Some(vec![0])),
            event(3, "assistant/message", "d", Some(vec![1])),
        ];
        for direction in [Direction::Backward, Direction::Forward] {
            assert!(compact_page(source.clone(), direction, 3, 10000).is_err());
        }
    }

    #[test]
    fn replacements_keep_their_references_without_binding_unrelated_history_pages() {
        for kind in ["system/message", "user/message", "tool/result"] {
            // Exercise both a reference in this window and one in an older
            // page, as occurs when a system prompt changes during a session.
            for offset in [0, 8640] {
                let mut source = fixture();
                source.pop();
                for event in &mut source {
                    event.seq = dsh_session::SessionSeq::new(event.seq.get() + offset).unwrap();
                    for seq in event.source_event_seqs.iter_mut().flatten() {
                        *seq += offset;
                    }
                }
                let mut replacement = event(offset + 6, kind, "replacement", Some(vec![0]));
                replacement.surface_op = Some(dsh_session::SurfaceOp::Replace { start: 0, end: 0 });
                source.push(replacement);
                source.push(event(offset + 7, "user/message", "next", None));
                source.push(event(offset + 8, "turn/end", "", None));
                let mut expected = crate::api::sessions::coalesce_history_transport_slice(&source);
                let clear_cursors = |events: &mut Vec<SessionEvent>| {
                    for event in events {
                        let data = event.data.as_object_mut().unwrap();
                        data.remove("__historyStartSeq");
                        data.remove("__historyEndSeq");
                    }
                };
                clear_cursors(&mut expected);
                for direction in [Direction::Backward, Direction::Forward] {
                    let mut remaining = source.clone();
                    let mut actual = Vec::new();
                    let mut covered = Vec::new();
                    while !remaining.is_empty() {
                        let (page, more) =
                            compact_page(remaining.clone(), direction, 4096, 2300).unwrap();
                        let first = page.first().unwrap().data["__historyStartSeq"]
                            .as_u64()
                            .unwrap();
                        let last = page.last().unwrap().data["__historyEndSeq"]
                            .as_u64()
                            .unwrap();
                        assert!(
                            page.iter()
                                .map(crate::api::sessions::serialized_event_len)
                                .sum::<usize>()
                                <= 2300
                        );
                        covered.extend(first..=last);
                        actual.extend(page);
                        remaining.retain(|event| match direction {
                            Direction::Backward => event.seq.get() < first,
                            Direction::Forward => event.seq.get() > last,
                        });
                        assert_eq!(more, !remaining.is_empty());
                    }
                    covered.sort_unstable();
                    assert_eq!(covered, (offset..=offset + 8).collect::<Vec<_>>());
                    actual.sort_unstable_by_key(|event| event.seq.get());
                    clear_cursors(&mut actual);
                    assert_eq!(actual, expected);
                }
            }
        }
    }

    #[test]
    fn assistant_replacements_still_bind_their_omitted_provenance() {
        let mut replacement = event(2, "assistant/message", "answer", Some(vec![0]));
        replacement.surface_op = Some(dsh_session::SurfaceOp::Replace { start: 0, end: 0 });
        let source = vec![
            event(0, "assistant/chunk", "answer", None),
            event(1, "user/message", "question", None),
            replacement,
        ];
        for direction in [Direction::Backward, Direction::Forward] {
            assert!(compact_page(source.clone(), direction, 1, 10000).is_err());
            let (page, more) = compact_page(source.clone(), direction, 2, 10000).unwrap();
            assert!(!more);
            assert_eq!(page.len(), 2);
            assert_eq!(page[0].data["__historyStartSeq"], 0);
            assert_eq!(page[1].data["__historyEndSeq"], 2);
        }
    }

    #[test]
    fn completed_chunks_keep_original_cursor_ranges_after_both_direction_cuts() {
        let mut source = Vec::new();
        for turn in 0..2_u64 {
            let start = turn * 5;
            source.push(event(start, "user/message", "question", None));
            for seq in start + 1..=start + 3 {
                let mut chunk = event(seq, "assistant/chunk", "", None);
                chunk.data = serde_json::json!({
                    "turn": turn, "step": 1,
                    "chunk": {"type": "text-delta", "index": 0, "text": "answer"}
                });
                source.push(chunk);
            }
            source.push(event(
                start + 4,
                "assistant/message",
                "answeransweranswer",
                Some((start + 1..=start + 3).collect()),
            ));
        }
        source.push(event(10, "turn/end", "", None));
        for direction in [Direction::Backward, Direction::Forward] {
            let mut remaining = source.clone();
            let mut seen = Vec::new();
            let mut messages = Vec::new();
            while !remaining.is_empty() {
                let (page, more) = compact_page(remaining.clone(), direction, 3, 10000).unwrap();
                let first = page.first().unwrap().data["__historyStartSeq"]
                    .as_u64()
                    .unwrap();
                let last = page.last().unwrap().data["__historyEndSeq"]
                    .as_u64()
                    .unwrap();
                assert!(page.iter().all(|event| event.type_ != "assistant/chunk"));
                messages.extend(
                    page.iter()
                        .filter(|event| event.type_ == "assistant/message")
                        .map(|event| event.seq.get()),
                );
                seen.extend(first..=last);
                remaining.retain(|event| match direction {
                    Direction::Backward => event.seq.get() < first,
                    Direction::Forward => event.seq.get() > last,
                });
                assert_eq!(more, !remaining.is_empty());
            }
            seen.sort_unstable();
            messages.sort_unstable();
            assert_eq!(seen, (0..=10).collect::<Vec<_>>());
            assert_eq!(messages, vec![4, 9]);
        }
    }

    #[test]
    fn cursor_marker_size_can_require_the_next_safe_boundary() {
        let source: Vec<_> = (0..4)
            .map(|seq| event(seq, "user/message", "text", None))
            .collect();
        let compact = crate::api::sessions::coalesce_history_transport_slice(&source);
        for direction in [Direction::Backward, Direction::Forward] {
            let pair = match direction {
                Direction::Backward => &compact[2..],
                Direction::Forward => &compact[..2],
            };
            let limit = pair
                .iter()
                .map(crate::api::sessions::serialized_event_len)
                .sum::<usize>();
            let (page, more) = compact_page(source.clone(), direction, 2, limit).unwrap();
            assert!(more);
            assert_eq!(page.len(), 1);
            assert!(crate::api::sessions::serialized_event_len(&page[0]) <= limit);
            assert_eq!(
                page[0].seq.get(),
                match direction {
                    Direction::Backward => 3,
                    Direction::Forward => 0,
                }
            );
        }
    }

    #[test]
    fn streaming_owned_pages_drop_unselected_payloads_before_finishing() {
        for direction in [Direction::Backward, Direction::Forward] {
            let mut sink = Box::new(TransportSink::new(direction, 4096, 350_000, usize::MAX));
            for seq in 0..80 {
                sink.inspect(&event(seq, "user/message", &"x".repeat(100_000), None))
                    .unwrap();
            }
            for seq in 0..80 {
                sink.push(event(seq, "user/message", &"x".repeat(100_000), None))
                    .unwrap();
                assert!(sink.kept_bytes <= 350_000);
                assert!(sink.current.bytes <= 350_000);
            }
            let (page, more) = sink.finish().unwrap();
            assert!(more);
            assert_eq!(page.len(), 3);
            assert_eq!(
                page[0].seq.get(),
                match direction {
                    Direction::Backward => 77,
                    Direction::Forward => 0,
                }
            );
        }
    }

    #[test]
    fn a_coalesced_delta_cannot_be_cut_across_omitted_completed_sources() {
        let mut source = Vec::new();
        for seq in 0..4 {
            let mut chunk = event(seq, "assistant/chunk", "", None);
            chunk.data = serde_json::json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":seq.to_string()}});
            source.push(chunk);
        }
        source.push(event(4, "assistant/message", "completed", Some(vec![1, 2])));
        // The retained 0 and 3 deltas coalesce across the nominal source-group
        // boundary at 1. Splitting there would drop the text from seq 3.
        assert!(compact_page(source.clone(), Direction::Backward, 1, 10000).is_err());
        let (page, more) = compact_page(source, Direction::Backward, 2, 10000).unwrap();
        assert!(!more);
        assert_eq!(page[0].data["chunk"]["text"], "03");
        assert_eq!(page[0].data["__historyStartSeq"], 0);
        assert_eq!(page[1].data["__historyEndSeq"], 4);
    }

    #[test]
    fn sizing_intermediate_group_cursors_preserves_non_object_payloads() {
        for data in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!("marker"),
            serde_json::json!([1, 2]),
        ] {
            let mut marker = event(1, "extension/marker", "", None);
            marker.data = data.clone();
            marker.ignorable = Some(true);
            let source = vec![
                event(0, "user/message", "first", None),
                marker,
                event(2, "user/message", "second", None),
                event(3, "assistant/message", "answer", None),
            ];
            let expected = crate::api::sessions::coalesce_history_transport_slice(&source);
            for direction in [Direction::Backward, Direction::Forward] {
                let (page, more) = compact_page(source.clone(), direction, 4096, 10000).unwrap();
                assert!(!more);
                assert_eq!(page, expected);
                assert_eq!(page[1].data, data);
            }
        }
    }
}
