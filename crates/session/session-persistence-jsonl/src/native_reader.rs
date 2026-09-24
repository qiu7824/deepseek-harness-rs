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
                .write(bytes, |row| {
                    crate::v4_artifact::check_cancel(cancelled)?;
                    let row: SessionEvent =
                        serde_json::from_value(row).map_err(|error| error.to_string())?;
                    if row.type_ == "user/message" {
                        updated_at = Some(row.time);
                    }
                    blank &= row.type_ != "turn/start";
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

pub(crate) fn window(
    path: &Path,
    id: &str,
    request: dsh_session_persistence::SessionReadWindowRequest,
    cancelled: &impl Fn() -> bool,
) -> Result<dsh_session_persistence::SessionReadWindowResult, String> {
    use dsh_session_persistence::{SessionReadWindowResult, select_history_window};
    use std::collections::VecDeque;
    use std::io::Write;
    struct Size(usize);
    impl Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let capacity = request.max_events.saturating_add(1).max(2);
    let mut candidates = VecDeque::new();
    let mut bytes = 0_usize;
    let mut dropped = false;
    let summary = visit(path, id, cancelled, |event| {
        if event.seq.get() >= request.before_seq.unwrap_or(u64::MAX) {
            return Ok(());
        }
        let mut size = Size(0);
        serde_json::to_writer(&mut size, &event).map_err(|e| e.to_string())?;
        bytes = bytes.saturating_add(size.0);
        candidates.push_back((event, size.0));
        while candidates.len() > capacity || bytes > 64 * 1024 * 1024 && candidates.len() > 1 {
            let (_, removed) = candidates.pop_front().unwrap();
            bytes -= removed;
            dropped = true;
        }
        if bytes > 64 * 1024 * 1024 {
            return Err("history scan exceeds the 64 MiB source budget".into());
        }
        Ok(())
    })?;
    let candidates: Vec<_> = candidates.into_iter().map(|(event, _)| event).collect();
    let mut messages = request.max_messages.max(1);
    loop {
        match select_history_window(
            &candidates,
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
                let mut events = Vec::with_capacity(selection.event_count());
                events.extend(
                    candidates
                        .into_iter()
                        .skip(selection.start)
                        .take(selection.event_count()),
                );
                return Ok(SessionReadWindowResult {
                    meta: summary.meta,
                    events,
                    has_more: selection.has_more || dropped,
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
