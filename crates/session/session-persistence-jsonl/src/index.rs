//! JSONL durable session-persistence backend. Rust port of
//! `packages/session/session-persistence-jsonl/src/index.ts`.
//!
//! # Deviations
//!
//! - Windows write-through namespace operations (the koffi `win32.ts`
//!   integration) are replaced by the portable temp-write + `hard_link`
//!   publish path (`link()` fails with EEXIST when the final path exists, so
//!   concurrent materialization cannot clobber — the TS POSIX semantics on
//!   both platforms).
//! - The file revision omits `dev`/`ino` where the platform cannot expose
//!   them; it is `size:mtimeNs:ctimeNs` (source-qualified within one root).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cordis::{Context, Service};
use dsh_session::{
    SessionEvent, SessionHeader, SessionId, SessionPreparation, StorageRecord,
    decode_storage_record, visit_decoded_storage_record_tail,
};
use dsh_session_persistence::{
    PersistenceBackend, PersistenceCoordinator, PersistenceCoordinatorOptions,
    SessionReadWindowRequest, SessionReadWindowResult, StoredPrefix,
};
use parking_lot::Mutex;

use crate::format::{
    JsonlCompression, SessionLogScanner, event_lines, log_path, log_suffix, parse_header_meta,
    project_dir, scan_log, session_dir, to_header_line,
};
use crate::zstd::{
    compress_zstd_frame, decode_zstd_frames, decompress_zstd_frame, decompress_zstd_prefix,
    scan_zstd_frames,
};

const DEFAULT_PACK_CHUNKS: bool = true;
const DEFAULT_COMPRESSION: JsonlCompression = JsonlCompression::Zstd;

/// Plugin config: where the JSONL backend keeps its session logs, and the
/// packed-row write switch.
#[derive(Debug, Clone)]
pub struct JsonlConfig {
    /// Root directory for all session files. Required (no default).
    pub root: String,
    /// Pack consecutive delta-chunk runs into storage rows.
    pub pack_chunks: bool,
    /// Physical encoding.
    pub compression: JsonlCompression,
    /// Maximum cold Session preparations retained.
    pub prepared_session_cache_size: usize,
    /// Fixed live-event coalescing window.
    pub write_batch_max_delay_ms: u64,
}

impl Default for JsonlConfig {
    fn default() -> Self {
        Self {
            root: String::new(),
            pack_chunks: DEFAULT_PACK_CHUNKS,
            compression: DEFAULT_COMPRESSION,
            prepared_session_cache_size: 5,
            write_batch_max_delay_ms: 200,
        }
    }
}

/// Parse a loader-supplied JSON config (root required).
pub fn parse_config(value: &serde_json::Value) -> Result<JsonlConfig, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "jsonl config must be an object".to_string())?;
    let root = object
        .get("root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "jsonl config root is required".to_string())?
        .to_string();
    let pack_chunks = match object.get("packChunks") {
        None | Some(serde_json::Value::Null) => DEFAULT_PACK_CHUNKS,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| "packChunks must be a boolean".to_string())?,
    };
    let compression = match object.get("compression") {
        None | Some(serde_json::Value::Null) => DEFAULT_COMPRESSION,
        Some(serde_json::Value::String(v)) if v == "zstd" => JsonlCompression::Zstd,
        Some(serde_json::Value::String(v)) if v == "none" => JsonlCompression::None,
        Some(_) => return Err("compression must be \"zstd\" or \"none\"".to_string()),
    };
    let prepared_session_cache_size = object
        .get("preparedSessionCacheSize")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as usize;
    let write_batch_max_delay_ms = object
        .get("writeBatchMaxDelayMs")
        .and_then(|v| v.as_u64())
        .unwrap_or(200);
    Ok(JsonlConfig {
        root,
        pack_chunks,
        compression,
        prepared_session_cache_size,
        write_batch_max_delay_ms,
    })
}

/// Opaque coordinator token for replacing bytes recovered from a torn frame.
#[derive(Clone)]
pub struct JsonlTornMarker {
    pub truncate_to: u64,
    pub recovered_events: Vec<SessionEvent>,
}

/// Build the source-qualified revision shared by full and lightweight reads.
fn file_revision(
    metadata: &std::fs::Metadata,
) -> dsh_session_persistence::SessionPersistenceRevision {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        dsh_session_persistence::session_persistence_revision(format!(
            "{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime_nsec(),
            metadata.ctime_nsec()
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let modified = metadata.last_write_time();
        let created = metadata.creation_time();
        dsh_session_persistence::session_persistence_revision(format!(
            "{}:{}:{}",
            metadata.len(),
            modified,
            created
        ))
    }
}

fn stream_zstd_events(
    path: &Path,
    mut on_event: impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let before = file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    // SAFETY: read-only mapping of a revision-checked snapshot candidate.
    let mapping =
        unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|error| error.to_string())?;
    let scan = scan_zstd_frames(&mapping)?;
    if scan.frames.is_empty() {
        return Err("empty or header-less Zstandard session log".to_string());
    }
    for frame in &scan.frames[1..] {
        if !visit_zstd_frame_events(&mapping[frame.start..frame.end], &mut on_event)? {
            let after = file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
            if before != after {
                return Err("session artifact changed during streaming read".to_string());
            }
            return Ok(());
        }
    }
    let after = file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
    if before != after {
        return Err("session artifact changed during streaming read".to_string());
    }
    Ok(())
}

fn visit_zstd_frame_events(
    frame: &[u8],
    on_event: &mut impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    let decoder = zstd::stream::read::Decoder::new(frame).map_err(|error| error.to_string())?;
    match crate::packed_stream::visit_frame(decoder, on_event) {
        Ok(accepted) => Ok(accepted),
        Err(crate::packed_stream::PackedStreamError::Noncanonical(_, emitted, accepting)) => {
            let decoder =
                zstd::stream::read::Decoder::new(frame).map_err(|error| error.to_string())?;
            crate::packed_stream::fallback_visit_skip(decoder, emitted, accepting, on_event)
        }
        Err(crate::packed_stream::PackedStreamError::Invalid(message)) => Err(message),
    }
}

fn visit_zstd_frame_tail(
    frame: &[u8],
    capacity: usize,
    on_event: &mut impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    let decoder = zstd::stream::read::Decoder::new(frame).map_err(|error| error.to_string())?;
    match crate::packed_stream::visit_tail_reader(decoder, capacity, on_event) {
        Ok(accepted) => Ok(accepted),
        Err(crate::packed_stream::PackedStreamError::Noncanonical(_, emitted, mut accepting)) => {
            let decoder =
                zstd::stream::read::Decoder::new(frame).map_err(|error| error.to_string())?;
            let records =
                serde_json::Deserializer::from_reader(decoder).into_iter::<StorageRecord>();
            let mut skip = emitted;
            for record in records {
                let record =
                    record.map_err(|error| format!("invalid JSONL event record: {error}"))?;
                if accepting
                    && !visit_decoded_storage_record_tail(record, capacity, &mut |event| {
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
        Err(crate::packed_stream::PackedStreamError::Invalid(message)) => Err(message),
    }
}

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// The JSONL persistence backend (registers as `ctx.sessionPersistence`).
pub struct JsonlSessionPersistence {
    ctx: Context,
    root: PathBuf,
    pack_chunks: bool,
    compression: JsonlCompression,
    coordinator: Mutex<Option<Arc<PersistenceCoordinator<JsonlTornMarker>>>>,
    root_encoding_check: tokio::sync::OnceCell<Result<(), String>>,
}

impl JsonlSessionPersistence {
    /// Create the backend, register the service, and build the coordinator.
    pub fn install(ctx: &Context, config: JsonlConfig) -> Result<Arc<Self>, String> {
        if config.root.is_empty() {
            return Err("jsonl config root is required".to_string());
        }
        let root = std::path::absolute(&config.root).map_err(|error| error.to_string())?;
        let backend = Arc::new(Self {
            ctx: ctx.clone(),
            root,
            pack_chunks: config.pack_chunks,
            compression: config.compression,
            coordinator: Mutex::new(None),
            root_encoding_check: tokio::sync::OnceCell::new(),
        });
        // Register the ERASED service shape: the session-query seam and the
        // schedule/corpus consumers observe `Arc<dyn SessionPersistenceApi>`.
        let erased: Arc<dyn dsh_session_persistence::SessionPersistenceApi> = backend.clone();
        ctx.register_service(erased);
        let coordinator = PersistenceCoordinator::new(
            ctx,
            backend.clone(),
            PersistenceCoordinatorOptions {
                prepared_session_cache_size: config.prepared_session_cache_size,
                write_batch_max_delay_ms: config.write_batch_max_delay_ms,
            },
        );
        *backend.coordinator.lock() = Some(coordinator);
        Ok(backend)
    }

    fn coordinator(&self) -> Arc<PersistenceCoordinator<JsonlTornMarker>> {
        self.coordinator
            .lock()
            .as_ref()
            .expect("coordinator installed")
            .clone()
    }

    fn opposite_compression(&self) -> JsonlCompression {
        self.compression.opposite()
    }

    async fn ensure_root_encoding(&self) -> Result<(), String> {
        let result = self
            .root_encoding_check
            .get_or_init(|| async { self.check_root_encoding().await })
            .await;
        result.clone()
    }

    async fn check_root_encoding(&self) -> Result<(), String> {
        for project in self.list_project_dirs().await? {
            for dir in self.list_session_dirs(&project).await? {
                let incompatible = dir.join(format!(
                    "session{}",
                    log_suffix(self.opposite_compression())
                ));
                if self.exists(&incompatible).await {
                    return Err(self.encoding_mismatch(&incompatible));
                }
            }
        }
        Ok(())
    }

    fn encoding_mismatch(&self, path: &Path) -> String {
        format!(
            "session artifact {} uses {}, but this backend is configured for compression {}; use a separate root or select the matching compression mode",
            serde_json::to_string(&path.to_string_lossy().to_string()).unwrap_or_default(),
            log_suffix(self.opposite_compression()),
            serde_json::to_string(self.compression.as_str()).unwrap_or_default(),
        )
    }

    fn legacy_layout(&self, path: &Path) -> String {
        format!(
            "session artifact {} uses the unsupported flat-file layout; use a separate root or move it into a project/session directory before loading",
            serde_json::to_string(&path.to_string_lossy().to_string()).unwrap_or_default(),
        )
    }

    async fn exists(&self, path: &Path) -> bool {
        match tokio::fs::metadata(path).await {
            Ok(_) => true,
            Err(error) if is_not_found(&error) => false,
            Err(_) => false,
        }
    }

    async fn list_project_dirs(&self) -> Result<Vec<PathBuf>, String> {
        let mut read = match tokio::fs::read_dir(&self.root).await {
            Ok(read) => read,
            Err(error) if is_not_found(&error) => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut dirs = Vec::new();
        while let Some(entry) = read.next_entry().await.map_err(|e| e.to_string())? {
            if entry.file_type().await.map_err(|e| e.to_string())?.is_dir() {
                dirs.push(entry.path());
            }
        }
        Ok(dirs)
    }

    async fn list_session_dirs(&self, project: &Path) -> Result<Vec<PathBuf>, String> {
        let mut read = tokio::fs::read_dir(project)
            .await
            .map_err(|e| e.to_string())?;
        let mut dirs = Vec::new();
        while let Some(entry) = read.next_entry().await.map_err(|e| e.to_string())? {
            let file_type = entry.file_type().await.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if file_type.is_file() && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zstd")) {
                return Err(self.legacy_layout(&entry.path()));
            }
            if file_type.is_dir() {
                dirs.push(entry.path());
            }
        }
        Ok(dirs)
    }

    /// Find the unique physical log for an id across every project
    /// directory.
    async fn find_log(&self, id: &SessionId) -> Result<Option<PathBuf>, String> {
        let mut matches = Vec::new();
        for project in self.list_project_dirs().await? {
            self.reject_legacy_flat_artifact(&project, id).await?;
            let dir =
                project.join(crate::format::encode_segment(id.as_str()).map_err(|error| {
                    format!("corrupt session log: header id cannot name a storage path ({error})")
                })?);
            let path = dir.join(format!("session{}", log_suffix(self.compression)));
            let opposite = dir.join(format!(
                "session{}",
                log_suffix(self.opposite_compression())
            ));
            if self.exists(&opposite).await {
                return Err(self.encoding_mismatch(&opposite));
            }
            if self.exists(&path).await {
                matches.push(path);
            }
        }
        if matches.len() > 1 {
            return Err(format!(
                "duplicate JSONL session id \"{}\" appears in multiple project directories",
                id.as_str()
            ));
        }
        Ok(matches.pop())
    }

    async fn reject_legacy_flat_artifact(
        &self,
        project: &Path,
        id: &SessionId,
    ) -> Result<(), String> {
        let encoded =
            crate::format::encode_segment(id.as_str()).map_err(|error| error.to_string())?;
        for compression in [JsonlCompression::Zstd, JsonlCompression::None] {
            let path = project.join(format!("{encoded}{}", log_suffix(compression)));
            if self.exists(&path).await {
                return Err(self.legacy_layout(&path));
            }
        }
        Ok(())
    }

    /// Read a file's bytes under a revision-stable loop.
    async fn read_stable_file(
        &self,
        path: &Path,
    ) -> Result<(Vec<u8>, dsh_session_persistence::SessionPersistenceRevision), String> {
        loop {
            let before_meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
            let before = file_revision(&before_meta);
            let buffer = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
            let after_meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
            let after = file_revision(&after_meta);
            if before == after {
                return Ok((buffer, after));
            }
        }
    }

    /// Read a stored prefix and convert torn-tail state to the opaque
    /// marker.
    async fn read_prefix(
        &self,
        path: &Path,
        expected_id: Option<&SessionId>,
    ) -> Result<StoredPrefix<JsonlTornMarker>, String> {
        let (buffer, revision) = self.read_stable_file(path).await?;
        let mut prefix = if self.compression == JsonlCompression::Zstd {
            self.read_zstd_prefix(&buffer).map_err(|error| {
                if error.contains("corrupt") || error.contains("newer harness") {
                    format!("{error} (raw log: {})", path.to_string_lossy())
                } else {
                    error
                }
            })?
        } else {
            let scan = scan_log(&buffer)
                .map_err(|error| format!("{error} (raw log: {})", path.to_string_lossy()))?;
            StoredPrefix::<JsonlTornMarker> {
                meta: scan.meta,
                events: scan.events,
                revision: revision.clone(),
                torn_marker: if scan.committed_bytes < buffer.len() {
                    Some(JsonlTornMarker {
                        truncate_to: scan.committed_bytes as u64,
                        recovered_events: Vec::new(),
                    })
                } else {
                    None
                },
            }
        };
        prefix.revision = revision;
        self.assert_stored_identity(path, &prefix.meta, expected_id)
            .await?;
        Ok(prefix)
    }

    fn decode_zstd_event_frame(&self, frame: &[u8]) -> Result<Vec<SessionEvent>, String> {
        let plaintext = decompress_zstd_frame(frame)?;
        let text = std::str::from_utf8(&plaintext)
            .map_err(|error| format!("stored event frame is not UTF-8: {error}"))?;
        let mut events = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: serde_json::Value = serde_json::from_str(line)
                .map_err(|error| format!("stored event frame is not valid JSON: {error}"))?;
            events.extend(
                decode_storage_record(&record)
                    .map_err(|error| format!("stored event frame is invalid: {error}"))?,
            );
        }
        Ok(events)
    }

    async fn read_zstd_window(
        &self,
        path: &Path,
        id: &SessionId,
        request: SessionReadWindowRequest,
    ) -> Result<Option<SessionReadWindowResult>, String> {
        let before = file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        // SAFETY: read-only mapping of the revision-checked artifact.
        let mapping =
            unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|error| error.to_string())?;
        let scan = scan_zstd_frames(&mapping)?;
        if scan.frames.is_empty() || scan.torn_start.is_some() {
            return Ok(None);
        }
        let header = scan.frames[0];
        let header_plaintext = decompress_zstd_frame(&mapping[header.start..header.end])?;
        let header_line = decode_zstd_header_line_single(&header_plaintext)?;
        let meta = parse_header_meta(&header_line)
            .ok_or_else(|| "invalid Zstandard session header".to_string())?;
        self.assert_stored_identity(path, &meta, Some(id)).await?;
        let before_seq = request.before_seq.unwrap_or(u64::MAX);
        let capacity = request.max_events.saturating_add(1).max(2);
        let mut candidates = VecDeque::with_capacity(capacity);
        let mut dropped = false;
        for (frame_index, frame) in scan.frames[1..].iter().enumerate().rev() {
            let mut frame_tail = VecDeque::with_capacity(capacity);
            let mut frame_dropped = false;
            visit_zstd_frame_tail(&mapping[frame.start..frame.end], capacity, &mut |event| {
                if event.seq >= before_seq {
                    return Ok(true);
                }
                frame_tail.push_back(event);
                if frame_tail.len() > capacity {
                    frame_tail.pop_front();
                    frame_dropped = true;
                }
                Ok(true)
            })?;
            for event in frame_tail.into_iter().rev() {
                candidates.push_front(event);
                if candidates.len() > capacity {
                    candidates.pop_front();
                    dropped = true;
                }
            }
            if frame_dropped {
                dropped = true;
            }
            if candidates.len() >= capacity {
                dropped |= frame_index > 0;
                break;
            }
        }
        let after = file_revision(&std::fs::metadata(path).map_err(|error| error.to_string())?);
        if before != after {
            return Err("session artifact changed during history read".to_string());
        }
        let candidates: Vec<SessionEvent> = candidates.into_iter().collect();
        let mut messages = request.max_messages.max(1);
        loop {
            match dsh_session_persistence::select_history_window(
                &candidates,
                Some(before_seq),
                messages,
                request.max_events,
            ) {
                Ok(selection) if dropped && selection.start == 0 => {
                    return Ok(Some(SessionReadWindowResult {
                        meta,
                        events: Vec::new(),
                        has_more: true,
                        oversized_event_count: Some(capacity),
                    }));
                }
                Ok(selection) => {
                    let events = candidates[selection.start..selection.end].to_vec();
                    return Ok(Some(SessionReadWindowResult {
                        meta,
                        events,
                        has_more: selection.has_more || dropped,
                        oversized_event_count: None,
                    }));
                }
                Err(error) if messages > 1 => {
                    let required = error.selection.event_count().max(1);
                    let proportional = messages
                        .saturating_mul(request.max_events as u64)
                        .checked_div(required as u64)
                        .unwrap_or(1)
                        .max(1);
                    messages = proportional.min(messages - 1);
                }
                Err(error) => {
                    return Ok(Some(SessionReadWindowResult {
                        meta,
                        events: Vec::new(),
                        has_more: error.selection.has_more,
                        oversized_event_count: Some(error.selection.event_count()),
                    }));
                }
            }
        }
    }

    /// Decode complete frames and retain complete JSONL records from a torn
    /// final frame.
    fn read_zstd_prefix(&self, buffer: &[u8]) -> Result<StoredPrefix<JsonlTornMarker>, String> {
        let scan = scan_zstd_frames(buffer)?;
        if scan.frames.is_empty() {
            return Err("empty or header-less Zstandard session log".to_string());
        }
        let plaintexts = decode_zstd_frames(buffer, &scan.frames)?;
        let mut frames = plaintexts.into_iter();
        let header_plaintext = frames
            .next()
            .ok_or_else(|| "empty or header-less Zstandard session log".to_string())?;
        // Validate the header line (errors propagate); the scanner below
        // re-parses it.
        decode_zstd_header_line_single(&header_plaintext)?;
        let mut scanner = SessionLogScanner::new(&header_plaintext)?;
        for plaintext in frames {
            scanner.write(&plaintext)?;
        }
        let complete = scanner.checkpoint();
        if complete.committed_bytes != complete.input_bytes {
            return Err(
                "corrupt Zstandard session log: complete frame contains a torn JSONL record"
                    .to_string(),
            );
        }
        if scan.torn_start.is_none() {
            let finished = scanner.finish();
            return Ok(StoredPrefix {
                meta: finished.meta,
                events: finished.events,
                revision: dsh_session_persistence::session_persistence_revision(""),
                torn_marker: None,
            });
        }
        let torn_start = scan.torn_start.expect("torn start");
        let recovered_plaintext = decompress_zstd_prefix(&buffer[torn_start..]);
        scanner.write(&recovered_plaintext)?;
        let recovered = scanner.finish();
        let recovered_events = recovered.events[complete.event_count..].to_vec();
        Ok(StoredPrefix {
            meta: recovered.meta,
            events: recovered.events,
            revision: dsh_session_persistence::session_persistence_revision(""),
            torn_marker: Some(JsonlTornMarker {
                truncate_to: torn_start as u64,
                recovered_events,
            }),
        })
    }

    /// Reject metadata that does not identify the selected physical log.
    async fn assert_stored_identity(
        &self,
        path: &Path,
        meta: &SessionHeader,
        expected_id: Option<&SessionId>,
    ) -> Result<(), String> {
        if let Some(expected_id) = expected_id
            && meta.id != *expected_id
        {
            return Err(format!(
                "corrupt session log \"{}\": requested id \"{}\" does not match header id \"{}\"",
                path.to_string_lossy(),
                expected_id.as_str(),
                meta.id.as_str()
            ));
        }
        let expected_path = log_path(
            &self.root.to_string_lossy(),
            meta.cwd.as_deref(),
            &meta.id,
            self.compression,
        );
        let same = path == expected_path
            || match (
                std::fs::canonicalize(path),
                std::fs::canonicalize(&expected_path),
            ) {
                (Ok(actual), Ok(expected)) => actual == expected,
                _ => false,
            };
        if !same {
            return Err(format!(
                "corrupt session log \"{}\": header id \"{}\" and cwd identify \"{}\"",
                path.to_string_lossy(),
                meta.id.as_str(),
                expected_path.to_string_lossy()
            ));
        }
        Ok(())
    }

    // ---- file mechanics ----

    async fn existing_materialization_matches(
        &self,
        path: &Path,
        expected_content: &[u8],
    ) -> Result<bool, String> {
        let (content, _) = self.read_stable_file(path).await?;
        Ok(content == expected_content)
    }

    async fn materialize(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
    ) -> Result<(), String> {
        let project = project_dir(&self.root.to_string_lossy(), meta.cwd.as_deref());
        let dir = session_dir(&self.root.to_string_lossy(), meta.cwd.as_deref(), &meta.id);
        let final_path = log_path(
            &self.root.to_string_lossy(),
            meta.cwd.as_deref(),
            &meta.id,
            self.compression,
        );
        let opposite = log_path(
            &self.root.to_string_lossy(),
            meta.cwd.as_deref(),
            &meta.id,
            self.opposite_compression(),
        );
        if self.exists(&opposite).await {
            return Err(self.encoding_mismatch(&opposite));
        }
        let content = self.encode_materialization(meta, events)?;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&project)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        if self.exists(&final_path).await {
            if self
                .existing_materialization_matches(&final_path, &content)
                .await?
            {
                return Ok(());
            }
            return Err(format!(
                "refusing to materialize \"{}\": a different log already exists on disk (load/resume it instead)",
                meta.id.as_str()
            ));
        }
        let tmp = self.write_synced_temp_file(&final_path, &content).await?;
        // Publish via link()+unlink(): link fails with EEXIST when the final
        // path already exists, so concurrent materialization cannot clobber.
        let tmp_for_link = tmp.clone();
        let final_for_link = final_path.clone();
        let linked =
            tokio::task::spawn_blocking(move || std::fs::hard_link(&tmp_for_link, &final_for_link))
                .await
                .map_err(|error| error.to_string())?;
        match linked {
            Ok(()) => {}
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                if self.exists(&final_path).await
                    && self
                        .existing_materialization_matches(&final_path, &content)
                        .await?
                {
                    return Ok(());
                }
                return Err(format!(
                    "failed to publish materialized session \"{}\" at \"{}\": {error}",
                    meta.id.as_str(),
                    final_path.display()
                ));
            }
        }
        let _ = tokio::fs::remove_file(&tmp).await;
        Ok(())
    }

    async fn write_synced_temp_file(
        &self,
        final_path: &Path,
        content: &[u8],
    ) -> Result<PathBuf, String> {
        let tmp = final_path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        // create_new: never clobber a temp collision.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await
            .map_err(|e| e.to_string())?;
        tokio::io::AsyncWriteExt::write_all(&mut file, content)
            .await
            .map_err(|e| e.to_string())?;
        file.sync_all().await.map_err(|e| e.to_string())?;
        drop(file);
        Ok(tmp)
    }

    fn encode_materialization(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
    ) -> Result<Vec<u8>, String> {
        let header = format!(
            "{}\n",
            serde_json::to_string(&to_header_line(meta))
                .map_err(|e| format!("header is not JSON-serializable: {e}"))?
        );
        let body = format!("{}\n", event_lines(events, self.pack_chunks));
        if self.compression == JsonlCompression::None {
            return Ok(format!("{header}{body}").into_bytes());
        }
        let header_frame = compress_zstd_frame(header.as_bytes())?;
        let event_frame = compress_zstd_frame(body.as_bytes())?;
        let mut concat = header_frame;
        concat.extend_from_slice(&event_frame);
        Ok(concat)
    }

    async fn append_lines(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
    ) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let content = self.encode_event_batch(events)?;
        let path = log_path(
            &self.root.to_string_lossy(),
            meta.cwd.as_deref(),
            &meta.id,
            self.compression,
        );
        let before = tokio::fs::metadata(&path)
            .await
            .map_err(|e| e.to_string())?
            .len();
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(|e| e.to_string())?;
        let write_result = async {
            tokio::io::AsyncWriteExt::write_all(&mut file, &content)
                .await
                .map_err(|e| e.to_string())?;
            file.sync_all().await.map_err(|e| e.to_string())
        }
        .await;
        drop(file);
        match write_result {
            Ok(()) => Ok(()),
            Err(error) => {
                // Restore the previous size so the retried batch cannot
                // duplicate sequence numbers.
                match self.rollback_append(&path, before).await {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(format!(
                        "failed to roll back append to \"{}\": {error}; rollback also failed: {rollback}",
                        path.to_string_lossy()
                    )),
                }
            }
        }
    }

    fn encode_event_batch(&self, events: &[SessionEvent]) -> Result<Vec<u8>, String> {
        let body = format!("{}\n", event_lines(events, self.pack_chunks));
        if self.compression == JsonlCompression::Zstd {
            compress_zstd_frame(body.as_bytes())
        } else {
            Ok(body.into_bytes())
        }
    }

    async fn rollback_append(&self, path: &Path, size: u64) -> Result<(), String> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.set_len(size).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn repair(&self, meta: &SessionHeader, offset: u64) -> Result<(), String> {
        let path = log_path(
            &self.root.to_string_lossy(),
            meta.cwd.as_deref(),
            &meta.id,
            self.compression,
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.set_len(offset).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn read_first_line(&self, path: &Path) -> Result<Option<String>, String> {
        let mut file = match tokio::fs::File::open(path).await {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        use tokio::io::AsyncReadExt;
        let mut buffer = [0u8; 8192];
        let mut collected = Vec::new();
        loop {
            let bytes_read = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if bytes_read == 0 {
                return Ok(None);
            }
            let slice = &buffer[..bytes_read];
            if let Some(nl) = slice.iter().position(|byte| *byte == 0x0A) {
                collected.extend_from_slice(&slice[..nl]);
                return Ok(Some(String::from_utf8_lossy(&collected).to_string()));
            }
            collected.extend_from_slice(slice);
        }
    }

    async fn read_first_zstd_line(&self, path: &Path) -> Result<Option<String>, String> {
        let mut file = match tokio::fs::File::open(path).await {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        use tokio::io::AsyncReadExt;
        let mut content = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let bytes_read = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if bytes_read == 0 {
                return Ok(None);
            }
            content.extend_from_slice(&buffer[..bytes_read]);
            if let Some(first) = scan_zstd_frames(&content)?.frames.first().copied() {
                let plaintext = decompress_zstd_frame(&content[first.start..first.end])
                    .map_err(|error| {
                        format!("corrupt Zstandard session log: header frame failed validation ({error})")
                    })?;
                return Ok(Some(crate::zstd::parse_zstd_header_plaintext(&plaintext)?));
            }
        }
    }

    async fn list_artifacts(&self) -> Result<Vec<(SessionHeader, PathBuf)>, String> {
        self.ensure_root_encoding().await?;
        let mut artifacts = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for project in self.list_project_dirs().await? {
            for dir in self.list_session_dirs(&project).await? {
                let opposite = dir.join(format!(
                    "session{}",
                    log_suffix(self.opposite_compression())
                ));
                if self.exists(&opposite).await {
                    return Err(self.encoding_mismatch(&opposite));
                }
                let path = dir.join(format!("session{}", log_suffix(self.compression)));
                if !self.exists(&path).await {
                    continue;
                }
                let first = if self.compression == JsonlCompression::Zstd {
                    self.read_first_zstd_line(&path).await?
                } else {
                    self.read_first_line(&path).await?
                };
                let Some(first) = first else {
                    continue;
                };
                let Some(meta) = parse_header_meta(&first) else {
                    continue;
                };
                if meta.version != dsh_session::SESSION_FORMAT_VERSION {
                    continue;
                }
                self.assert_stored_identity(&path, &meta, None).await?;
                if !ids.insert(meta.id.clone()) {
                    return Err(format!(
                        "duplicate JSONL session id \"{}\" appears in multiple project directories",
                        meta.id.as_str()
                    ));
                }
                artifacts.push((meta, path));
            }
        }
        Ok(artifacts)
    }
}

fn decode_zstd_header_line_single(plaintext: &[u8]) -> Result<String, String> {
    crate::zstd::parse_zstd_header_plaintext(plaintext)
}

impl Service for JsonlSessionPersistence {
    fn service_name(&self) -> &'static str {
        "sessionPersistence"
    }
}

#[async_trait::async_trait]
impl dsh_session_persistence::SessionPersistenceApi for JsonlSessionPersistence {
    fn locate(&self, meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        Some(dsh_session_persistence::SessionLocation {
            kind: "jsonl".to_string(),
            path: log_path(
                &self.root.to_string_lossy(),
                meta.cwd.as_deref(),
                &meta.id,
                self.compression,
            )
            .to_string_lossy()
            .to_string(),
        })
    }

    fn supports_raw_artifacts(&self) -> bool {
        true
    }

    async fn read_raw(
        &self,
        id: &SessionId,
    ) -> Result<Option<dsh_session_persistence::SessionRawArtifact>, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Ok(None);
        };
        let (buffer, _) = self.read_stable_file(&path).await?;
        let content = if self.compression == JsonlCompression::Zstd {
            let scan = scan_zstd_frames(&buffer)?;
            if scan.frames.is_empty() {
                return Err("empty or header-less Zstandard session log".to_string());
            }
            let plaintexts = decode_zstd_frames(&buffer, &scan.frames)?;
            String::from_utf8_lossy(&plaintexts.concat()).to_string()
        } else {
            String::from_utf8_lossy(&buffer).to_string()
        };
        let meta = parse_header_meta(content.split('\n').next().unwrap_or(""));
        let Some(meta) = meta else {
            return Err(format!(
                "corrupt session log: invalid header line in \"{}\"",
                path.to_string_lossy()
            ));
        };
        if meta.id != *id {
            return Err(format!(
                "corrupt session log: invalid header line in \"{}\"",
                path.to_string_lossy()
            ));
        }
        Ok(Some(dsh_session_persistence::SessionRawArtifact {
            meta,
            filename: "session.jsonl".to_string(),
            content,
        }))
    }

    async fn create(&self, meta: SessionHeader) -> Result<(), String> {
        self.coordinator().create(meta).await
    }

    async fn append(&self, id: &SessionId, events: &[SessionEvent]) -> Result<(), String> {
        self.coordinator().append(id, events).await
    }

    async fn delete(&self, id: &SessionId) -> Result<bool, String> {
        self.coordinator().delete(id).await
    }

    async fn prepare(&self, id: &SessionId) -> Result<SessionPreparation, String> {
        self.coordinator().prepare(id).await
    }

    async fn load(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        self.coordinator().load(id).await
    }

    async fn inspect(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        self.coordinator().inspect(id).await
    }

    async fn read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        self.coordinator().read_from(id, from_seq).await
    }

    async fn read_event_chunk(
        &self,
        id: &SessionId,
        from_seq: u64,
        max_events: usize,
    ) -> Result<dsh_session_persistence::SessionEventChunk, String> {
        if max_events == 0 {
            return Err("event chunk max_events must be positive".to_string());
        }
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        if self.compression != JsonlCompression::Zstd {
            let mut whole = self.read_from(id, from_seq).await?;
            let has_more = whole.events.len() > max_events;
            whole.events.truncate(max_events);
            return Ok(dsh_session_persistence::SessionEventChunk {
                next_seq: has_more.then(|| from_seq + whole.events.len() as u64),
                events: whole.events,
            });
        }
        let mut events = Vec::with_capacity(max_events.min(4_096));
        let mut next_seq = None;
        stream_zstd_events(&path, |event| {
            if event.seq < from_seq {
                return Ok(true);
            }
            if events.len() == max_events {
                next_seq = Some(event.seq);
                return Ok(false);
            }
            events.push(event);
            Ok(true)
        })?;
        Ok(dsh_session_persistence::SessionEventChunk { events, next_seq })
    }

    async fn visit_event_chunks(
        &self,
        id: &SessionId,
        max_events: usize,
        visitor: Arc<dyn for<'a> Fn(&'a [SessionEvent]) -> Result<(), String> + Send + Sync>,
    ) -> Result<(), String> {
        if max_events == 0 {
            return Err("event chunk max_events must be positive".to_string());
        }
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        if self.compression != JsonlCompression::Zstd {
            let whole = self.read_from(id, 0).await?;
            for chunk in whole.events.chunks(max_events) {
                visitor(chunk)?;
            }
            return Ok(());
        }
        let mut chunk = Vec::with_capacity(max_events.min(4_096));
        stream_zstd_events(&path, |event| {
            chunk.push(event);
            if chunk.len() == max_events {
                visitor(&chunk)?;
                chunk.clear();
            }
            Ok(true)
        })?;
        if !chunk.is_empty() {
            visitor(&chunk)?;
        }
        Ok(())
    }

    async fn read_user_message_events(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionUserMessageEvents, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        let from_whole = |whole: dsh_session_persistence::SessionReadFromResult| {
            let last_seq = whole
                .events
                .last()
                .map(|event| event.seq as i64)
                .unwrap_or(-1);
            dsh_session_persistence::SessionUserMessageEvents {
                meta: whole.meta,
                last_seq,
                events: whole
                    .events
                    .into_iter()
                    .filter(|event| event.type_ == "user/message")
                    .collect(),
            }
        };
        if self.compression != JsonlCompression::Zstd {
            return Ok(from_whole(self.read_from(id, 0).await?));
        }
        let before = file_revision(&std::fs::metadata(&path).map_err(|error| error.to_string())?);
        let file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
        // SAFETY: read-only mapping of a revision-checked artifact.
        let mapping =
            unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|error| error.to_string())?;
        let scan = scan_zstd_frames(&mapping)?;
        if scan.frames.is_empty() || scan.torn_start.is_some() {
            return Ok(from_whole(self.read_from(id, 0).await?));
        }
        let header = scan.frames[0];
        let header_plaintext = decompress_zstd_frame(&mapping[header.start..header.end])?;
        let header_line = decode_zstd_header_line_single(&header_plaintext)?;
        let meta = parse_header_meta(&header_line)
            .ok_or_else(|| "invalid Zstandard session header".to_string())?;
        self.assert_stored_identity(&path, &meta, Some(id)).await?;
        let mut messages = Vec::new();
        let mut last_seq = -1_i64;
        for frame in &scan.frames[1..] {
            let bytes = &mapping[frame.start..frame.end];
            let decoder =
                zstd::stream::read::Decoder::new(bytes).map_err(|error| error.to_string())?;
            let mut frame_messages = Vec::new();
            let mut frame_last_seq = -1_i64;
            let fast = crate::packed_stream::visit_tail_reader(decoder, 1, &mut |event| {
                frame_last_seq = frame_last_seq.max(i64::try_from(event.seq).unwrap_or(i64::MAX));
                if event.type_ == "user/message" {
                    frame_messages.push(event);
                }
                Ok(true)
            });
            match fast {
                Ok(_) => {}
                Err(crate::packed_stream::PackedStreamError::Noncanonical(_, _, _)) => {
                    frame_messages.clear();
                    frame_last_seq = -1;
                    let decoder = zstd::stream::read::Decoder::new(bytes)
                        .map_err(|error| error.to_string())?;
                    let records =
                        serde_json::Deserializer::from_reader(decoder).into_iter::<StorageRecord>();
                    for record in records {
                        let record = record
                            .map_err(|error| format!("invalid JSONL event record: {error}"))?;
                        visit_decoded_storage_record_tail(
                            record,
                            1,
                            &mut |event: SessionEvent| {
                                frame_last_seq = frame_last_seq
                                    .max(i64::try_from(event.seq).unwrap_or(i64::MAX));
                                if event.type_ == "user/message" {
                                    frame_messages.push(event);
                                }
                                Ok(true)
                            },
                        )?;
                    }
                }
                Err(crate::packed_stream::PackedStreamError::Invalid(message)) => {
                    return Err(message);
                }
            }
            last_seq = last_seq.max(frame_last_seq);
            messages.extend(frame_messages);
        }
        let after = file_revision(&std::fs::metadata(&path).map_err(|error| error.to_string())?);
        if before != after {
            return Err("session artifact changed during user-message read".to_string());
        }
        Ok(dsh_session_persistence::SessionUserMessageEvents {
            meta,
            last_seq,
            events: messages,
        })
    }

    async fn read_window(
        &self,
        id: &SessionId,
        request: SessionReadWindowRequest,
    ) -> Result<SessionReadWindowResult, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        if self.compression == JsonlCompression::Zstd
            && let Some(window) = self.read_zstd_window(&path, id, request).await?
        {
            return Ok(window);
        }
        let whole = self.read_from(id, 0).await?;
        let mut messages = request.max_messages.max(1);
        loop {
            match dsh_session_persistence::select_history_window(
                &whole.events,
                request.before_seq,
                messages,
                request.max_events,
            ) {
                Ok(selection) => {
                    return Ok(SessionReadWindowResult {
                        meta: whole.meta,
                        events: whole.events[selection.start..selection.end].to_vec(),
                        has_more: selection.has_more,
                        oversized_event_count: None,
                    });
                }
                Err(error) if messages > 1 => {
                    let required = error.selection.event_count().max(1);
                    let proportional = messages
                        .saturating_mul(request.max_events as u64)
                        .checked_div(required as u64)
                        .unwrap_or(1)
                        .max(1);
                    messages = proportional.min(messages - 1);
                }
                Err(error) => {
                    return Ok(SessionReadWindowResult {
                        meta: whole.meta,
                        events: Vec::new(),
                        has_more: error.selection.has_more,
                        oversized_event_count: Some(error.selection.event_count()),
                    });
                }
            }
        }
    }

    async fn read_list_metadata(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionListMetadata, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        if self.compression != JsonlCompression::Zstd {
            let whole = self.read_from(id, 0).await?;
            let blank = !whole.events.iter().any(|event| event.type_ == "turn/start");
            let updated_at = whole
                .events
                .iter()
                .rev()
                .find(|event| event.type_ == "user/message")
                .map(|event| event.time)
                .unwrap_or(whole.meta.created_at as i64);
            return Ok(dsh_session_persistence::SessionListMetadata {
                last_seq: whole
                    .events
                    .last()
                    .map(|event| event.seq as i64)
                    .unwrap_or(-1),
                meta: whole.meta,
                blank,
                updated_at,
            });
        }
        let (buffer, _) = self.read_stable_file(&path).await?;
        let scan = scan_zstd_frames(&buffer)?;
        if scan.torn_start.is_some() || scan.frames.is_empty() {
            let whole = self.read_from(id, 0).await?;
            let blank = !whole.events.iter().any(|event| event.type_ == "turn/start");
            let updated_at = whole
                .events
                .iter()
                .rev()
                .find(|event| event.type_ == "user/message")
                .map(|event| event.time)
                .unwrap_or(whole.meta.created_at as i64);
            return Ok(dsh_session_persistence::SessionListMetadata {
                last_seq: whole
                    .events
                    .last()
                    .map(|event| event.seq as i64)
                    .unwrap_or(-1),
                meta: whole.meta,
                blank,
                updated_at,
            });
        }
        let header = scan.frames[0];
        let header_plaintext = decompress_zstd_frame(&buffer[header.start..header.end])?;
        let header_line = decode_zstd_header_line_single(&header_plaintext)?;
        let meta = parse_header_meta(&header_line)
            .ok_or_else(|| "invalid Zstandard session header".to_string())?;
        self.assert_stored_identity(&path, &meta, Some(id)).await?;
        let mut blank = true;
        let mut updated_at = meta.created_at as i64;
        let mut last_seq = -1_i64;
        for frame in &scan.frames[1..] {
            for event in self.decode_zstd_event_frame(&buffer[frame.start..frame.end])? {
                last_seq = event.seq as i64;
                if event.type_ == "turn/start" {
                    blank = false;
                } else if event.type_ == "user/message" {
                    updated_at = event.time;
                }
            }
        }
        Ok(dsh_session_persistence::SessionListMetadata {
            meta,
            last_seq,
            blank,
            updated_at,
        })
    }

    async fn read_model_selection_state(
        &self,
        id: &SessionId,
    ) -> Result<Option<serde_json::Value>, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Err(format!("session \"{}\" not found", id.as_str()));
        };
        if self.compression != JsonlCompression::Zstd {
            return Ok(None);
        }
        let (buffer, _) = self.read_stable_file(&path).await?;
        let scan = scan_zstd_frames(&buffer)?;
        if scan.torn_start.is_some() || scan.frames.is_empty() {
            return Ok(None);
        }
        let mut explicit = false;
        let mut selection = serde_json::Value::Null;
        for frame in &scan.frames[1..] {
            let plaintext = decompress_zstd_frame(&buffer[frame.start..frame.end])?;
            let text = std::str::from_utf8(&plaintext)
                .map_err(|error| format!("invalid UTF-8 in Zstandard event frame: {error}"))?;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let stored: serde_json::Value = serde_json::from_str(line)
                    .map_err(|error| format!("invalid JSONL event record: {error}"))?;
                for event in decode_storage_record(&stored)? {
                    if event.type_ == "model/selection"
                        && event
                            .data
                            .get("provider")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                        && event
                            .data
                            .get("model")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                    {
                        explicit = true;
                        selection = event.data;
                    } else if event.type_ == "request/header"
                        && !explicit
                        && let Some(config) = event
                            .data
                            .get("header")
                            .and_then(|header| header.get("config"))
                        && config
                            .get("provider")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                        && config
                            .get("model")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                    {
                        selection = serde_json::json!({
                            "provider": config.get("provider").cloned().unwrap_or(serde_json::Value::Null),
                            "model": config.get("model").cloned().unwrap_or(serde_json::Value::Null),
                            "reasoningEffort": config.get("reasoningEffort").cloned().unwrap_or(serde_json::Value::Null),
                        });
                    }
                }
            }
        }
        Ok(Some(serde_json::json!({
            "explicit": explicit,
            "selection": selection,
        })))
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self
            .list_artifacts()
            .await?
            .into_iter()
            .map(|(meta, _)| meta)
            .collect())
    }

    async fn list_snapshots(
        &self,
    ) -> Result<Vec<dsh_session_persistence::SessionPersistenceSnapshot>, String> {
        let mut snapshots = Vec::new();
        for (header, path) in self.list_artifacts().await? {
            match tokio::fs::metadata(&path).await {
                Ok(metadata) => {
                    snapshots.push(dsh_session_persistence::SessionPersistenceSnapshot {
                        header,
                        revision: file_revision(&metadata),
                    })
                }
                Err(error) if is_not_found(&error) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(snapshots)
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }
}

#[async_trait::async_trait]
impl PersistenceBackend<JsonlTornMarker> for JsonlSessionPersistence {
    fn name(&self) -> &'static str {
        "session-persistence-jsonl"
    }

    async fn load_stored(
        &self,
        id: &SessionId,
    ) -> Result<Option<StoredPrefix<JsonlTornMarker>>, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Ok(None);
        };
        Ok(Some(self.read_prefix(&path, Some(id)).await?))
    }

    async fn read_stored_revision(
        &self,
        id: &SessionId,
    ) -> Result<Option<dsh_session_persistence::SessionPersistenceRevision>, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Ok(None);
        };
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => Ok(Some(file_revision(&metadata))),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn delete_stored(&self, id: &SessionId) -> Result<bool, String> {
        self.ensure_root_encoding().await?;
        let Some(path) = self.find_log(id).await? else {
            return Ok(false);
        };
        let directory = path.parent().ok_or_else(|| {
            format!(
                "session artifact has no parent directory: {}",
                path.display()
            )
        })?;
        match tokio::fs::remove_dir_all(directory).await {
            Ok(()) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(format!(
                "failed to permanently delete session \"{}\" at \"{}\": {error}",
                id.as_str(),
                directory.display()
            )),
        }
    }

    async fn append_batch(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
        is_materialized: bool,
    ) -> Result<(), String> {
        self.ensure_root_encoding().await?;
        if is_materialized {
            self.append_lines(meta, events).await
        } else {
            self.materialize(meta, events).await
        }
    }

    async fn commit_repair(
        &self,
        meta: &SessionHeader,
        torn_marker: Option<JsonlTornMarker>,
        closers: &[SessionEvent],
    ) -> Result<(), String> {
        let mut repaired = Vec::new();
        if let Some(torn) = &torn_marker {
            self.repair(meta, torn.truncate_to).await?;
            repaired.extend(torn.recovered_events.iter().cloned());
        }
        repaired.extend(closers.iter().cloned());
        if !repaired.is_empty() {
            self.append_lines(meta, &repaired).await?;
        }
        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self
            .list_artifacts()
            .await?
            .into_iter()
            .map(|(meta, _)| meta)
            .collect())
    }

    fn locate(&self, meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        Some(dsh_session_persistence::SessionLocation {
            kind: "jsonl".to_string(),
            path: log_path(
                &self.root.to_string_lossy(),
                meta.cwd.as_deref(),
                &meta.id,
                self.compression,
            )
            .to_string_lossy()
            .to_string(),
        })
    }
}

#[cfg(test)]
mod history_window_tests {
    use super::*;
    use dsh_session::{SESSION_FORMAT_VERSION, SurfaceOp, session_id};
    use dsh_session_persistence::SessionPersistenceApi;

    fn event(seq: u64, type_: &str, append_surface: bool) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: seq as i64,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: append_surface.then_some(SurfaceOp::Append),
            source_event_seqs: None,
        }
    }

    #[tokio::test]
    async fn zstd_jsonl_reports_oversized_sparse_window() {
        let root = std::env::temp_dir().join(format!("dsh-jsonl-window-{}", uuid::Uuid::new_v4()));
        let ctx = Context::root();
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().to_string(),
                compression: JsonlCompression::Zstd,
                ..JsonlConfig::default()
            },
        )
        .expect("install jsonl persistence");
        let id = session_id("jsonl-bounded-window");
        SessionPersistenceApi::create(
            backend.as_ref(),
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: id.clone(),
                created_at: 1,
                cwd: Some("C:/workspace".to_string()),
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: Some("standard".to_string()),
            },
        )
        .await
        .expect("create session");
        let mut events = Vec::with_capacity(2_002);
        events.push(event(0, "user/message", true));
        for seq in 1..=2_000 {
            events.push(event(seq, "assistant/chunk", false));
        }
        events.push(event(2_001, "assistant/message", true));
        SessionPersistenceApi::append(backend.as_ref(), &id, &events)
            .await
            .expect("append events");

        let window = SessionPersistenceApi::read_window(
            backend.as_ref(),
            &id,
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 2,
                max_events: 512,
            },
        )
        .await
        .expect("read bounded window");

        assert!(window.events.len() <= 512);
        assert_eq!(window.events.last().map(|event| event.seq), Some(2_001));
        assert_eq!(window.oversized_event_count, None);
        assert!(window.has_more);
        let metadata = SessionPersistenceApi::read_list_metadata(backend.as_ref(), &id)
            .await
            .expect("read list metadata");
        assert!(metadata.blank);
        assert_eq!(metadata.updated_at, 0);
        assert_eq!(metadata.last_seq, 2_001);

        let first = SessionPersistenceApi::read_event_chunk(backend.as_ref(), &id, 0, 1_000)
            .await
            .expect("read first event chunk");
        assert_eq!(first.events.len(), 1_000);
        assert_eq!(first.events.first().map(|event| event.seq), Some(0));
        assert_eq!(first.events.last().map(|event| event.seq), Some(999));
        assert_eq!(first.next_seq, Some(1_000));

        let path = backend
            .find_log(&id)
            .await
            .expect("find log")
            .expect("materialized log");
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open log for torn-tail fixture");
            file.write_all(&[0x28, 0xb5, 0x2f, 0xfd, 0x00])
                .expect("append torn zstd frame");
        }
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let visitor = {
            let count = count.clone();
            Arc::new(move |events: &[SessionEvent]| {
                count.fetch_add(events.len(), std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })
        };
        SessionPersistenceApi::visit_event_chunks(backend.as_ref(), &id, 512, visitor)
            .await
            .expect("visit ignores recoverable torn final frame");
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 2_002);

        drop(backend);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn zstd_jsonl_bounds_oversized_failed_turn_without_final_message() {
        let root =
            std::env::temp_dir().join(format!("dsh-jsonl-failed-turn-{}", uuid::Uuid::new_v4()));
        let ctx = Context::root();
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().to_string(),
                compression: JsonlCompression::Zstd,
                ..JsonlConfig::default()
            },
        )
        .expect("install jsonl persistence");
        let id = session_id("jsonl-oversized-failed-turn");
        SessionPersistenceApi::create(
            backend.as_ref(),
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: id.clone(),
                created_at: 1,
                cwd: Some("D:/workspace".to_string()),
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: Some("standard".to_string()),
            },
        )
        .await
        .expect("create session");
        let mut events = Vec::with_capacity(4_099);
        for seq in 0..4_097 {
            let mut chunk = event(seq, "assistant/chunk", false);
            chunk.data = serde_json::json!({"turn": 1, "step": 1});
            events.push(chunk);
        }
        events.push(event(4_097, "step/end", false));
        let mut turn_end = event(4_098, "turn/end", false);
        turn_end.data = serde_json::json!({
            "turn": 1,
            "reason": {"kind": "error", "error": {"code": "TRANSPORT"}}
        });
        events.push(turn_end);
        SessionPersistenceApi::append(backend.as_ref(), &id, &events)
            .await
            .expect("append events");

        let window = SessionPersistenceApi::read_window(
            backend.as_ref(),
            &id,
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 100,
                max_events: 4_096,
            },
        )
        .await
        .expect("read bounded failed turn");

        assert_eq!(window.events.len(), 4_096);
        assert_eq!(window.events.first().map(|event| event.seq), Some(3));
        assert_eq!(window.events.last().map(|event| event.seq), Some(4_098));
        assert_eq!(window.oversized_event_count, None);
        assert!(window.has_more);

        drop(backend);
        let _ = std::fs::remove_dir_all(root);
    }
}
