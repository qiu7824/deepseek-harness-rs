//! Sequential V4 reads validate dense positions before expanding references.
use crate::{
    format::compression_of,
    v4_artifact::{ArtifactPart, visit_artifact_with_recovery},
};
use dsh_session::format_v4::{V4LogScanner, V4Recovery, V4Vocabulary};
use dsh_session::{SessionEvent, SessionHeader, SessionLogOffset};
use std::io::{Read, Seek, SeekFrom};
use std::{fs::File, path::Path};

pub(crate) async fn run<T: Send + 'static>(
    path: &Path,
    id: &dsh_session::SessionId,
    read: impl FnOnce(&Path, &str) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let path = path.to_owned();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || read(&path, &id))
        .await
        .map_err(|error| error.to_string())?
}

struct Snapshot<'a> {
    file: &'a mut File,
    length: u64,
    position: u64,
}
impl Read for Snapshot<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = buffer
            .len()
            .min(self.length.saturating_sub(self.position) as usize);
        let count = self.file.read(&mut buffer[..length])?;
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for Snapshot<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let target = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
            SeekFrom::End(value) => i128::from(self.length) + i128::from(value),
        };
        if target < 0 || target > i128::from(self.length) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek outside captured Session prefix",
            ));
        }
        self.position = self.file.seek(SeekFrom::Start(target as u64))?;
        Ok(self.position)
    }
}

pub(crate) struct NativeSummary {
    pub meta: SessionHeader,
    pub inherited: SessionLogOffset,
    pub event_count: u64,
    pub updated_at: i64,
    pub blank: bool,
    pub recovered_tail: bool,
}

/// Build a lossless indexed cold prefix with one decoded event in memory.
/// Torn physical tails retain the backend's established repair path.
pub(crate) fn prepare(
    path: &Path,
    expected_id: &str,
) -> Result<Option<dsh_session_persistence::StoredPreparation<crate::index::JsonlTornMarker>>, String> {
    let revision = crate::index::file_revision(
        &std::fs::metadata(path).map_err(|error| error.to_string())?,
    );
    let mut archive = dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir())?;
    let mut repair = dsh_session::repair::InterruptedTurnRepair::default();
    let summary = visit(path, expected_id, &|| false, |event| {
        repair.observe(&event);
        archive.push(&event)
    })?;
    if summary.recovered_tail {
        return Ok(None);
    }
    let archive = archive.finish()?;
    let mut validator = dsh_session::format_v4::V4Validator::new(
        serde_json::to_value(&summary.meta).map_err(|error| error.to_string())?,
        summary.inherited.get(),
    )?;
    archive.visit(0..archive.len(), |event| {
        validator
            .push(&serde_json::to_value(event).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid V4 lifecycle at {} {}: {error}", event.seq, event.type_))?;
        Ok(true)
    })?;
    validator.finish()?;
    let closers = repair.finish();
    let inspection_length = archive.len() + closers.len();
    let session = dsh_session::Session::from_event_archive(
        dsh_session::session_id(expected_id),
        archive,
        &summary.meta,
        summary.inherited,
        closers.clone(),
    )?;
    if revision
        != crate::index::file_revision(
            &std::fs::metadata(path).map_err(|error| error.to_string())?,
        )
    {
        return Err("Session source changed during native restore".into());
    }
    Ok(Some(dsh_session_persistence::StoredPreparation {
        session,
        inspection_length,
        revision,
        torn_marker: None,
        closers,
    }))
}

pub(crate) fn is_native(path: &Path) -> Result<bool, String> {
    let line = crate::index::read_authority_header(path, compression_of(path))?
        .ok_or("missing Session header")?;
    let value: serde_json::Value =
        serde_json::from_str(&line).map_err(|error| error.to_string())?;
    Ok(value["version"].as_f64() == Some(4.0))
}

/// Callbacks are provisional until the complete, revision-stable scan succeeds.
pub(crate) fn visit(
    path: &Path,
    expected_id: &str,
    cancelled: &impl Fn() -> bool,
    mut event: impl FnMut(SessionEvent) -> Result<(), String>,
) -> Result<NativeSummary, String> {
    visit_physical(path, expected_id, cancelled, |row| {
        event(logical_event(row)?)
    })
}

/// The scanner has already validated density, reference bounds and integer
/// endpoints. Expand directly into the logical u64 vector, never a JSON array.
fn logical_event(mut row: serde_json::Value) -> Result<SessionEvent, String> {
    let references = row.as_object_mut().unwrap().remove("sourceEventSeqs");
    let mut event: SessionEvent = serde_json::from_value(row).map_err(|error| error.to_string())?;
    if let Some(serde_json::Value::Array(values)) = references {
        let count = values.iter().try_fold(0_usize, |total, value| {
            let length = value.as_array().map_or(1, |range| {
                range[1].as_u64().unwrap() - range[0].as_u64().unwrap() + 1
            });
            total
                .checked_add(usize::try_from(length).map_err(|_| "source range size overflow")?)
                .ok_or("source range size overflow")
        })?;
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(count)
            .map_err(|_| "cannot allocate source references")?;
        for value in values {
            if let Some(range) = value.as_array() {
                sources.extend(range[0].as_u64().unwrap()..=range[1].as_u64().unwrap());
            } else {
                sources.push(value.as_u64().unwrap());
            }
        }
        event.source_event_seqs = Some(sources);
    }
    Ok(event)
}

fn visit_physical(
    path: &Path,
    expected_id: &str,
    cancelled: &impl Fn() -> bool,
    mut event: impl FnMut(serde_json::Value) -> Result<(), String>,
) -> Result<NativeSummary, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    let mut snapshot = Snapshot {
        file: &mut file,
        length: before.len(),
        position: 0,
    };
    let mut scanner = None;
    let mut updated_at = None;
    let mut blank = true;
    let framing = visit_artifact_with_recovery(
        &mut snapshot,
        compression_of(path),
        cancelled,
        true,
        |part| match part {
            ArtifactPart::Header(header) => {
                scanner = Some(V4LogScanner::new(
                    header,
                    V4Recovery::RecoverableTail,
                    V4Vocabulary::default(),
                )?);
                Ok(())
            }
            ArtifactPart::Body(bytes) => scanner
                .as_mut()
                .ok_or("missing native Session header")?
                .write_with_physical_references(bytes, |row| {
                    crate::v4_artifact::check_cancel(cancelled)?;
                    // Metadata-only passes must retain the same envelope
                    // admission as materializing a SessionEvent.
                    if let Some(surface) = row.get("surfaceOp").filter(|value| !value.is_null()) {
                        <dsh_session::SurfaceOp as serde::Deserialize>::deserialize(surface)
                            .map_err(|error| error.to_string())?;
                    }
                    if row["type"] == "user/message" {
                        updated_at = row["time"].as_i64();
                    }
                    blank &= row["type"] != "turn/start";
                    event(row)
                }),
        },
    )?;
    drop(snapshot);
    let after = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let handle_after = file.metadata().map_err(|error| error.to_string())?;
    let changed = crate::index::file_revision(&before) != crate::index::file_revision(&after);
    let appended = after.len() > before.len()
        && before.created().ok() == after.created().ok()
        && crate::index::file_revision(&handle_after) == crate::index::file_revision(&after);
    if changed && !appended {
        return Err("Session source changed during native history read".into());
    }
    let scan = scanner.ok_or("missing native Session header")?.finish()?;
    let recovered_tail = framing.recovered_tail || scan.accepted_bytes != scan.input_bytes;
    let decoded = scan.decoded;
    let meta: SessionHeader =
        serde_json::from_value(decoded.header).map_err(|error| error.to_string())?;
    if meta.id.as_str() != expected_id {
        return Err("Session artifact identity mismatch".into());
    }
    Ok(NativeSummary {
        updated_at: updated_at.unwrap_or(meta.created_at as i64),
        meta,
        inherited: SessionLogOffset::new(decoded.inherited_event_count)?,
        event_count: decoded.event_count,
        blank,
        recovered_tail,
    })
}

impl NativeSummary {
    pub(crate) fn list_metadata(self) -> dsh_session_persistence::SessionListMetadata {
        dsh_session_persistence::SessionListMetadata {
            meta: self.meta,
            inherited_event_count: self.inherited,
            last_seq: self.event_count as i64 - 1,
            blank: self.blank,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryKind {
    Other,
    User,
    Assistant,
    Chunk,
    TurnStart,
    TurnEnd,
    StepStart,
    ToolCall,
    ToolResult,
}

impl HistoryKind {
    fn new(kind: &str) -> Self {
        match kind {
            "user/message" => Self::User,
            "assistant/message" => Self::Assistant,
            "assistant/chunk" => Self::Chunk,
            "turn/start" => Self::TurnStart,
            "turn/end" => Self::TurnEnd,
            "step/start" => Self::StepStart,
            "tool/call" => Self::ToolCall,
            "tool/result" => Self::ToolResult,
            _ => Self::Other,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Other => "",
            Self::User => "user/message",
            Self::Assistant => "assistant/message",
            Self::Chunk => "assistant/chunk",
            Self::TurnStart => "turn/start",
            Self::TurnEnd => "turn/end",
            Self::StepStart => "step/start",
            Self::ToolCall => "tool/call",
            Self::ToolResult => "tool/result",
        }
    }
}

/// Only fields inspected by the shared history selector. In particular, a
/// completed message may cite a much earlier chunk, so payloads cannot safely
/// be discarded at a message boundary during a single forward read.
#[derive(Clone, Copy)]
struct HistoryBoundary {
    seq: u64,
    source: u64,
    turn: u64,
    kind: HistoryKind,
    surface: Option<bool>,
    has_source: bool,
    has_turn: bool,
    failed: bool,
}

impl HistoryBoundary {
    #[cfg(test)]
    fn new(event: &SessionEvent) -> Self {
        let kind = HistoryKind::new(&event.type_);
        let source = event
            .source_event_seqs
            .as_ref()
            .and_then(|sources| sources.iter().copied().min());
        let turn = event.data.get("turn").and_then(serde_json::Value::as_u64);
        Self {
            seq: event.seq.get(),
            source: source.unwrap_or(0),
            turn: turn.unwrap_or(0),
            kind,
            surface: event.surface_op.as_ref().map(|op| op.is_append()),
            has_source: source.is_some(),
            has_turn: turn.is_some(),
            failed: event
                .data
                .get("reason")
                .and_then(|reason| reason.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("error"),
        }
    }

    fn from_physical(event: &serde_json::Value) -> Self {
        let source = event
            .get("sourceEventSeqs")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| {
                        value.as_array().map_or_else(
                            || value.as_u64().unwrap(),
                            |range| range[0].as_u64().unwrap(),
                        )
                    })
                    .min()
            });
        let turn = event["data"]
            .get("turn")
            .and_then(serde_json::Value::as_u64);
        Self {
            seq: event["seq"].as_u64().unwrap(),
            source: source.unwrap_or(0),
            turn: turn.unwrap_or(0),
            kind: HistoryKind::new(event["type"].as_str().unwrap()),
            surface: event
                .get("surfaceOp")
                .filter(|value| !value.is_null())
                .map(|value| value == "append"),
            has_source: source.is_some(),
            has_turn: turn.is_some(),
            failed: event["data"]
                .get("reason")
                .and_then(|reason| reason.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("error"),
        }
    }
}

impl dsh_session_persistence::HistoryWindowEvent for HistoryBoundary {
    fn history_seq(&self) -> u64 {
        self.seq
    }
    fn history_type(&self) -> &str {
        self.kind.as_str()
    }
    fn history_surface_append(&self) -> Option<bool> {
        self.surface
    }
    fn history_source_start(&self) -> Option<u64> {
        self.has_source.then_some(self.source)
    }
    fn history_turn(&self) -> Option<u64> {
        self.has_turn.then_some(self.turn)
    }
    fn history_failed(&self) -> bool {
        self.failed
    }
}

pub(crate) fn window(
    path: &Path,
    id: &str,
    request: dsh_session_persistence::SessionReadWindowRequest,
    cancelled: &impl Fn() -> bool,
) -> Result<dsh_session_persistence::SessionReadWindowResult, String> {
    window_with_sink(path, id, request, cancelled, None)
}

pub(crate) fn window_with_sink(
    path: &Path,
    id: &str,
    request: dsh_session_persistence::SessionReadWindowRequest,
    cancelled: &impl Fn() -> bool,
    sink: Option<Box<dyn dsh_session_persistence::HistoryWindowSink>>,
) -> Result<dsh_session_persistence::SessionReadWindowResult, String> {
    use dsh_session_persistence::{SessionReadWindowResult, select_history_window};
    use std::collections::VecDeque;
    let capacity = request.max_events.saturating_add(1).max(2);
    let revision =
        crate::index::file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
    let mut candidates = VecDeque::new();
    let mut dropped = false;
    let summary = visit_physical(path, id, cancelled, |event| {
        if event["seq"].as_u64().unwrap() >= request.before_seq.unwrap_or(u64::MAX) {
            return Ok(());
        }
        if candidates.len() == capacity {
            candidates.pop_front();
            dropped = true;
        } else if candidates.len() == candidates.capacity() {
            // Geometric growth must stop at the requested bound, including
            // the extra witness row when max_events is a power of two.
            let extra = (capacity - candidates.len()).min(candidates.len().max(4));
            candidates.reserve_exact(extra);
        }
        candidates.push_back(HistoryBoundary::from_physical(&event));
        Ok(())
    })?;
    candidates.make_contiguous();
    let mut messages = request.max_messages.max(1);
    loop {
        match select_history_window(
            candidates.as_slices().0,
            request.before_seq,
            messages,
            request.max_events,
        ) {
            Ok(selection) if dropped && selection.start == 0 => {
                return Ok(SessionReadWindowResult {
                    meta: summary.meta,
                    events: vec![],
                    has_more: true,
                    oversized_event_count: Some(capacity),
                });
            }
            Ok(selection) => {
                let count = selection.event_count();
                let range = candidates.as_slices().0[selection.start..selection.end]
                    .first()
                    .zip(candidates.as_slices().0[selection.start..selection.end].last())
                    .map(|(first, last)| (first.seq, last.seq));
                drop(candidates);
                let (events, reduced) =
                    materialize_range(path, id, range, count, &revision, cancelled, true, sink)?;
                return Ok(SessionReadWindowResult {
                    meta: summary.meta,
                    events,
                    has_more: selection.has_more || dropped || reduced,
                    oversized_event_count: None,
                });
            }
            Err(error) if messages > 1 => {
                let required = error.selection.event_count().max(1);
                messages = (messages.saturating_mul(request.max_events as u64) / required as u64)
                    .max(1)
                    .min(messages - 1);
            }
            Err(error) => {
                return Ok(SessionReadWindowResult {
                    meta: summary.meta,
                    events: vec![],
                    has_more: error.selection.has_more || dropped,
                    oversized_event_count: Some(error.selection.event_count()),
                });
            }
        }
    }
}

pub(crate) fn forward_window_with_sink(
    path: &Path,
    id: &str,
    request: dsh_session_persistence::SessionReadForwardWindowRequest,
    cancelled: &impl Fn() -> bool,
    sink: Option<Box<dyn dsh_session_persistence::HistoryWindowSink>>,
) -> Result<dsh_session_persistence::SessionReadWindowResult, String> {
    let revision =
        crate::index::file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
    let mut count = 0;
    let mut messages = 0;
    let mut range: Option<(u64, u64)> = None;
    let mut has_more = false;
    let summary = visit_physical(path, id, cancelled, |event| {
        let seq = event["seq"].as_u64().unwrap();
        if seq < request.after_seq {
            return Ok(());
        }
        if count >= request.max_events || messages >= request.max_messages.max(1) {
            has_more = true;
            return Ok(());
        }
        let boundary = HistoryBoundary::from_physical(&event);
        messages += u64::from(
            matches!(boundary.kind, HistoryKind::User | HistoryKind::Assistant)
                && boundary.surface.unwrap_or(true),
        );
        count += 1;
        range = Some((range.map_or(seq, |range| range.0), seq));
        Ok(())
    })?;
    // Forward readers historically apply their consumer's source budget;
    // only backwards native windows impose the raw serialized-event limit.
    let (events, reduced) =
        materialize_range(path, id, range, count, &revision, cancelled, false, sink)?;
    Ok(dsh_session_persistence::SessionReadWindowResult {
        meta: summary.meta,
        events,
        has_more: has_more || reduced,
        oversized_event_count: None,
    })
}

fn materialize_range(
    path: &Path,
    id: &str,
    range: Option<(u64, u64)>,
    count: usize,
    revision: &dsh_session_persistence::SessionPersistenceRevision,
    cancelled: &impl Fn() -> bool,
    raw_byte_budget: bool,
    sink: Option<Box<dyn dsh_session_persistence::HistoryWindowSink>>,
) -> Result<(Vec<SessionEvent>, bool), String> {
    let scan = |consume: &mut dyn FnMut(SessionEvent) -> Result<(), String>| -> Result<(), String> {
        struct Size(usize);
        impl std::io::Write for Size {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut seen = 0;
        let mut bytes = 0_usize;
        if let Some((first, last)) = range {
            visit_physical(path, id, cancelled, |row| {
                let seq = row["seq"].as_u64().unwrap();
                if seq < first || seq > last {
                    return Ok(());
                }
                let event = logical_event(row)?;
                if raw_byte_budget {
                    let mut size = Size(0);
                    serde_json::to_writer(&mut size, &event).map_err(|error| error.to_string())?;
                    bytes = bytes.saturating_add(size.0);
                    if bytes > 64 * 1024 * 1024 {
                        return Err("history scan exceeds the 64 MiB source budget".into());
                    }
                }
                seen += 1;
                consume(event)
            })?;
        }
        if &crate::index::file_revision(
            &std::fs::metadata(path).map_err(|error| error.to_string())?,
        ) != revision
            || seen != count
        {
            return Err("Session source changed between native history scans".into());
        }
        Ok(())
    };
    if let Some(mut sink) = sink {
        scan(&mut |event| sink.inspect(&event))?;
        scan(&mut |event| sink.push(event))?;
        sink.finish()
    } else {
        let mut events = Vec::with_capacity(count);
        scan(&mut |event| {
            events.push(event);
            Ok(())
        })?;
        Ok((events, false))
    }
}

#[cfg(test)]
mod tests;
