//! On-disk format helpers for the JSONL session-persistence backend: path
//! sanitization, the per-project/session directory layout, header-line
//! (de)serialization, and the truncation-repair offset computation.
//! Rust port of `packages/session/session-persistence-jsonl/src/format.ts`.

use std::path::{Path, PathBuf};

use dsh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, decode_storage_record,
    pack_chunk_runs,
};
use dsh_session_persistence::session_format_version_refusal;

/// Physical encoding selected for JSONL session artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonlCompression {
    Zstd,
    None,
}

impl JsonlCompression {
    pub fn as_str(&self) -> &'static str {
        match self {
            JsonlCompression::Zstd => "zstd",
            JsonlCompression::None => "none",
        }
    }

    pub fn opposite(&self) -> Self {
        match self {
            JsonlCompression::Zstd => JsonlCompression::None,
            JsonlCompression::None => JsonlCompression::Zstd,
        }
    }
}

/// Return the artifact suffix for one physical encoding.
pub fn log_suffix(compression: JsonlCompression) -> &'static str {
    match compression {
        JsonlCompression::Zstd => ".jsonl.zstd",
        JsonlCompression::None => ".jsonl",
    }
}

/// The first JSONL record of a session artifact (TS `HeaderLine`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeaderLine {
    #[serde(rename = "type")]
    pub type_: String,
    pub version: u64,
    pub id: SessionId,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "parentSession"
    )]
    pub parent_session: Option<SessionId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "seedLength"
    )]
    pub seed_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(rename = "delegationDepth")]
    pub delegation_depth: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "agentPreset"
    )]
    pub agent_preset: Option<String>,
}

/// Build the header line object from a [`SessionHeader`] (TS
/// `toHeaderLine`).
pub fn to_header_line(header: &SessionHeader) -> HeaderLine {
    HeaderLine {
        type_: "session".to_string(),
        version: header.version,
        id: header.id.clone(),
        created_at: header.created_at,
        cwd: header.cwd.clone(),
        parent_session: header.parent_session.clone(),
        seed_length: header.seed_length,
        origin: header.origin.clone(),
        delegation_depth: header.delegation_depth.unwrap_or(0),
        agent_preset: header.agent_preset.clone(),
    }
}

/// Parse a header line back into a [`SessionHeader`] (TS `fromHeaderLine`).
pub fn from_header_line(line: &HeaderLine) -> SessionHeader {
    SessionHeader {
        version: line.version,
        id: line.id.clone(),
        created_at: line.created_at,
        cwd: line.cwd.clone(),
        parent_session: line.parent_session.clone(),
        seed_length: line.seed_length,
        origin: line.origin.clone(),
        delegation_depth: Some(line.delegation_depth),
        agent_preset: line.agent_preset.clone(),
    }
}

/// Whether a parsed first line is a well-formed session header (TS
/// `isHeaderLine`).
fn is_header_line(value: &serde_json::Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    record.get("type").and_then(|v| v.as_str()) == Some("session")
        && record.get("version").and_then(|v| v.as_u64()).is_some()
        && record.get("id").and_then(|v| v.as_str()).is_some()
        && record.get("createdAt").and_then(|v| v.as_u64()).is_some()
        && record
            .get("delegationDepth")
            .and_then(|v| v.as_u64())
            .is_some()
        && match record.get("origin") {
            None => true,
            Some(value) => value.as_str() == Some("subagent"),
        }
        && match record.get("agentPreset") {
            None => true,
            Some(value) => value.is_string(),
        }
}

/// Encode an arbitrary string as a single safe path segment, injectively
/// over all JS (UTF-16) strings (TS `encodeSegment`). Safe code units stay
/// literal; every other unit, including `~`, becomes `~XXXX`.
pub fn encode_segment(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("cannot encode an empty path segment".to_string());
    }
    if raw == "." {
        return Ok("~002E".to_string());
    }
    if raw == ".." {
        return Ok("~002E~002E".to_string());
    }
    let mut out = String::new();
    for unit in raw.encode_utf16() {
        let ch = char::from_u32(unit as u32).unwrap_or('\u{FFFD}');
        if ch != '~' && (ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) {
            out.push(ch);
        } else {
            out.push('~');
            out.push_str(&format!("{unit:04X}"));
        }
    }
    Ok(out)
}

/// Build the readable directory key for a project path (TS `projectKey`).
pub fn project_key(cwd: &str) -> Result<String, String> {
    if cwd.is_empty() {
        return Err("cannot encode an empty project path".to_string());
    }
    let mut readable = String::new();
    let mut separator_run = false;
    for unit in cwd.encode_utf16() {
        let ch = char::from_u32(unit as u32).unwrap_or('\u{FFFD}');
        if matches!(ch, '/' | '\\' | ':') {
            if !separator_run {
                readable.push('-');
            }
            separator_run = true;
        } else if ch != '~' && (ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) {
            readable.push(ch);
            separator_run = false;
        } else {
            readable.push('~');
            readable.push_str(&format!("{unit:04X}"));
            separator_run = false;
        }
    }
    let slug = readable.trim_start_matches('-');
    let slug = if slug.is_empty() { "root" } else { slug };
    let truncated: String = slug.chars().take(251).collect();
    Ok(format!("--{truncated}--"))
}

/// The configured root's human-navigable project directory.
pub fn project_dir(root: &str, cwd: Option<&str>) -> PathBuf {
    match cwd {
        None => Path::new(root).join("_no-cwd"),
        Some(cwd) => Path::new(root).join(project_key(cwd).expect("project key")),
    }
}

/// The directory owned by one session.
pub fn session_dir(root: &str, cwd: Option<&str>, id: &SessionId) -> PathBuf {
    project_dir(root, cwd).join(encode_segment(id.as_str()).expect("session id segment"))
}

/// The append-only event-log file path for a session.
pub fn log_path(
    root: &str,
    cwd: Option<&str>,
    id: &SessionId,
    compression: JsonlCompression,
) -> PathBuf {
    session_dir(root, cwd, id).join(format!("session{}", log_suffix(compression)))
}

/// Serialize an event batch as JSONL lines (no trailing newline)
/// (TS `eventLines`).
pub fn event_lines(events: &[SessionEvent], pack_chunks: bool) -> String {
    let lines: Vec<String> = if pack_chunks {
        pack_chunk_runs(events)
            .into_iter()
            .map(|record| record.to_json().to_string())
            .collect()
    } else {
        events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap_or_default())
            .collect()
    };
    lines.join("\n")
}

/// One complete session-log scan result.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLogScan {
    pub meta: SessionHeader,
    pub events: Vec<SessionEvent>,
    pub committed_bytes: usize,
}

/// Parse one complete header record supplied independently from event rows
/// (TS `parseHeaderRecord`).
fn parse_header_record(record: &[u8]) -> Result<SessionHeader, String> {
    if record.is_empty()
        || record.last() != Some(&0x0A)
        || record
            .iter()
            .take(record.len() - 1)
            .any(|byte| *byte == 0x0A)
    {
        return Err("empty or header-less session log".to_string());
    }
    let parsed: serde_json::Value = serde_json::from_slice(&record[..record.len() - 1])
        .map_err(|_| "corrupt session log: header line is not valid JSON".to_string())?;
    refuse_foreign_format_version(&parsed)?;
    if !is_header_line(&parsed) {
        return Err("corrupt session log: first line is not a session header".to_string());
    }
    let line: HeaderLine = serde_json::from_value(parsed)
        .map_err(|_| "corrupt session log: first line is not a session header".to_string())?;
    Ok(from_header_line(&line))
}

/// Refuse a header carrying a format version this build does not read.
fn refuse_foreign_format_version(parsed: &serde_json::Value) -> Result<(), String> {
    let Some(record) = parsed.as_object() else {
        return Ok(());
    };
    let Some(version) = record.get("version").and_then(|v| v.as_u64()) else {
        return Ok(());
    };
    if version == SESSION_FORMAT_VERSION {
        return Ok(());
    }
    let id = record
        .get("id")
        .and_then(|v| v.as_str())
        .map(dsh_session::session_id)
        .unwrap_or_else(|| dsh_session::session_id(String::new()));
    Err(session_format_version_refusal(&id, version))
}

/// Incrementally scan complete JSONL event records after an independently
/// supplied header record (TS `SessionLogScanner`).
pub struct SessionLogScanner {
    meta: SessionHeader,
    events: Vec<SessionEvent>,
    fragments: Vec<u8>,
    fragment_bytes: usize,
    input_bytes: usize,
    committed_bytes: usize,
    event_line: usize,
    issue: Option<String>,
    finished: bool,
}

impl SessionLogScanner {
    /// Create an event scanner from exactly one newline-terminated header
    /// record.
    pub fn new(header_record: &[u8]) -> Result<Self, String> {
        let meta = parse_header_record(header_record)?;
        Ok(Self {
            meta,
            events: Vec::new(),
            fragments: Vec::new(),
            fragment_bytes: 0,
            input_bytes: header_record.len(),
            committed_bytes: header_record.len(),
            event_line: 0,
            issue: None,
            finished: false,
        })
    }

    /// Consume the next raw plaintext chunk, retaining only an incomplete
    /// final record.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
        if self.finished {
            return Err("cannot write to a finished session log scanner".to_string());
        }
        let chunk_start = self.input_bytes;
        self.input_bytes += chunk.len();
        let mut line_start = 0usize;
        while let Some(relative) = chunk[line_start..].iter().position(|byte| *byte == 0x0A) {
            let newline = line_start + relative;
            let fragment = &chunk[line_start..newline];
            let line: Vec<u8>;
            if !self.fragments.is_empty() {
                if !fragment.is_empty() {
                    self.fragments.extend_from_slice(fragment);
                    self.fragment_bytes += fragment.len();
                }
                line = std::mem::take(&mut self.fragments);
                self.fragment_bytes = 0;
            } else {
                line = fragment.to_vec();
            }
            self.consume_event_line(&line, chunk_start + newline + 1)?;
            line_start = newline + 1;
        }
        if line_start < chunk.len() {
            self.fragments.extend_from_slice(&chunk[line_start..]);
            self.fragment_bytes += chunk.len() - line_start;
        }
        Ok(())
    }

    /// Snapshot progress before appending a recoverable torn-frame prefix.
    pub fn checkpoint(&self) -> ScannerCheckpoint {
        ScannerCheckpoint {
            input_bytes: self.input_bytes,
            committed_bytes: self.committed_bytes,
            event_count: self.events.len(),
        }
    }

    /// Finish scanning, ignoring a final record without a newline as a torn
    /// tail.
    pub fn finish(mut self) -> SessionLogScan {
        self.finished = true;
        SessionLogScan {
            meta: self.meta,
            events: self.events,
            committed_bytes: self.committed_bytes,
        }
    }

    /// Decode one complete event row and update the contiguous prefix
    /// (TS `consumeEventLine`; the `throw` sites surface as `Err`).
    fn consume_event_line(&mut self, line: &[u8], end_byte: usize) -> Result<(), String> {
        self.event_line += 1;
        let parsed: serde_json::Value = match serde_json::from_slice(line) {
            Ok(parsed) => parsed,
            Err(_) => {
                if self.issue.is_none() {
                    self.issue = Some(format!(
                        "corrupt session log: unparsable committed event at line {}",
                        self.event_line
                    ));
                }
                return Ok(());
            }
        };
        let decoded = match decode_storage_record(&parsed) {
            Ok(decoded) => decoded,
            Err(error) => {
                if self.issue.is_none() {
                    self.issue = Some(format!(
                        "corrupt session log: unparsable committed event at line {}: {error}",
                        self.event_line
                    ));
                }
                return Ok(());
            }
        };

        if let Some(issue) = &self.issue {
            if decoded.iter().any(|event| event.type_ == "turn/end") {
                return Err(issue.clone());
            }
            return Ok(());
        }

        let row_start = self.events.len();
        for event in decoded.iter() {
            if event.seq != self.events.len() as u64 {
                let expected = self.events.len();
                self.events.truncate(row_start);
                self.issue = Some(format!(
                    "corrupt session log: seq gap in committed region at line {} (expected {expected}, got {})",
                    self.event_line, event.seq
                ));
                // TS throws when the offending ROW contains a turn/end.
                if decoded
                    .iter()
                    .any(|candidate| candidate.type_ == "turn/end")
                {
                    return Err(self.issue.clone().expect("issue just set"));
                }
                return Ok(());
            }
            self.events.push(event.clone());
        }
        self.committed_bytes = end_byte;
        Ok(())
    }
}

/// The scanner checkpoint shape (TS `checkpoint()` return).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannerCheckpoint {
    pub input_bytes: usize,
    pub committed_bytes: usize,
    pub event_count: usize,
}

/// Parse a complete or torn JSONL buffer into its preserved event prefix
/// (TS `scanLog`).
pub fn scan_log(buffer: &[u8]) -> Result<SessionLogScan, String> {
    let Some(header_end) = buffer.iter().position(|byte| *byte == 0x0A) else {
        return Err("empty or header-less session log".to_string());
    };
    let mut scanner = SessionLogScanner::new(&buffer[..=header_end])?;
    scanner.write(&buffer[header_end + 1..])?;
    Ok(scanner.finish())
}

/// Parse just the header line of a log into a [`SessionHeader`], or `None`
/// if it is missing/not a header (TS `parseHeaderMeta`).
pub fn parse_header_meta(first_line: &str) -> Option<SessionHeader> {
    let parsed: serde_json::Value = serde_json::from_str(first_line).ok()?;
    if !is_header_line(&parsed) {
        return None;
    }
    let line: HeaderLine = serde_json::from_value(parsed).ok()?;
    Some(from_header_line(&line))
}
