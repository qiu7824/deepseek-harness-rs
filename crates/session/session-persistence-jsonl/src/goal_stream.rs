//! Selective streaming reader for the repository's canonical storage writer.
//! No frame index, whole-line buffer, event vector, or unrelated data Value is
//! built. Unsupported field ordering fails closed instead of taking a full-log
//! fallback. Data from a scan is authoritative only after the caller validates
//! the same physical source revision before and after it.

use std::cell::Cell;
use std::io::{BufReader, Read};
use std::path::Path;
use std::rc::Rc;

use dsh_session::{SessionEvent, SessionSeq};
use dsh_session_persistence::NonpackedEventVisitor;
use serde::Deserialize;
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, Visitor};
use serde_json::{Value, json};

use crate::format::JsonlCompression;

const MAX_GOAL_DATA_BYTES: u64 = 128 * 1024;
const MAX_TOKEN_BYTES: u64 = 1024;

#[derive(Default)]
struct ReadBudget {
    bytes: Cell<u64>,
    limit: Cell<Option<u64>>,
    record_started: Cell<bool>,
    non_whitespace: Cell<bool>,
}

struct Counted<R> {
    input: R,
    budget: Rc<ReadBudget>,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self
            .budget
            .limit
            .get()
            .is_some_and(|limit| self.budget.bytes.get() >= limit)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bounded goal JSON field exceeds its byte limit",
            ));
        }
        let available = self.budget.limit.get().map_or(buffer.len(), |limit| {
            (limit - self.budget.bytes.get()).min(buffer.len() as u64) as usize
        });
        let count = self.input.read(&mut buffer[..available])?;
        self.budget
            .bytes
            .set(self.budget.bytes.get() + count as u64);
        if buffer[..count]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            self.budget.non_whitespace.set(true);
        }
        Ok(count)
    }
}

struct FieldBudget<'a> {
    budget: &'a ReadBudget,
    previous: Option<u64>,
}
impl<'a> FieldBudget<'a> {
    fn new(budget: &'a ReadBudget, max: u64) -> Self {
        let previous = budget.limit.get();
        let next = budget.bytes.get().saturating_add(max);
        budget
            .limit
            .set(Some(previous.map_or(next, |limit| limit.min(next))));
        Self { budget, previous }
    }
}
impl Drop for FieldBudget<'_> {
    fn drop(&mut self) {
        self.budget.limit.set(self.previous);
    }
}

fn key<'de, M: MapAccess<'de>>(
    map: &mut M,
    budget: &ReadBudget,
) -> Result<Option<String>, M::Error> {
    let _limit = FieldBudget::new(budget, MAX_TOKEN_BYTES);
    map.next_key()
}
fn scalar<'de, M: MapAccess<'de>, T: Deserialize<'de>>(
    map: &mut M,
    budget: &ReadBudget,
) -> Result<T, M::Error> {
    let _limit = FieldBudget::new(budget, MAX_TOKEN_BYTES);
    map.next_value()
}

struct SourceSeed<'a> {
    budget: &'a ReadBudget,
}
impl<'de> DeserializeSeed<'de> for SourceSeed<'_> {
    type Value = Option<Value>;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_any(SourceVisitor {
            budget: self.budget,
        })
    }
}
struct SourceVisitor<'a> {
    budget: &'a ReadBudget,
}
impl<'de> Visitor<'de> for SourceVisitor<'_> {
    type Value = Option<Value>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a message source")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut kind, mut goal_id, mut revision, mut round) = (None, None, None, None);
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = key(&mut map, self.budget)? {
            let relevant = matches!(key.as_str(), "kind" | "goalId" | "revision" | "round");
            if relevant && !seen.insert(key.clone()) {
                return Err(M::Error::custom("duplicate goal source field"));
            }
            match key.as_str() {
                "kind" => kind = Some(scalar::<_, String>(&mut map, self.budget)?),
                "goalId" if kind.as_deref() != Some("user") => {
                    goal_id = Some(scalar::<_, Value>(&mut map, self.budget)?)
                }
                "revision" if kind.as_deref() != Some("user") => {
                    revision = Some(scalar::<_, Value>(&mut map, self.budget)?)
                }
                "round" if kind.as_deref() != Some("user") => {
                    round = Some(scalar::<_, Value>(&mut map, self.budget)?)
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        if kind.as_deref() != Some("goal") {
            return Ok(None);
        }
        let id = goal_id
            .as_ref()
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| M::Error::custom("invalid goal source identity"))?;
        let revision = revision
            .and_then(|value| value.as_u64())
            .filter(|value| *value > 0)
            .ok_or_else(|| M::Error::custom("invalid goal source revision"))?;
        let round = round
            .and_then(|value| value.as_u64())
            .filter(|value| *value > 0)
            .ok_or_else(|| M::Error::custom("invalid goal source round"))?;
        Ok(Some(
            json!({"kind":"goal","goalId":id,"revision":revision,"round":round}),
        ))
    }
}

struct UserDataSeed<'a> {
    budget: &'a ReadBudget,
}
impl<'de> DeserializeSeed<'de> for UserDataSeed<'_> {
    type Value = Option<Value>;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_map(UserDataVisitor {
            budget: self.budget,
        })
    }
}
struct UserDataVisitor<'a> {
    budget: &'a ReadBudget,
}
impl<'de> Visitor<'de> for UserDataVisitor<'_> {
    type Value = Option<Value>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a user message data object")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut source = None;
        let mut source_seen = false;
        while let Some(key) = key(&mut map, self.budget)? {
            if key == "source" {
                if source_seen {
                    return Err(M::Error::custom("duplicate message source"));
                }
                source_seen = true;
                source = map.next_value_seed(SourceSeed {
                    budget: self.budget,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(source.map(|source| json!({"source":source})))
    }
}

struct RecordSeed<'a> {
    budget: &'a ReadBudget,
}
impl<'de> DeserializeSeed<'de> for RecordSeed<'_> {
    type Value = Option<SessionEvent>;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_map(RecordVisitor {
            budget: self.budget,
        })
    }
}
struct RecordVisitor<'a> {
    budget: &'a ReadBudget,
}
impl<'de> Visitor<'de> for RecordVisitor<'_> {
    type Value = Option<SessionEvent>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a canonical session storage record")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        self.budget.record_started.set(true);
        if key(&mut map, self.budget)?.as_deref() != Some("type") {
            return Err(M::Error::custom(
                "bounded goal scan requires the canonical type-first storage record; no unbounded fallback is allowed",
            ));
        }
        let kind: String = scalar(&mut map, self.budget)?;
        let selected = matches!(kind.as_str(), "goal/change" | "user/message");
        let (mut seq, mut time, mut data) = (None, None, None);
        let mut data_seen = false;
        while let Some(key) = key(&mut map, self.budget)? {
            match key.as_str() {
                "type" => return Err(M::Error::custom("duplicate event type")),
                "seq" if selected => {
                    if seq.is_some() {
                        return Err(M::Error::custom("duplicate event sequence"));
                    }
                    seq = Some(scalar::<_, u64>(&mut map, self.budget)?);
                }
                "time" if selected => {
                    if time.is_some() {
                        return Err(M::Error::custom("duplicate event time"));
                    }
                    time = Some(scalar::<_, i64>(&mut map, self.budget)?);
                }
                "data" if selected => {
                    if data_seen {
                        return Err(M::Error::custom("duplicate event data"));
                    }
                    data_seen = true;
                    if kind == "goal/change" {
                        let _limit = FieldBudget::new(self.budget, MAX_GOAL_DATA_BYTES);
                        data = Some(map.next_value::<Value>()?);
                    } else {
                        data = map.next_value_seed(UserDataSeed {
                            budget: self.budget,
                        })?;
                    }
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let Some(data) = data else {
            if kind == "goal/change" {
                return Err(M::Error::custom("goal change is missing data"));
            }
            return Ok(None);
        };
        Ok(Some(SessionEvent {
            type_: kind,
            seq: SessionSeq::new(
                seq.ok_or_else(|| M::Error::custom("goal event is missing sequence"))?,
            )
            .map_err(M::Error::custom)?,
            time: time.ok_or_else(|| M::Error::custom("goal event is missing time"))?,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }))
    }
}

pub(crate) fn visit_reader<R: Read>(
    reader: R,
    visitor: &NonpackedEventVisitor,
) -> Result<(), String> {
    let budget = Rc::new(ReadBudget::default());
    let mut de = serde_json::Deserializer::from_reader(Counted {
        input: reader,
        budget: budget.clone(),
    });
    let mut prior = None;
    loop {
        budget.record_started.set(false);
        budget.non_whitespace.set(false);
        match (RecordSeed { budget: &budget }).deserialize(&mut de) {
            Ok(Some(event)) => {
                if prior.is_some_and(|prior| event.seq.get() <= prior) {
                    return Err("goal events are not in increasing sequence order".into());
                }
                prior = Some(event.seq.get());
                if !visitor(std::slice::from_ref(&event))? {
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(error)
                if error.is_eof()
                    && !budget.record_started.get()
                    && !budget.non_whitespace.get() =>
            {
                return Ok(());
            }
            Err(error) => return Err(format!("bounded goal scan failed: {error}")),
        }
    }
}

pub(crate) fn visit_path(
    path: &Path,
    compression: JsonlCompression,
    visitor: NonpackedEventVisitor,
) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    match compression {
        JsonlCompression::None => visit_reader(BufReader::with_capacity(16 * 1024, file), &visitor),
        JsonlCompression::Zstd => {
            let mut decoder =
                zstd::stream::read::Decoder::new(file).map_err(|error| error.to_string())?;
            decoder
                .window_log_max(super::MAX_AUTHORITY_WINDOW_LOG)
                .map_err(|error| error.to_string())?;
            visit_reader(BufReader::with_capacity(16 * 1024, decoder), &visitor)
        }
    }
}
