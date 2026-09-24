//! Session-log ZIP export helpers: filenames, archive paths, and media
//! reference collection. Rust port of the corresponding portions of
//! `packages/host/apiproxy/src/session-export.ts` (the streaming zip
//! assembly arrives with the zip-crate milestone).

use std::collections::HashMap;
use std::sync::Arc;

use dsh_attachment::{ImageAttachmentRef, ImageMediaType};

mod zip_stream;
pub use zip_stream::stream_session_log_zip;

/// Valid DEFLATE levels accepted by session-log export.
pub type SessionLogCompressionLevel = u8;

/// Balanced default used when a direct createApiProxy caller omits
/// deployment config.
pub const DEFAULT_SESSION_LOG_COMPRESSION_LEVEL: SessionLogCompressionLevel = 6;

/// Zip extension for each accepted raster media type.
pub fn media_type_extension(media_type: ImageMediaType) -> &'static str {
    match media_type {
        ImageMediaType::Png => "png",
        ImageMediaType::Jpeg => "jpg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Gif => "gif",
    }
}

/// The zip path for one media object: content-addressed by the opaque
/// attachment id so shared images land once and the id in the log maps back
/// to the archive entry without a manifest.
pub fn media_entry_path(reference: &ImageAttachmentRef) -> String {
    format!(
        "media/{}.{}",
        reference.attachment_id,
        media_type_extension(reference.media_type)
    )
}

/// Portable archive path for verbatim file bytes, distinct from image variants.
pub fn file_entry_path(reference: &dsh_attachment::FileAttachmentRef) -> Result<String, String> {
    let digest = reference
        .attachment_id
        .as_str()
        .strip_prefix("sha256:")
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or("invalid file attachment identity")?;
    let name = &reference.name;
    if name.is_empty()
        || name.len() > 240
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err("invalid file attachment name".into());
    }
    Ok(format!("files/{digest}/{name}"))
}
pub fn collect_event_file_refs(
    event: &dsh_session::SessionEvent,
    files: &mut HashMap<String, dsh_attachment::FileAttachmentRef>,
) -> Result<(), String> {
    let mut refs = dsh_attachment::file_references_for_event(&event.type_, &event.data);
    if event.type_ == "agent/inbox/spliced" {
        for message in ["inserted", "cancelled"]
            .into_iter()
            .flat_map(|key| event.data[key].as_array().into_iter().flatten())
        {
            refs.extend(dsh_attachment::file_references_for_event(
                "user/message",
                message,
            ));
        }
    }
    for reference in refs {
        let path = file_entry_path(&reference)?;
        if let Some(existing) = files.insert(path, reference.clone()) {
            if existing != reference {
                return Err("conflicting file attachment metadata".into());
            }
        }
    }
    Ok(())
}

/// Collect every image reference inside one content array, descending into
/// nested tool results the way the live attachment route does.
pub fn collect_image_refs(
    content: &serde_json::Value,
    refs: &mut HashMap<String, ImageAttachmentRef>,
) {
    let Some(array) = content.as_array() else {
        return;
    };
    let mut pending: Vec<&serde_json::Value> = array.iter().collect();
    while let Some(value) = pending.pop() {
        let Some(object) = value.as_object() else {
            continue;
        };
        if object.get("type").and_then(serde_json::Value::as_str) == Some("image")
            && let Some(attachment) = object.get("attachment")
            && let Ok(reference) = serde_json::from_value::<ImageAttachmentRef>(attachment.clone())
        {
            refs.insert(reference.attachment_id.to_string(), reference);
        }
        if let Some(nested) = object.get("content").and_then(serde_json::Value::as_array) {
            pending.extend(nested);
        }
    }
}

/// Collect every image reference one session event carries, across the same
/// carriers the live attachment route scans (direct content, message
/// content, inserted messages, and completed assistant chunk blocks).
pub fn collect_event_image_refs(
    event: &dsh_session::SessionEvent,
    refs: &mut HashMap<String, ImageAttachmentRef>,
) {
    let Some(data) = event.data.as_object() else {
        return;
    };
    if let Some(meta) = data.get("meta").filter(|value| {
        value.get("kind").and_then(serde_json::Value::as_str) == Some("image-generation")
    }) {
        if let Some(images) = meta.get("images") {
            collect_image_refs(images, refs);
        }
        if let Some(images) = meta.get("sourceImages") {
            collect_image_refs(images, refs);
        }
    }
    if let Some(content) = data.get("content") {
        collect_image_refs(content, refs);
    }
    if let Some(message) = data.get("message").and_then(serde_json::Value::as_object)
        && let Some(content) = message.get("content")
    {
        collect_image_refs(content, refs);
    }
    if event.type_ == "agent/inbox/spliced" {
        for message in ["inserted", "cancelled"].into_iter().flat_map(|key| {
            data.get(key)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
        }) {
            collect_image_refs(&message["content"], refs);
        }
    }
    if let Some(messages) = data.get("messages").and_then(serde_json::Value::as_array) {
        for message in messages {
            if let Some(content) = message
                .as_object()
                .and_then(|message| message.get("content"))
            {
                collect_image_refs(content, refs);
            }
        }
    }
    if let Some(chunk) = data.get("chunk").and_then(serde_json::Value::as_object)
        && let Some(content) = chunk.get("content")
    {
        collect_image_refs(content, refs);
    }
}

/// One exported file: a stored artifact text or one referenced media object.
pub enum SessionLogZipEntry {
    Text {
        path: String,
        content: String,
    },
    Data {
        path: String,
        data: Vec<u8>,
    },
    Stream {
        path: String,
        bytes: u64,
        reader: dsh_attachment::AttachmentReader,
    },
}

/// The services a session-log export needs.
pub struct SessionLogExportDeps {
    pub session_query: Option<Arc<dsh_session_query::SessionQueryEngine>>,
    pub session_persistence: Option<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>,
    pub attachments: Option<Arc<dyn dsh_attachment::AttachmentStore>>,
    pub sessions: Option<Arc<dsh_session::SessionStore>>,
}

/// Flush one currently live session through the store's authoritative
/// durability barrier immediately before its raw artifact is read.
pub async fn flush_live_session_log(
    deps: &SessionLogExportDeps,
    id: &dsh_session::SessionId,
    signal: &crate::fetch::handler::AbortSignal,
) -> Result<(), String> {
    if signal.aborted() {
        return Err("session log export was cancelled".to_string());
    }
    let Some(sessions) = &deps.sessions else {
        return Ok(());
    };
    let Some(session) = sessions.get(id) else {
        return Ok(());
    };
    sessions
        .flush(&session)
        .await
        .map_err(|error| format!("session log flush failed: {error}"))?;
    if signal.aborted() {
        return Err("session log export was cancelled".to_string());
    }
    Ok(())
}

/// Collect media references from one artifact text (one JSON event per
/// line).
fn artifact_refs(
    content: &str,
    media: &mut HashMap<String, ImageAttachmentRef>,
    files: &mut HashMap<String, dsh_attachment::FileAttachmentRef>,
) -> Result<(), String> {
    let mut lines = content.lines();
    let header: serde_json::Value =
        serde_json::from_str(lines.next().ok_or("empty session artifact")?)
            .map_err(|e| e.to_string())?;
    let mut decoder = if header["version"] == 4 {
        Some(dsh_session::format_v4::V4Decoder::new(
            header,
            dsh_session::format_v4::V4Recovery::Strict,
        )?)
    } else {
        None
    };
    for line in lines {
        let row: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if let Some(decoder) = &mut decoder {
            if let Some(row) = decoder.decode_row(row)? {
                let event = serde_json::from_value(row).map_err(|e| e.to_string())?;
                collect_event_image_refs(&event, media);
                collect_event_file_refs(&event, files)?;
            }
        } else {
            dsh_session::visit_storage_record_events(&row, |event| {
                collect_event_image_refs(&event, media);
                collect_event_file_refs(&event, files)?;
                Ok(true)
            })?;
        }
    }
    Ok(())
}

/// Produce export entries in ZIP order through a bounded channel. The
/// capacity-one handoff prevents descendant logs and attachments from
/// accumulating while the blocking ZIP writer applies downstream pressure.
pub async fn produce_session_log_zip_entries(
    deps: &SessionLogExportDeps,
    root: dsh_session_persistence::SessionRawArtifact,
    session_id: &dsh_session::SessionId,
    include_descendants: bool,
    signal: &crate::fetch::handler::AbortSignal,
    sender: &tokio::sync::mpsc::Sender<Result<SessionLogZipEntry, String>>,
) -> Result<(), String> {
    let mut media: HashMap<String, ImageAttachmentRef> = HashMap::new();
    let mut files = HashMap::new();
    artifact_refs(&root.content, &mut media, &mut files)?;
    sender
        .send(Ok(SessionLogZipEntry::Text {
            path: root.filename,
            content: root.content,
        }))
        .await
        .map_err(|_| "session log export consumer closed".to_string())?;

    if include_descendants {
        let Some(query) = &deps.session_query else {
            return Err("session log export requires the sessionQuery service".to_string());
        };
        let Some(persistence) = &deps.session_persistence else {
            return Err("session log export requires the sessionPersistence service".to_string());
        };
        let abort_flag = signal.clone();
        let signal_ref: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || abort_flag.aborted());
        let lineage = query
            .trace_session(session_id, Some(&signal_ref))
            .await
            .map_err(|error| error.to_string())?;
        let descendants: &Vec<dsh_session_query::SessionLineageNode> = match &lineage {
            dsh_session_query::SessionLineageTrace::Complete { descendants, .. }
            | dsh_session_query::SessionLineageTrace::Partial { descendants, .. } => descendants,
        };
        let mut seen: std::collections::HashSet<dsh_session::SessionId> =
            std::collections::HashSet::new();
        seen.insert(session_id.clone());
        // Collect in pre-order, like the TS generator recursion.
        let mut pending: Vec<&dsh_session_query::SessionLineageNode> = descendants.iter().collect();
        while let Some(node) = pending.pop() {
            if signal.aborted() {
                return Err("session log export was cancelled".to_string());
            }
            let id = node.session.header.id.clone();
            if seen.contains(&id) {
                continue;
            }
            seen.insert(id.clone());
            flush_live_session_log(deps, &id, signal).await?;
            let Some(raw) = persistence
                .read_raw(&id)
                .await
                .map_err(|error| error.to_string())?
            else {
                return Err(format!("subagent \"{id}\" has no stored log artifact"));
            };
            artifact_refs(&raw.content, &mut media, &mut files)?;
            sender
                .send(Ok(SessionLogZipEntry::Text {
                    path: format!(
                        "subagents/{}/{}",
                        safe_session_id_segment(id.as_str()),
                        raw.filename
                    ),
                    content: raw.content,
                }))
                .await
                .map_err(|_| "session log export consumer closed".to_string())?;
            pending.extend(node.descendants.iter());
        }
    }

    let Some(attachments) = &deps.attachments else {
        return Err("session log export requires the attachments service".to_string());
    };
    let abort = signal.clone();
    let attachment_abort: dsh_attachment::AttachmentAbort = Arc::new(move || abort.aborted());
    for (path, reference) in files {
        let stored = attachments
            .open_file(&reference, Some(&attachment_abort))
            .await
            .map_err(|e| e.to_string())?;
        sender
            .send(Ok(SessionLogZipEntry::Stream {
                path,
                bytes: reference.bytes,
                reader: stored.reader,
            }))
            .await
            .map_err(|_| "session log export consumer closed".to_string())?;
    }
    for reference in media.values() {
        if signal.aborted() {
            return Err("session log export was cancelled".to_string());
        }
        let stored = attachments
            .read_image(reference, None)
            .await
            .map_err(|error| error.to_string())?;
        sender
            .send(Ok(SessionLogZipEntry::Data {
                path: media_entry_path(reference),
                data: stored.data,
            }))
            .await
            .map_err(|_| "session log export consumer closed".to_string())?;
    }
    Ok(())
}

/// Assemble the session-log ZIP as one byte vector (the TS streams deflate
/// chunks through a capacity gate; the Rust counterpart builds in memory —
/// a bounded-export deviation).
pub fn assemble_session_log_zip(
    entries: Vec<SessionLogZipEntry>,
    compression_level: SessionLogCompressionLevel,
) -> Result<Vec<u8>, String> {
    use std::io::Write;

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(i64::from(compression_level)));
        for entry in entries {
            match entry {
                SessionLogZipEntry::Text { path, content } => {
                    writer
                        .start_file(path, options)
                        .map_err(|error| error.to_string())?;
                    writer
                        .write_all(content.as_bytes())
                        .map_err(|error| error.to_string())?;
                }
                SessionLogZipEntry::Stream { .. } => {
                    return Err("file attachments require streaming ZIP export".into());
                }
                SessionLogZipEntry::Data { path, data } => {
                    writer
                        .start_file(path, options)
                        .map_err(|error| error.to_string())?;
                    writer.write_all(&data).map_err(|error| error.to_string())?;
                }
            }
        }
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(buffer.into_inner())
}
pub fn safe_session_id_segment(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// The export archive filename for one root session.
pub fn session_log_zip_filename(session_id: &str) -> String {
    format!("dsh-session-{}.zip", safe_session_id_segment(session_id))
}

#[cfg(test)]
mod generated_image_tests {
    #[test]
    fn generated_images_and_edit_sources_survive_export() {
        let generated_id = format!("sha256:{}", "a".repeat(64));
        let source_id = format!("sha256:{}", "b".repeat(64));
        let reference = serde_json::json!({"type":"image","attachment":{"attachmentId":generated_id,"mediaType":"image/png","bytes":20,"width":1,"height":1,"name":"generated-fixture.png"}});
        let mut source = reference.clone();
        source["attachment"]["attachmentId"] = serde_json::json!(source_id);
        let event = dsh_session::SessionEvent {
            type_: "tool/result".into(),
            seq: dsh_session::SessionSeq::new(0).unwrap(),
            time: 0,
            data: serde_json::json!({"meta":{"kind":"image-generation","images":[reference.clone(),reference],"sourceImages":[source]}}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let mut found = std::collections::HashMap::new();
        super::collect_event_image_refs(&event, &mut found);
        assert_eq!(found.len(), 2);
        assert!(found.contains_key(&generated_id));
        assert!(found.contains_key(&source_id));
    }
}

#[cfg(test)]
mod file_archive_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn declared_user_files_are_collected_but_opaque_tool_arguments_are_not() {
        let reference =
            json!({"attachmentId":format!("sha256:{}","a".repeat(64)),"name":"file.bin","bytes":3});
        let mut event:dsh_session::SessionEvent=serde_json::from_value(json!({"type":"user/message","seq":0,"time":0,"data":{"role":"user","source":{"kind":"user"},"content":[{"type":"file","attachment":reference}]}})).unwrap();
        let mut refs = HashMap::new();
        collect_event_file_refs(&event, &mut refs).unwrap();
        assert_eq!(refs.len(), 1);
        event.type_ = "tool/call".into();
        refs.clear();
        collect_event_file_refs(&event, &mut refs).unwrap();
        assert!(refs.is_empty());
        let mut bad: dsh_attachment::FileAttachmentRef = serde_json::from_value(reference).unwrap();
        bad.name = "../escape".into();
        assert!(file_entry_path(&bad).is_err());
    }
}
