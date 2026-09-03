#[cfg(test)]
use std::cell::Cell;
use std::collections::VecDeque;
use std::io::Read;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use dsh_session::{SessionEvent, StorageRecord};
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};

#[derive(Debug)]
pub enum PackedStreamError {
    Noncanonical(usize, bool),
    Invalid(String),
}

#[cfg(test)]
thread_local! {
    static PACKED_EVENT_BUILDS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn reset_packed_event_builds() {
    PACKED_EVENT_BUILDS.set(0);
}

#[cfg(test)]
fn packed_event_builds() -> usize {
    PACKED_EVENT_BUILDS.get()
}

#[cfg(test)]
pub fn visit_reader<R, F>(reader: R, on_event: &mut F) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    visit_reader_inner(reader, None, None, false, on_event)
}

/// Visit only events at or after `from_seq`. Canonical packed members before
/// the cut are consumed as ignored JSON values instead of materializing their
/// strings and `SessionEvent`s, keeping far indexed reads allocation-bounded.
pub fn visit_reader_from<R, F>(
    reader: R,
    from_seq: u64,
    on_event: &mut F,
) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    visit_reader_inner(reader, None, Some(from_seq), false, on_event)
}

/// Visit only event envelopes, skipping canonical packed chunk payloads.
pub fn visit_nonpacked_reader<R, F>(reader: R, on_event: &mut F) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    visit_reader_inner(reader, None, None, true, on_event)
}

pub fn visit_tail_reader<R, F>(
    reader: R,
    capacity: usize,
    on_event: &mut F,
) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    visit_reader_inner(reader, Some(capacity), None, false, on_event)
}

fn visit_reader_inner<R, F>(
    reader: R,
    tail_capacity: Option<usize>,
    from_seq: Option<u64>,
    skip_packed: bool,
    on_event: &mut F,
) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    struct CountingReader<R> {
        inner: R,
        bytes: Arc<AtomicUsize>,
    }
    impl<R: Read> Read for CountingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.inner.read(buffer)?;
            self.bytes.fetch_add(read, Ordering::Relaxed);
            Ok(read)
        }
    }
    let bytes = Arc::new(AtomicUsize::new(0));
    let mut de = serde_json::Deserializer::from_reader(CountingReader {
        inner: reader,
        bytes: bytes.clone(),
    });
    let mut accepting = true;
    let mut emitted = 0_usize;
    loop {
        let before = bytes.load(Ordering::Relaxed);
        let mut counted = |event| {
            emitted += 1;
            on_event(event)
        };
        let result = RecordSeed {
            on_event: &mut counted,
            accepting,
            tail_capacity,
            from_seq,
            skip_packed,
        }
        .deserialize(&mut de);
        match result {
            Ok(next) => {
                accepting = next;
                if skip_packed && !accepting {
                    return Ok(false);
                }
            }
            Err(error)
                if error.is_eof()
                    && (error.column() == 0 || bytes.load(Ordering::Relaxed) == before) =>
            {
                return Ok(accepting);
            }
            Err(error) if error.to_string().contains("noncanonical packed") => {
                return Err(PackedStreamError::Noncanonical(emitted, accepting));
            }
            Err(error) => return Err(PackedStreamError::Invalid(error.to_string())),
        }
    }
}

struct RecordSeed<'a, F> {
    on_event: &'a mut F,
    accepting: bool,
    tail_capacity: Option<usize>,
    from_seq: Option<u64>,
    skip_packed: bool,
}

impl<'de, F> DeserializeSeed<'de> for RecordSeed<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<bool, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(RecordVisitor {
            on_event: self.on_event,
            accepting: self.accepting,
            tail_capacity: self.tail_capacity,
            from_seq: self.from_seq,
            skip_packed: self.skip_packed,
        })
    }
}

struct RecordVisitor<'a, F> {
    on_event: &'a mut F,
    accepting: bool,
    tail_capacity: Option<usize>,
    from_seq: Option<u64>,
    skip_packed: bool,
}

impl<'de, F> Visitor<'de> for RecordVisitor<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a session storage record")
    }

    fn visit_map<M>(self, mut map: M) -> Result<bool, M::Error>
    where
        M: MapAccess<'de>,
    {
        let first = map
            .next_key::<String>()?
            .ok_or_else(|| M::Error::custom("empty session storage record"))?;
        if first != "type" {
            return Err(M::Error::custom("noncanonical packed record"));
        }
        let type_: String = map.next_value()?;
        if !matches!(
            type_.as_str(),
            "text-chunks" | "reasoning-chunks" | "tool-call-chunks"
        ) {
            let mut object = serde_json::Map::new();
            object.insert("type".into(), serde_json::Value::String(type_));
            while let Some(key) = map.next_key::<String>()? {
                object.insert(key, map.next_value()?);
            }
            let event: SessionEvent = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(M::Error::custom)?;
            if self
                .from_seq
                .is_some_and(|from_seq| event.seq.get() < from_seq)
            {
                return Ok(self.accepting);
            }
            return if self.accepting {
                (self.on_event)(event).map_err(M::Error::custom)
            } else {
                Ok(false)
            };
        }
        expect_key(&mut map, "seq0")?;
        let seq0 = map.next_value()?;
        expect_key(&mut map, "time0")?;
        let time0 = map.next_value()?;
        expect_key(&mut map, "data")?;
        if self.skip_packed {
            let _: IgnoredAny = map.next_value()?;
            if map.next_key::<IgnoredAny>()?.is_some() {
                return Err(M::Error::custom("packed record has trailing fields"));
            }
            return Ok(self.accepting);
        }
        let accepted = map.next_value_seed(DataSeed {
            kind: type_,
            seq0,
            time0,
            on_event: self.on_event,
            accepting: self.accepting,
            tail_capacity: self.tail_capacity,
            from_seq: self.from_seq,
        })?;
        if map.next_key::<IgnoredAny>()?.is_some() {
            return Err(M::Error::custom("packed record has trailing fields"));
        }
        Ok(accepted)
    }
}

fn expect_key<'de, M>(map: &mut M, expected: &str) -> Result<(), M::Error>
where
    M: MapAccess<'de>,
{
    let key = map
        .next_key::<String>()?
        .ok_or_else(|| M::Error::custom(format!("missing packed field {expected}")))?;
    if key != expected {
        return Err(M::Error::custom("noncanonical packed record"));
    }
    Ok(())
}

struct DataSeed<'a, F> {
    kind: String,
    seq0: u64,
    time0: i64,
    on_event: &'a mut F,
    accepting: bool,
    tail_capacity: Option<usize>,
    from_seq: Option<u64>,
}

impl<'de, F> DeserializeSeed<'de> for DataSeed<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<bool, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(DataVisitor { seed: self })
    }
}

struct DataVisitor<'a, F> {
    seed: DataSeed<'a, F>,
}

impl<'de, F> Visitor<'de> for DataVisitor<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("canonical packed chunk data")
    }

    fn visit_map<M>(self, mut map: M) -> Result<bool, M::Error>
    where
        M: MapAccess<'de>,
    {
        expect_key(&mut map, "turn")?;
        let turn = map.next_value()?;
        expect_key(&mut map, "step")?;
        let step = map.next_value()?;
        expect_key(&mut map, "index")?;
        let index = map.next_value()?;
        expect_key(&mut map, "dt")?;
        let dt: Vec<i64> = map.next_value()?;
        let (id, name): (Option<String>, Option<String>) = if self.seed.kind == "tool-call-chunks" {
            expect_key(&mut map, "id")?;
            let id: String = map.next_value()?;
            let key = map
                .next_key::<String>()?
                .ok_or_else(|| M::Error::custom("missing packed args"))?;
            if key == "name" {
                let name = map.next_value()?;
                expect_key(&mut map, "args")?;
                (Some(id), Some(name))
            } else if key == "args" {
                (Some(id), None)
            } else {
                return Err(M::Error::custom("noncanonical packed record"));
            }
        } else {
            expect_key(&mut map, "texts")?;
            (None, None)
        };
        let accepted = map.next_value_seed(MembersSeed {
            kind: &self.seed.kind,
            seq0: self.seed.seq0,
            time0: self.seed.time0,
            turn,
            step,
            index,
            dt: &dt,
            id: id.as_deref(),
            name: name.as_deref(),
            on_event: self.seed.on_event,
            accepting: self.seed.accepting,
            tail_capacity: self.seed.tail_capacity,
            from_seq: self.seed.from_seq,
        })?;
        if map.next_key::<IgnoredAny>()?.is_some() {
            return Err(M::Error::custom("packed data has trailing fields"));
        }
        Ok(accepted)
    }
}

struct MembersSeed<'a, F> {
    kind: &'a str,
    seq0: u64,
    time0: i64,
    turn: u64,
    step: u64,
    index: u64,
    dt: &'a [i64],
    id: Option<&'a str>,
    name: Option<&'a str>,
    on_event: &'a mut F,
    accepting: bool,
    tail_capacity: Option<usize>,
    from_seq: Option<u64>,
}

impl<'de, F> DeserializeSeed<'de> for MembersSeed<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<bool, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(MembersVisitor { seed: self })
    }
}

struct MembersVisitor<'a, F> {
    seed: MembersSeed<'a, F>,
}

impl<'de, F> Visitor<'de> for MembersVisitor<'_, F>
where
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a non-empty packed member array")
    }

    fn visit_seq<A>(mut self, mut seq: A) -> Result<bool, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut k = 0_usize;
        let mut time = self.seed.time0;
        let mut tail = self.seed.tail_capacity.map(VecDeque::with_capacity);
        loop {
            let event_seq = self.seed.seq0.saturating_add(k as u64);
            let materialize = tail.is_some()
                || (self.seed.accepting
                    && self
                        .seed
                        .from_seq
                        .is_none_or(|from_seq| event_seq >= from_seq));
            let member = if materialize {
                let Some(member) = seq.next_element::<String>()? else {
                    break;
                };
                Some(member)
            } else {
                if seq.next_element::<IgnoredAny>()?.is_none() {
                    break;
                }
                None
            };
            self.seed
                .seq0
                .checked_add(k as u64)
                .ok_or_else(|| A::Error::custom(format!("{} seq overflow", self.seed.kind)))?;
            if k > 0 {
                let gap =
                    self.seed.dt.get(k - 1).ok_or_else(|| {
                        A::Error::custom("dt length does not match packed members")
                    })?;
                time = time
                    .checked_add(*gap)
                    .ok_or_else(|| A::Error::custom("packed member time overflow"))?;
            }
            if let Some(member) = member {
                if let Some(tail) = tail.as_mut() {
                    tail.push_back((k, time, member));
                    if tail.len() > self.seed.tail_capacity.unwrap_or(0) {
                        tail.pop_front();
                    }
                } else if self.seed.accepting {
                    let event =
                        packed_event(&self.seed, k, time, member).map_err(A::Error::custom)?;
                    self.seed.accepting = (self.seed.on_event)(event).map_err(A::Error::custom)?;
                }
            }
            k += 1;
        }
        if k == 0 || self.seed.dt.len() + 1 != k {
            return Err(A::Error::custom("dt length does not match packed members"));
        }
        if let Some(tail) = tail {
            for (k, time, member) in tail {
                if !self.seed.accepting {
                    break;
                }
                let event = packed_event(&self.seed, k, time, member).map_err(A::Error::custom)?;
                self.seed.accepting = (self.seed.on_event)(event).map_err(A::Error::custom)?;
            }
        }
        Ok(self.seed.accepting)
    }
}

fn packed_event<F>(
    seed: &MembersSeed<'_, F>,
    k: usize,
    time: i64,
    member: String,
) -> Result<SessionEvent, String> {
    #[cfg(test)]
    PACKED_EVENT_BUILDS.set(PACKED_EVENT_BUILDS.get() + 1);
    let chunk = match seed.kind {
        "text-chunks" => {
            serde_json::json!({"type":"text-delta","index":seed.index,"text":member})
        }
        "reasoning-chunks" => {
            serde_json::json!({"type":"reasoning-delta","index":seed.index,"text":member})
        }
        "tool-call-chunks" => {
            let mut value = serde_json::json!({"type":"tool-call-delta","index":seed.index,"id":seed.id.unwrap_or_default(),"argumentsDelta":member});
            if let Some(name) = seed.name {
                value["name"] = serde_json::Value::String(name.to_string());
            }
            value
        }
        _ => return Err("unknown packed chunk type".to_string()),
    };
    Ok(SessionEvent {
        type_: "assistant/chunk".into(),
        seq: dsh_session::SessionSeq::new(
            seed.seq0
                .checked_add(k as u64)
                .ok_or_else(|| format!("{} seq overflow", seed.kind))?,
        )?,
        time,
        data: serde_json::json!({"turn":seed.turn,"step":seed.step,"chunk":chunk}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    })
}

pub fn visit_frame_from<R, F>(
    reader: R,
    from_seq: u64,
    on_event: &mut F,
) -> Result<bool, PackedStreamError>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    visit_reader_from(reader, from_seq, on_event)
}

#[cfg(test)]
pub fn fallback_visit_skip<R, F>(
    reader: R,
    mut skip: usize,
    mut accepting: bool,
    on_event: &mut F,
) -> Result<bool, String>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    let records = serde_json::Deserializer::from_reader(reader).into_iter::<StorageRecord>();
    for record in records {
        let record = record.map_err(|error| format!("invalid JSONL event record: {error}"))?;
        if accepting
            && !dsh_session::visit_decoded_storage_record_events(record, &mut |event| {
                if skip > 0 {
                    skip -= 1;
                    return Ok(true);
                }
                on_event(event)
            })?
        {
            accepting = false;
        }
    }
    Ok(accepting)
}

pub fn fallback_visit_skip_from<R, F>(
    reader: R,
    from_seq: u64,
    mut skip: usize,
    mut accepting: bool,
    on_event: &mut F,
) -> Result<bool, String>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    let records = serde_json::Deserializer::from_reader(reader).into_iter::<StorageRecord>();
    for record in records {
        let record = record.map_err(|error| format!("invalid JSONL event record: {error}"))?;
        if accepting
            && !dsh_session::visit_decoded_storage_record_events(
                record,
                &mut |event: SessionEvent| {
                    if event.seq.get() < from_seq {
                        return Ok(true);
                    }
                    if skip > 0 {
                        skip -= 1;
                        return Ok(true);
                    }
                    on_event(event)
                },
            )?
        {
            accepting = false;
        }
    }
    Ok(accepting)
}

pub fn fallback_visit_nonpacked<R, F>(
    reader: R,
    mut skip: usize,
    mut accepting: bool,
    on_event: &mut F,
) -> Result<bool, String>
where
    R: Read,
    F: FnMut(SessionEvent) -> Result<bool, String>,
{
    let records = serde_json::Deserializer::from_reader(reader).into_iter::<StorageRecord>();
    for record in records {
        let record = record.map_err(|error| format!("invalid JSONL event record: {error}"))?;
        if matches!(&record, StorageRecord::Row(_)) {
            continue;
        }
        if accepting
            && !dsh_session::visit_decoded_storage_record_events(record, &mut |event| {
                if skip > 0 {
                    skip -= 1;
                    return Ok(true);
                }
                on_event(event)
            })?
        {
            accepting = false;
        }
    }
    Ok(accepting)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_decode(input: &str) -> Vec<SessionEvent> {
        let value: serde_json::Value = serde_json::from_str(input).unwrap();
        dsh_session::decode_storage_record(&value).unwrap()
    }

    #[test]
    fn canonical_text_row_matches_legacy_decode() {
        let input = r#"{"type":"text-chunks","seq0":7,"time0":100,"data":{"turn":2,"step":3,"index":1,"dt":[4,5],"texts":["a","b","c"]}}"#;
        let mut actual = Vec::new();
        let accepted = visit_reader(input.as_bytes(), &mut |event| {
            actual.push(event);
            Ok(true)
        })
        .unwrap();
        assert!(accepted);
        assert_eq!(actual, old_decode(input));
    }

    #[test]
    fn canonical_tool_row_matches_legacy_decode() {
        let input = r#"{"type":"tool-call-chunks","seq0":10,"time0":200,"data":{"turn":4,"step":5,"index":2,"dt":[9],"id":"call-1","name":"run","args":["x","y"]}}"#;
        let mut actual = Vec::new();
        visit_reader(input.as_bytes(), &mut |event| {
            actual.push(event);
            Ok(true)
        })
        .unwrap();
        assert_eq!(actual, old_decode(input));
    }

    #[test]
    fn early_stop_still_validates_remaining_members() {
        let input = r#"{"type":"text-chunks","seq0":0,"time0":0,"data":{"turn":0,"step":0,"index":0,"dt":[],"texts":["a","b"]}}"#;
        let error = visit_reader(input.as_bytes(), &mut |_| Ok(false)).unwrap_err();
        assert!(matches!(error, PackedStreamError::Invalid(_)));
    }

    #[test]
    fn noncanonical_order_requests_fallback() {
        let input = r#"{"seq0":0,"type":"text-chunks","time0":0,"data":{"turn":0,"step":0,"index":0,"dt":[],"texts":["a"]}}"#;
        let error = visit_reader(input.as_bytes(), &mut |_| Ok(true)).unwrap_err();
        assert!(matches!(error, PackedStreamError::Noncanonical(_, _)));
    }

    #[test]
    fn fallback_skips_events_already_emitted_by_fast_path() {
        let input = concat!(
            "{\"type\":\"text-chunks\",\"seq0\":0,\"time0\":0,\"data\":{\"turn\":0,\"step\":0,\"index\":0,\"dt\":[],\"texts\":[\"a\"]}}\n",
            "{\"seq0\":1,\"type\":\"text-chunks\",\"time0\":1,\"data\":{\"turn\":0,\"step\":0,\"index\":0,\"dt\":[],\"texts\":[\"a\"]}}"
        );
        let mut events = Vec::new();
        let error = visit_reader(input.as_bytes(), &mut |event| {
            events.push(event);
            Ok(true)
        })
        .unwrap_err();
        let PackedStreamError::Noncanonical(emitted, accepting) = error else {
            panic!("expected noncanonical fallback");
        };
        assert!(accepting);
        fallback_visit_skip(input.as_bytes(), emitted, accepting, &mut |event| {
            events.push(event);
            Ok(true)
        })
        .unwrap();
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn forward_cut_builds_only_retained_packed_events() {
        let members = 1_000_usize;
        let input = serde_json::json!({
            "type": "text-chunks",
            "seq0": 0,
            "time0": 0,
            "data": {
                "turn": 0,
                "step": 0,
                "index": 0,
                "dt": vec![1; members - 1],
                "texts": vec!["payload"; members],
            }
        })
        .to_string();
        reset_packed_event_builds();
        let mut events = Vec::new();

        visit_reader_from(input.as_bytes(), 998, &mut |event| {
            events.push(event);
            Ok(true)
        })
        .unwrap();

        assert_eq!(
            events
                .iter()
                .map(|event| event.seq.get())
                .collect::<Vec<_>>(),
            [998, 999]
        );
        assert_eq!(packed_event_builds(), 2);
    }

    #[test]
    fn nonpacked_scan_never_builds_chunk_events() {
        let packed = serde_json::json!({
            "type": "text-chunks",
            "seq0": 0,
            "time0": 0,
            "data": {
                "turn": 0,
                "step": 0,
                "index": 0,
                "dt": vec![1; 999],
                "texts": vec!["payload"; 1_000],
            }
        });
        let event = SessionEvent {
            type_: "user/message".into(),
            seq: dsh_session::SessionSeq::new(1_000).unwrap(),
            time: 1_000,
            data: serde_json::json!({"content": []}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let input = format!("{}\n{}", packed, serde_json::to_string(&event).unwrap());
        reset_packed_event_builds();
        let mut events = Vec::new();

        visit_nonpacked_reader(input.as_bytes(), &mut |event| {
            events.push(event);
            Ok(true)
        })
        .unwrap();

        assert_eq!(events, [event]);
        assert_eq!(packed_event_builds(), 0);
    }

    #[test]
    fn truncated_record_is_not_clean_eof() {
        let input = br#"{"type":"event","event":{"seq":0"#;
        let error = visit_reader(input.as_slice(), &mut |_| Ok(true)).unwrap_err();
        assert!(matches!(error, PackedStreamError::Invalid(_)));
    }
}
