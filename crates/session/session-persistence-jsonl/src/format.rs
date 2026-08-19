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
        loop {
            let Some(relative) = chunk[line_start..].iter().position(|byte| *byte == 0x0A) else {
                break;
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::session_id;

    #[test]
    fn segment_encoding_round_trips_safe_and_special() {
        assert_eq!(encode_segment("abc_1.X-y").unwrap(), "abc_1.X-y");
        assert_eq!(encode_segment(".").unwrap(), "~002E");
        assert_eq!(encode_segment("..").unwrap(), "~002E~002E");
        // TS special-cases only the exact `.` / `..` segments; dots in
        // longer segments stay literal (traversal is still neutralized
        // because separators escape).
        assert_eq!(encode_segment("../etc").unwrap(), "..~002Fetc");
        assert_eq!(encode_segment("a b").unwrap(), "a~0020b");
        assert_eq!(encode_segment("til~de").unwrap(), "til~007Ede");
        // Non-ASCII code units escape; lone surrogates cannot exist in a
        // Rust `str` (UTF-8), so the TS lone-surrogate support is a
        // documented deviation.
        assert_eq!(encode_segment("中").unwrap(), "~4E2D");
        assert!(encode_segment("").is_err());
    }

    #[test]
    fn project_key_layout() {
        assert_eq!(project_key("C:\\work\\repo").unwrap(), "--C-work-repo--");
        assert_eq!(project_key("/a//b").unwrap(), "--a-b--");
        assert_eq!(project_key("///").unwrap(), "--root--");
        assert_eq!(project_key("a b").unwrap(), "--a~0020b--");
        assert!(project_key("").is_err());
    }

    #[test]
    fn directory_layout() {
        let root = "root";
        assert_eq!(project_dir(root, None), Path::new("root").join("_no-cwd"));
        assert_eq!(
            session_dir(root, Some("C:\\work"), &session_id("a/b")),
            Path::new("root").join("--C-work--").join("a~002Fb")
        );
        assert_eq!(
            log_path(root, None, &session_id("s1"), JsonlCompression::Zstd),
            Path::new("root")
                .join("_no-cwd")
                .join("s1")
                .join("session.jsonl.zstd")
        );
        assert_eq!(log_suffix(JsonlCompression::None), ".jsonl");
        assert_eq!(log_suffix(JsonlCompression::Zstd), ".jsonl.zstd");
    }

    #[test]
    fn header_line_round_trip() {
        let header = SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 42,
            cwd: Some("C:\\work".to_string()),
            parent_session: Some(session_id("p")),
            seed_length: Some(3),
            origin: Some("subagent".to_string()),
            delegation_depth: Some(2),
            agent_preset: Some("preset".to_string()),
        };
        let line = to_header_line(&header);
        let json = serde_json::to_value(&line).unwrap();
        assert_eq!(json["type"], "session");
        assert_eq!(json["delegationDepth"], 2);
        assert_eq!(json["cwd"], "C:\\work");
        assert_eq!(from_header_line(&line), header);

        // Absent optionals are omitted, delegationDepth defaults to 0.
        let minimal = SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s2"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let json = serde_json::to_value(to_header_line(&minimal)).unwrap();
        assert_eq!(json["delegationDepth"], 0);
        assert_eq!(
            from_header_line(&to_header_line(&minimal)).delegation_depth,
            Some(0)
        );
    }

    fn event(seq: u64, type_: &str) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: 0,
            data: serde_json::json!({"turn": 1}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[test]
    fn event_lines_pack_and_verbatim() {
        let events = vec![event(0, "turn/start"), event(1, "turn/end")];
        let lines = event_lines(&events, false);
        assert_eq!(lines.lines().count(), 2);
        let packed = event_lines(&events, true);
        assert_eq!(
            packed, lines,
            "non-chunk events are identical in both layouts"
        );
    }

    #[test]
    fn scanner_commits_complete_records_and_keeps_torn_tail() {
        let header = serde_json::to_string(&to_header_line(&SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }))
        .unwrap();
        let mut bytes = format!("{header}\n").into_bytes();
        for seq in 0..3 {
            bytes.extend_from_slice(
                format!(
                    "{}\n",
                    serde_json::to_string(&event(seq, "turn/start")).unwrap()
                )
                .as_bytes(),
            );
        }
        let torn = format!("{}", serde_json::to_string(&event(3, "turn/end")).unwrap());
        bytes.extend_from_slice(torn.as_bytes());

        let scan = scan_log(&bytes).unwrap();
        assert_eq!(scan.meta.id.as_str(), "s1");
        assert_eq!(scan.events.len(), 3, "the torn final record is excluded");
        assert_eq!(scan.committed_bytes, bytes.len() - torn.len());
    }

    #[test]
    fn scanner_rejects_seq_gap_in_committed_region() {
        let header = serde_json::to_string(&to_header_line(&SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }))
        .unwrap();
        let mut bytes = format!("{header}\n").into_bytes();
        bytes.extend_from_slice(
            format!(
                "{}\n",
                serde_json::to_string(&event(0, "turn/start")).unwrap()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(
            format!(
                "{}\n",
                serde_json::to_string(&event(5, "turn/end")).unwrap()
            )
            .as_bytes(),
        );
        let error = scan_log(&bytes).unwrap_err();
        assert!(
            error.contains("seq gap in committed region at line 2"),
            "{error}"
        );
    }

    #[test]
    fn scanner_truncates_at_seq_gap_without_turn_end() {
        let header = serde_json::to_string(&to_header_line(&SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }))
        .unwrap();
        let mut bytes = format!("{header}\n").into_bytes();
        let first = format!(
            "{}\n",
            serde_json::to_string(&event(0, "turn/start")).unwrap()
        );
        bytes.extend_from_slice(first.as_bytes());
        bytes.extend_from_slice(
            format!(
                "{}\n",
                serde_json::to_string(&event(5, "turn/start")).unwrap()
            )
            .as_bytes(),
        );
        // A gap before any turn/end keeps the committed prefix silently
        // (TS tolerates this: only a turn/end row throws).
        let scan = scan_log(&bytes).unwrap();
        assert_eq!(scan.events.len(), 1, "committed prefix survives the gap");
        assert_eq!(scan.committed_bytes, header.len() + 1 + first.len());
    }

    #[test]
    fn scanner_header_refusals() {
        assert_eq!(parse_header_meta("not json"), None);
        assert_eq!(parse_header_meta("{\"type\":\"event\"}"), None);
        let header = serde_json::to_string(&to_header_line(&SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }))
        .unwrap();
        let parsed = parse_header_meta(&header).unwrap();
        assert_eq!(parsed.id.as_str(), "s1");

        // A future format version refuses.
        let future = serde_json::json!({
            "type": "session",
            "version": SESSION_FORMAT_VERSION + 1,
            "id": "s1",
            "createdAt": 1,
            "delegationDepth": 0,
        });
        let refusal = scan_log(format!("{}\n", serde_json::to_string(&future).unwrap()).as_bytes())
            .unwrap_err();
        assert!(refusal.contains("written by a newer harness"), "{refusal}");
    }

    #[test]
    fn scanner_accepts_storage_rows() {
        // packChunkRuns output decodes through the scanner.
        let chunks: Vec<SessionEvent> = (0..4)
            .map(|index| SessionEvent {
                type_: "assistant/chunk".to_string(),
                seq: index,
                time: 1000 + index as i64,
                data: serde_json::json!({
                    "turn": 1, "step": 1,
                    "chunk": {"type": "text-delta", "index": 0, "text": format!("t{index}")},
                }),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            })
            .collect();
        let packed = event_lines(&chunks, true);
        let header = serde_json::to_string(&to_header_line(&SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }))
        .unwrap();
        let bytes = format!("{header}\n{packed}\n").into_bytes();
        let scan = scan_log(&bytes).unwrap();
        assert_eq!(scan.events.len(), 4, "packed rows expand back to events");
        assert_eq!(scan.events[3].data["chunk"]["text"], "t3");
    }
}
