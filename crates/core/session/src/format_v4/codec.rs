use super::{
    V4Vocabulary,
    admission::{admit_v4_structural_fields, validate_v4_row_fields},
    wire::{MAX_SAFE_INTEGER, count, object, validate_header},
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V4Recovery {
    Strict,
    RecoverableTail,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V4DecodeIssue {
    pub row: u64,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct V4DecodeSummary {
    pub header: Value,
    pub event_count: u64,
    pub inherited_event_count: u64,
    pub recovered_tail: Option<V4DecodeIssue>,
}

fn signed(value: &Value, field: &str) -> Result<i64, String> {
    value
        .as_i64()
        .filter(|n| n.unsigned_abs() <= MAX_SAFE_INTEGER)
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| {
                    n.is_finite()
                        && n.abs() <= MAX_SAFE_INTEGER as f64
                        && n.fract() == 0.0
                        && (*n != 0.0 || !n.is_sign_negative())
                })
                .map(|n| n as i64)
        })
        .ok_or_else(|| format!("{field} must be a safe integer"))
}

pub fn decode_v4_header(mut physical: Value) -> Result<Value, String> {
    let record = physical
        .as_object_mut()
        .ok_or("V4 physical header must be an object")?;
    if record.remove("type") != Some(json!("session")) {
        return Err("V4 physical header requires type session".into());
    }
    validate_header(&physical, 4)?;
    for key in ["version", "createdAt", "delegationDepth"] {
        physical[key] = json!(count(&physical[key], key)?);
    }
    Ok(physical)
}
pub fn encode_v4_header(mut header: Value, inherited_event_count: u64) -> Result<Value, String> {
    validate_header(&header, 4)?;
    if inherited_event_count > MAX_SAFE_INTEGER
        || header["isSeeded"] != true && inherited_event_count != 0
    {
        return Err("invalid V4 inherited count".into());
    }
    header["type"] = json!("session");
    Ok(header)
}

fn envelope(row: &Value) -> Result<u64, String> {
    let record = object(row, "V4 row")?;
    for key in ["type", "seq", "time", "data"] {
        if !record.contains_key(key) {
            return Err(format!("V4 row lacks {key}"));
        }
    }
    if record.keys().any(|key| {
        ![
            "type",
            "seq",
            "time",
            "data",
            "ignorable",
            "surfaceOp",
            "sourceEventSeqs",
        ]
        .contains(&key.as_str())
    }) {
        return Err("V4 row has an unknown envelope field".into());
    }
    if !row["type"].is_string() || record.get("ignorable").is_some_and(|v| v != true) {
        return Err("invalid V4 row type or ignorable marker".into());
    }
    signed(&row["time"], "V4 row time")?;
    count(&row["seq"], "V4 row seq")
}

fn native_admission(row: &Value, vocabulary: &V4Vocabulary, writer: bool) -> Result<(), String> {
    let Some(kind) = row.get("type").and_then(Value::as_str) else {
        return Ok(());
    };
    if !vocabulary.contains(kind) && row["ignorable"] != true {
        return Err(format!("unknown required V4 event {kind}"));
    }
    if kind == "developer/message"
        && row["ignorable"] == true
        && !vocabulary.contains(kind)
        && !writer
    {
        return Ok(());
    }
    admit_v4_structural_fields(row)
}

fn validate_refs(values: &[Value], seq: u64) -> Result<(bool, u64), String> {
    if values.len() as u64 > seq {
        return Err("source references exceed the accepted prefix".into());
    }
    let has_range = values.iter().any(Value::is_array);
    if !has_range {
        let mut seen = Vec::new();
        seen.try_reserve_exact(values.len())
            .map_err(|_| "cannot allocate source reference index")?;
        for value in values {
            let source = count(value, "source reference")?;
            if source >= seq {
                return Err("source reference must precede its row".into());
            }
            seen.push(source);
        }
        seen.sort_unstable();
        if seen.windows(2).any(|w| w[0] == w[1]) {
            return Err("duplicate source reference".into());
        }
        return Ok((false, values.len() as u64));
    }
    let mut total = 0u64;
    let mut last = None;
    for entry in values {
        let (start, end) = if let Some(range) = entry.as_array() {
            if range.len() != 2 {
                return Err("source range must be a start/end pair".into());
            }
            (
                count(&range[0], "source range start")?,
                count(&range[1], "source range end")?,
            )
        } else {
            let value = count(entry, "source reference")?;
            (value, value)
        };
        if start > end || end >= seq || last.is_some_and(|old| start <= old) {
            return Err("source ranges must be ordered unique earlier positions".into());
        }
        total = total
            .checked_add(end - start + 1)
            .filter(|n| *n <= seq)
            .ok_or("source range expansion exceeds its accepted prefix")?;
        last = Some(end);
    }
    Ok((true, total))
}

fn decode_refs(mut values: Vec<Value>, seq: u64) -> Result<Vec<Value>, String> {
    let (has_range, total) = validate_refs(&values, seq)?;
    if !has_range {
        for value in &mut values {
            *value = json!(count(value, "source reference")?);
        }
        return Ok(values);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(usize::try_from(total).map_err(|_| "source range size overflow")?)
        .map_err(|_| "cannot allocate expanded source references")?;
    for entry in values {
        if let Some(range) = entry.as_array() {
            let start = count(&range[0], "range start")?;
            let end = count(&range[1], "range end")?;
            for value in start..=end {
                output.push(json!(value));
            }
        } else {
            output.push(json!(count(&entry, "source reference")?));
        }
    }
    Ok(output)
}

fn decode_row(mut row: Value, expected: u64) -> Result<Value, String> {
    let seq = envelope(&row)?;
    // Validate density before any range expansion; the bound comes from rows
    // actually accepted, never from an attacker-controlled enormous seq field.
    if seq != expected {
        return Err(format!(
            "V4 row sequence gap: expected {expected}, got {seq}"
        ));
    }
    if let Some(values) = row.as_object_mut().unwrap().remove("sourceEventSeqs") {
        let values = match values {
            Value::Array(values) => values,
            _ => return Err("sourceEventSeqs must be an array".into()),
        };
        row["sourceEventSeqs"] = Value::Array(decode_refs(values, seq)?);
    }
    row["seq"] = json!(seq);
    row["time"] = json!(signed(&row["time"], "event time")?);
    Ok(row)
}

pub fn encode_v4_event(mut row: Value, vocabulary: &V4Vocabulary) -> Result<Value, String> {
    native_admission(&row, vocabulary, true)?;
    let seq = envelope(&row)?;
    if vocabulary.contains(row["type"].as_str().unwrap()) {
        validate_v4_row_fields(&row)?;
    }
    let Some(value) = row.as_object_mut().unwrap().remove("sourceEventSeqs") else {
        return Ok(row);
    };
    let values = match value {
        Value::Array(values) => values,
        _ => return Err("sourceEventSeqs must be an array".into()),
    };
    if values.iter().any(Value::is_array) {
        return Err("logical source references must not contain physical ranges".into());
    }
    let values = decode_refs(values, seq)?;
    if values
        .windows(2)
        .any(|w| w[0].as_u64().unwrap() >= w[1].as_u64().unwrap())
    {
        row["sourceEventSeqs"] = Value::Array(values);
        return Ok(row);
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let start = values[index].as_u64().unwrap();
        let mut end = start;
        while index + 1 < values.len() && values[index + 1].as_u64() == Some(end + 1) {
            index += 1;
            end += 1;
        }
        if end - start >= 2 {
            output.push(json!([start, end]));
        } else {
            output.push(json!(start));
            if end != start {
                output.push(json!(end));
            }
        }
        index += 1;
    }
    row["sourceEventSeqs"] = Value::Array(output);
    Ok(row)
}

/// Incremental physical decoder. Accepted rows remain provisional until finish
/// and native logical relationship validation both succeed. The decoder keeps
/// no body history and continues hard admission after a recoverable row issue.
pub struct V4Decoder {
    header: Value,
    recovery: V4Recovery,
    vocabulary: V4Vocabulary,
    next_seq: u64,
    row_index: u64,
    inherited: Option<u64>,
    issue: Option<V4DecodeIssue>,
    failed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct V4LogScan {
    pub decoded: V4DecodeSummary,
    pub input_bytes: u64,
    pub accepted_bytes: u64,
    pub torn_tail_bytes: usize,
}

/// Byte-stream framing around V4Decoder. Event payloads are delivered to the
/// caller and never collected here; an incomplete final line remains a tail.
pub struct V4LogScanner {
    decoder: V4Decoder,
    fragment: Vec<u8>,
    input_bytes: u64,
    accepted_bytes: u64,
    failed: bool,
}
impl V4LogScanner {
    pub fn new(
        header_line: &[u8],
        recovery: V4Recovery,
        vocabulary: V4Vocabulary,
    ) -> Result<Self, String> {
        if header_line.last() != Some(&b'\n')
            || header_line[..header_line.len() - 1].contains(&b'\n')
        {
            return Err("Session header must be one complete newline-terminated record".into());
        }
        let header = serde_json::from_slice(&header_line[..header_line.len() - 1])
            .map_err(|e| format!("invalid Session header JSON: {e}"))?;
        let decoder = V4Decoder::with_vocabulary(header, recovery, vocabulary)?;
        Ok(Self {
            decoder,
            fragment: vec![],
            input_bytes: header_line.len() as u64,
            accepted_bytes: header_line.len() as u64,
            failed: false,
        })
    }
    pub fn write(
        &mut self,
        chunk: &[u8],
        mut event: impl FnMut(Value) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.failed {
            return Err("V4 scanner already failed".into());
        }
        let result = (|| {
            let offset = self.input_bytes;
            self.input_bytes = self
                .input_bytes
                .checked_add(chunk.len() as u64)
                .ok_or("Session byte offset overflow")?;
            let mut start = 0;
            while let Some(relative) = chunk[start..].iter().position(|b| *b == b'\n') {
                let end = start + relative;
                let decoded = if self.fragment.is_empty() {
                    self.decoder.decode_json_line(&chunk[start..end])
                } else {
                    self.fragment.extend_from_slice(&chunk[start..end]);
                    let decoded = self.decoder.decode_json_line(&self.fragment);
                    self.fragment.clear();
                    if self.fragment.capacity() > 64 * 1024 {
                        self.fragment = vec![];
                    }
                    decoded
                };
                if let Some(row) = decoded? {
                    event(row)?;
                    self.accepted_bytes = offset + end as u64 + 1;
                }
                start = end + 1;
            }
            self.fragment.extend_from_slice(&chunk[start..]);
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn finish(self) -> Result<V4LogScan, String> {
        if self.failed {
            return Err("V4 scanner failed; staged output cannot be published".into());
        }
        // Never inspect or execute an unterminated fragment as a complete row.
        let torn_tail_bytes = self.fragment.len();
        if torn_tail_bytes > 0 && self.decoder.recovery == V4Recovery::Strict {
            return Err("strict V4 scan has an incomplete final record".into());
        }
        Ok(V4LogScan {
            decoded: self.decoder.finish()?,
            input_bytes: self.input_bytes,
            accepted_bytes: self.accepted_bytes,
            torn_tail_bytes,
        })
    }
}
impl V4Decoder {
    pub fn new(physical_header: Value, recovery: V4Recovery) -> Result<Self, String> {
        Self::with_vocabulary(physical_header, recovery, V4Vocabulary::default())
    }
    pub fn with_vocabulary(
        physical_header: Value,
        recovery: V4Recovery,
        vocabulary: V4Vocabulary,
    ) -> Result<Self, String> {
        Ok(Self {
            header: decode_v4_header(physical_header)?,
            recovery,
            vocabulary,
            next_seq: 0,
            row_index: 0,
            inherited: None,
            issue: None,
            failed: false,
        })
    }
    pub fn header(&self) -> &Value {
        &self.header
    }
    pub fn decode_json_line(&mut self, line: &[u8]) -> Result<Option<Value>, String> {
        if self.failed {
            return Err("V4 decoder already refused the generation".into());
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(row) => self.decode_row(row),
            Err(error) => {
                let row = self.row_index;
                self.row_index += 1;
                self.recover(row, format!("invalid JSON row: {error}"))
            }
        }
    }
    pub fn decode_row(&mut self, row: Value) -> Result<Option<Value>, String> {
        if self.failed {
            return Err("V4 decoder already refused the generation".into());
        }
        let index = self.row_index;
        self.row_index += 1;
        if let Err(error) = native_admission(&row, &self.vocabulary, false) {
            self.failed = true;
            return Err(error);
        }
        let closing = row.get("type").and_then(Value::as_str) == Some("turn/end");
        // Framing remains checked after a suffix issue. A complete later
        // turn/end proves the corrupt span was not merely an unfinished tail.
        if self.issue.is_some() {
            let seq = match envelope(&row) {
                Ok(seq) => seq,
                Err(error) => return self.recover(index, error),
            };
            if let Some(refs) = row.get("sourceEventSeqs") {
                let result = refs
                    .as_array()
                    .ok_or_else(|| "sourceEventSeqs must be an array".to_string())
                    .and_then(|values| validate_refs(values, seq));
                if let Err(error) = result {
                    return self.recover(index, error);
                }
            }
            if closing {
                let error = self.issue.as_ref().unwrap().message.clone();
                self.failed = true;
                return Err(error);
            }
            // The suffix is not emitted, so even a valid enormous declared
            // range is checked arithmetically and never expanded.
            return Ok(None);
        }
        let row = match decode_row(row, self.next_seq) {
            Ok(row) => row,
            Err(error) => return self.recover(index, error),
        };
        if row["type"] == "session/end-seed" && row["data"]["inherited"] == true {
            self.inherited = Some(self.next_seq);
        }
        self.next_seq += 1;
        Ok(Some(row))
    }
    fn recover(&mut self, index: u64, error: String) -> Result<Option<Value>, String> {
        if self.recovery == V4Recovery::Strict {
            self.failed = true;
            return Err(error);
        }
        self.issue.get_or_insert(V4DecodeIssue {
            row: index,
            message: error,
        });
        Ok(None)
    }
    pub fn finish(self) -> Result<V4DecodeSummary, String> {
        if self.failed {
            return Err("V4 decoder refused the generation; retain the source".into());
        }
        if self.header["isSeeded"] == true && self.inherited.is_none()
            || self.header["isSeeded"] != true && self.inherited.is_some()
        {
            return Err("V4 header has no matching accepted inherited marker".into());
        }
        Ok(V4DecodeSummary {
            header: self.header,
            event_count: self.next_seq,
            inherited_event_count: self.inherited.unwrap_or(0),
            recovered_tail: self.issue,
        })
    }
}
