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
    if let Some(content) = data.get("content") {
        collect_image_refs(content, refs);
    }
    if let Some(message) = data.get("message").and_then(serde_json::Value::as_object)
        && let Some(content) = message.get("content")
    {
        collect_image_refs(content, refs);
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
    Text { path: String, content: String },
    Data { path: String, data: Vec<u8> },
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
fn image_refs_in_artifact(content: &str, media: &mut HashMap<String, ImageAttachmentRef>) {
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Ok(event) = serde_json::from_value::<dsh_session::SessionEvent>(value) {
            collect_event_image_refs(&event, media);
        }
    }
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
    image_refs_in_artifact(&root.content, &mut media);
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
            image_refs_in_artifact(&raw.content, &mut media);
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
mod tests {
    use super::*;
    use dsh_attachment::attachment_id;
    use futures::StreamExt;

    #[tokio::test]
    async fn streamed_zip_yields_a_header_before_the_entry_producer_finishes() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender
            .send(Ok(SessionLogZipEntry::Text {
                path: "first.jsonl".to_string(),
                content: "x".repeat(2 * 1024 * 1024),
            }))
            .await
            .expect("first entry");
        let signal = crate::fetch::handler::AbortSignal::new();
        let mut stream = stream_session_log_zip(receiver, 6, signal);
        let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("first chunk while producer remains open")
            .expect("first chunk")
            .expect("first chunk is not an error");
        assert!(first.starts_with(b"PK"));
        sender
            .send(Ok(SessionLogZipEntry::Text {
                path: "second.jsonl".to_string(),
                content: "second".to_string(),
            }))
            .await
            .expect("second entry after first response chunk");
        drop(sender);
        let mut bytes = first;
        while let Some(chunk) = stream.next().await {
            bytes.extend(chunk.expect("archive stream remains successful"));
        }
        let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        assert_eq!(archive.len(), 2);
    }

    #[test]
    fn zip_filename_sanitizes_one_segment() {
        assert_eq!(
            session_log_zip_filename("session-abc_123"),
            "dsh-session-session-abc_123.zip"
        );
        assert_eq!(
            session_log_zip_filename("a/b:c\\d"),
            "dsh-session-a_b_c_d.zip"
        );
    }

    #[test]
    fn media_entry_paths_carry_the_extension_per_type() {
        let reference = ImageAttachmentRef {
            attachment_id: attachment_id("img-1"),
            media_type: ImageMediaType::Png,
            bytes: 0,
            width: 0,
            height: 0,
            name: None,
        };
        assert_eq!(media_entry_path(&reference), "media/img-1.png");
    }

    #[test]
    fn image_refs_collect_direct_and_nested() {
        let mut refs = HashMap::new();
        let content = serde_json::json!([
            { "type": "text", "text": "hi" },
            {
                "type": "image",
                "attachment": {
                    "attachmentId": "img-1",
                    "mediaType": "image/png",
                    "bytes": 3,
                    "width": 1,
                    "height": 1
                }
            },
            {
                "type": "tool-call",
                "id": "c1",
                "name": "t",
                "arguments": "{}",
                "content": [{
                    "type": "tool-result",
                    "toolCallId": "c1",
                    "content": [{
                        "type": "image",
                        "attachment": {
                            "attachmentId": "img-2",
                            "mediaType": "image/jpeg",
                            "bytes": 3,
                            "width": 1,
                            "height": 1
                        }
                    }]
                }]
            }
        ]);
        collect_image_refs(&content, &mut refs);
        assert_eq!(refs.len(), 2);
        assert!(refs.contains_key("img-1"));
        assert!(refs.contains_key("img-2"));
    }

    #[test]
    fn assembles_a_readable_zip_with_text_and_data_entries() {
        let bytes = assemble_session_log_zip(
            vec![
                SessionLogZipEntry::Text {
                    path: "session-1.jsonl".to_string(),
                    content: "{\"line\":1}\n".to_string(),
                },
                SessionLogZipEntry::Data {
                    path: "media/img-1.png".to_string(),
                    data: vec![1, 2, 3],
                },
            ],
            DEFAULT_SESSION_LOG_COMPRESSION_LEVEL,
        )
        .expect("assemble");
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).expect("zip parses");
        assert_eq!(archive.len(), 2);
        {
            let mut first = archive.by_index(0).expect("first entry");
            assert_eq!(first.name(), "session-1.jsonl");
            let mut text = String::new();
            std::io::Read::read_to_string(&mut first, &mut text).expect("read");
            assert_eq!(text, "{\"line\":1}\n");
        }
        {
            let mut second = archive.by_index(1).expect("second entry");
            assert_eq!(second.name(), "media/img-1.png");
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut second, &mut data).expect("read");
            assert_eq!(data, vec![1, 2, 3]);
        }
    }
}
