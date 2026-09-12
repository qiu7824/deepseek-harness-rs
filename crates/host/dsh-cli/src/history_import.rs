//! Validated, bounded history import. Source artifacts are never modified.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dsh_session::{Session, SessionEvent, SessionHeader, SessionLogOffset};
use dsh_session_persistence_jsonl::{
    JsonlCompression, compress_zstd_frame, log_path, to_header_line,
};
use serde_json::{Value, json};

const MAX_LOG: usize = 64 * 1024 * 1024;
const MAX_BATCH: usize = 256 * 1024 * 1024;
const MAX_EVENTS: usize = 200_000;

fn bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("history exceeds the {limit}-byte import limit"));
    }
    Ok(bytes)
}

fn normalize_message(message: &mut Value, role: &str, id: &str) {
    if let Some(object) = message.as_object_mut() {
        object.entry("id").or_insert_with(|| json!(id));
        object.entry("role").or_insert_with(|| json!(role));
    }
}

fn normalize_v0(
    event: &mut Value,
    session: &str,
    ids: &HashMap<u64, String>,
    retries: &mut HashMap<String, String>,
    compaction: &mut Option<String>,
) -> Result<(), String> {
    let seq = event["seq"].as_u64().ok_or("event sequence is missing")?;
    let id = format!("legacy-message:{session}:{seq}");
    let kind = event["type"]
        .as_str()
        .ok_or("event type is missing")?
        .to_owned();
    if matches!(kind.as_str(), "request/header-delta" | "mode/set") {
        return Err(format!("unsupported released legacy event {kind}"));
    }
    if let Some(suffix) = kind.strip_prefix("compact/") {
        event["type"] = json!(format!("compaction/{suffix}"));
    }
    if kind == "steering/message" {
        let mut data = event["data"].clone();
        if let Some(message) = data.get("message") {
            data = message.clone();
        } else {
            data.as_object_mut()
                .ok_or("invalid steering message")?
                .remove("turn");
        }
        normalize_message(&mut data, "user", &id);
        event["type"] = json!("user/message");
        event["data"] = data;
    }
    let kind = event["type"].as_str().unwrap().to_owned();
    let replacement = event.pointer("/surfaceOp/start").and_then(Value::as_u64);
    let data = event["data"]
        .as_object_mut()
        .ok_or("event data must be an object")?;
    match kind.as_str() {
        "user/message" => {
            let mut message = Value::Object(data.clone());
            normalize_message(&mut message, "user", &id);
            *data = message.as_object().unwrap().clone();
        }
        "assistant/message" if !data.contains_key("message") => {
            let content = data
                .remove("content")
                .ok_or("legacy assistant content is missing")?;
            let mut source = data
                .remove("provenance")
                .ok_or("legacy assistant provenance is missing")?;
            source
                .as_object_mut()
                .ok_or("invalid assistant provenance")?
                .insert("kind".into(), json!("model"));
            data.insert(
                "message".into(),
                json!({"id":id,"role":"assistant","content":content,"source":source}),
            );
        }
        "tool/result" if !data.contains_key("message") => {
            let call = data
                .remove("callId")
                .ok_or("legacy tool call identity is missing")?;
            let content = data
                .remove("content")
                .ok_or("legacy tool content is missing")?;
            let error = data
                .remove("isError")
                .ok_or("legacy tool outcome is missing")?;
            let id = match replacement {
                Some(seq) => ids
                    .get(&seq)
                    .ok_or("tool replacement has no source message")?
                    .clone(),
                None => id,
            };
            data.insert("message".into(), json!({"id":id,"role":"user","source":{"kind":"tool","callId":call},"content":[{"type":"tool-result","toolCallId":call,"content":content,"isError":error}]}));
        }
        "turn/start" => {
            data.remove("trigger");
        }
        "turn/end" => {
            let reason = data
                .get_mut("reason")
                .and_then(Value::as_object_mut)
                .ok_or("invalid turn end reason")?;
            match reason.get("kind").and_then(Value::as_str) {
                Some("disposed") => {
                    *reason = json!({"kind":"aborted","reason":{"kind":"disposed"}})
                        .as_object()
                        .unwrap()
                        .clone();
                }
                Some("aborted") => {
                    reason
                        .entry("reason")
                        .or_insert_with(|| json!({"kind":"legacy"}));
                }
                Some("error") if !reason.contains_key("error") => {
                    let error = reason.remove("failure").unwrap_or_else(|| json!({"message":reason.get("message").cloned().unwrap_or(json!("Legacy model failure")),"code":reason.get("code").cloned().unwrap_or(json!("UNKNOWN"))}));
                    *reason = json!({"kind":"error","error":error})
                        .as_object()
                        .unwrap()
                        .clone();
                }
                _ => {}
            }
        }
        "request/header" => {
            if data.get("reason").and_then(Value::as_str) == Some("fallback") {
                return Err("unsupported legacy fallback request header".into());
            }
            let header = data
                .get_mut("header")
                .and_then(Value::as_object_mut)
                .ok_or("invalid request header")?;
            if header.get("messagePrefix").is_some_and(|v| !v.is_array()) {
                return Err("invalid legacy messagePrefix".into());
            }
            header.remove("messagePrefix");
        }
        "llm/retry" => {
            let key = serde_json::to_string(&[
                data.get("turn"),
                data.get("step"),
                data.get("provider"),
                data.get("policyKey"),
            ])
            .unwrap();
            let identity = data
                .get("retryId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    retries
                        .entry(key.clone())
                        .or_insert_with(|| format!("legacy-retry:{session}:{seq}"))
                        .clone()
                });
            retries.insert(key, identity.clone());
            data.entry("retryId").or_insert(json!(identity));
        }
        "compaction/start" => {
            let identity = data
                .entry("compactionId")
                .or_insert_with(|| json!(format!("legacy-compaction:{session}:{seq}")))
                .as_str()
                .ok_or("invalid compaction identity")?
                .to_owned();
            *compaction = Some(identity);
        }
        "compaction/summary" | "compaction/end" => {
            if let Some(id) = compaction.as_ref() {
                data.entry("compactionId").or_insert_with(|| json!(id));
            }
            if kind == "compaction/end" {
                *compaction = None;
            }
        }
        "session/end-seed" => *compaction = None,
        _ => {}
    }
    Ok(())
}

fn provenance(row: &mut Value) -> Result<(), String> {
    let seq = row["seq"].as_u64().unwrap_or(0);
    let Some(ranges) = row.get("sourceEventSeqs") else {
        return Ok(());
    };
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut has_range = false;
    for range in ranges
        .as_array()
        .ok_or("sourceEventSeqs must be an array")?
    {
        let (start, end) = if let Some(value) = range.as_u64() {
            (value, value)
        } else {
            has_range = true;
            let pair = range
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("invalid provenance range")?;
            (
                pair[0].as_u64().ok_or("invalid range start")?,
                pair[1].as_u64().ok_or("invalid range end")?,
            )
        };
        if start > end || end >= seq || end - start + 1 > MAX_EVENTS as u64 {
            return Err("provenance must reference bounded earlier events".into());
        }
        for value in start..=end {
            if !seen.insert(value) {
                return Err("duplicate provenance reference".into());
            }
            entries.push(value);
        }
    }
    if has_range && entries.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("provenance ranges must be increasing".into());
    }
    row["sourceEventSeqs"] = json!(entries);
    Ok(())
}

fn convert(
    bytes: &[u8],
    compressed: bool,
) -> Result<
    (
        SessionHeader,
        Vec<u8>,
        HashMap<String, dsh_attachment::ImageAttachmentRef>,
    ),
    String,
> {
    let decoded = if compressed {
        bounded(
            zstd::stream::read::Decoder::new(bytes).map_err(|e| e.to_string())?,
            MAX_LOG,
        )?
    } else {
        bytes.to_vec()
    };
    if decoded.len() > MAX_LOG || !decoded.ends_with(b"\n") {
        return Err(
            "history is oversized or has an incomplete final record; original retained".into(),
        );
    }
    let text = std::str::from_utf8(&decoded).map_err(|_| "history is not UTF-8")?;
    let mut lines = text.lines();
    let raw: Value = serde_json::from_str(lines.next().ok_or("missing history header")?)
        .map_err(|_| "invalid history header JSON")?;
    let version = raw["version"]
        .as_u64()
        .filter(|v| *v <= 3)
        .ok_or("unsupported history version")?;
    if raw["type"] != "session" {
        return Err("not a session artifact".into());
    }
    let mut header: SessionHeader = serde_json::from_value(json!({
        "version":3,"id":raw["id"],"createdAt":raw["createdAt"],"cwd":raw.get("cwd"),"parentSession":raw.get("parentSession"),
        "isSeeded":raw.get("isSeeded").cloned().unwrap_or(json!(raw.get("seedLength").is_some())),
        "origin":raw.get("origin"),"delegationDepth":raw.get("delegationDepth"),"agentPreset":raw.get("agentPreset")
    })).map_err(|_| "invalid session header fields")?;
    let mut events: Vec<SessionEvent> = Vec::new();
    let mut ids = HashMap::new();
    let mut retries = HashMap::new();
    let mut compaction = None;
    for line in lines {
        let mut row: Value =
            serde_json::from_str(line).map_err(|_| "invalid history record JSON")?;
        if version == 0 && row["type"].as_str().is_some_and(|kind| kind.contains('/')) {
            normalize_v0(
                &mut row,
                header.id.as_str(),
                &ids,
                &mut retries,
                &mut compaction,
            )?;
        }
        if version >= 2 {
            provenance(&mut row)?;
        }
        if row["type"] == "tool/ptc-dispatch" {
            row["type"] = json!("tool/code-dispatch");
        }
        if row["type"] == "tool/ptc-dispatch-start" {
            row["type"] = json!("tool/code-dispatch-start");
        }
        if version < 3
            && matches!(
                row["type"].as_str(),
                Some("user/message" | "assistant/message" | "tool/result")
            )
            && row.get("surfaceOp").is_none()
        {
            row["surfaceOp"] = json!("append");
        }
        if row["type"] == "assistant/attempt"
            && !row.pointer("/data/stream").is_some_and(Value::is_array)
        {
            return Err("invalid assistant attempt stream".into());
        }
        if version < 3
            && row["type"] == "agent-preset/selected"
            && row["data"]["agentPreset"] == "code"
        {
            row["data"]["agentPreset"] = json!("ptc");
        }
        dsh_session::visit_storage_record_events(&row, |event| {
            if events.len() >= MAX_EVENTS || event.seq.get() != events.len() as u64 {
                return Err("history has excessive or noncontiguous events".into());
            }
            if !dsh_session::is_known_session_event_type(&event.type_)
                && event.ignorable != Some(true)
            {
                return Err(format!("unsupported required event {}", event.type_));
            }
            if let Some(id) = event
                .data
                .get("id")
                .or_else(|| event.data.pointer("/message/id"))
                .and_then(Value::as_str)
            {
                ids.insert(event.seq.get(), id.to_owned());
            }
            events.push(event);
            Ok(true)
        })?;
    }
    let markers: Vec<u64> = events
        .iter()
        .filter(|e| e.type_ == "session/end-seed" && e.data["inherited"] == true)
        .map(|e| e.seq.get())
        .collect();
    let mut cut = match raw.get("seedLength").and_then(Value::as_u64) {
        Some(cut) => cut,
        None if header.is_seeded => *markers
            .first()
            .ok_or("seeded history has no inherited end-seed marker")?,
        None => 0,
    };
    if markers.len() > 1 || (!header.is_seeded && !markers.is_empty()) || cut > events.len() as u64
    {
        return Err("invalid inherited history boundary".into());
    }
    if version < 3 && header.agent_preset.as_deref() == Some("code") {
        header.agent_preset = Some("ptc".into());
    }
    if version < 3 {
        header.version = 0;
        let report = dsh_session::migrate_v0_to_v3(header, &events)?;
        cut = *report
            .source_cuts
            .get(cut as usize)
            .ok_or("invalid inherited migration cut")? as u64;
        header = report.header;
        events = report.events;
    }
    let inherited = SessionLogOffset::new(cut)?;
    Session::from_restore(header.id.clone(), events.clone(), &header, inherited)?;
    for event in &events {
        if dsh_session::surface::is_surface_event(event) {
            let value = if event.type_ == "user/message" {
                &event.data
            } else {
                &event.data["message"]
            };
            serde_json::from_value::<dsh_llm::Message>(value.clone())
                .map_err(|_| format!("invalid message at sequence {}", event.seq))?;
        }
    }
    let physical = to_header_line(&header, Some(inherited))?;
    let mut output = compress_zstd_frame(
        format!(
            "{}\n",
            serde_json::to_string(&physical).map_err(|e| e.to_string())?
        )
        .as_bytes(),
    )?;
    let mut records = Vec::new();
    let mut media = HashMap::new();
    for event in events {
        dsh_host_apiproxy::session_export::collect_event_image_refs(&event, &mut media);
        serde_json::to_writer(&mut records, &event).map_err(|e| e.to_string())?;
        records.push(b'\n');
    }
    if !records.is_empty() {
        output.extend(compress_zstd_frame(&records)?);
    }
    Ok((header, output, media))
}

fn collect(path: &Path, output: &mut Vec<PathBuf>, depth: usize) -> Result<(), String> {
    if depth > 32 || output.len() > 1000 {
        return Err("history import tree exceeds its limit".into());
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        return Err("history import does not follow symbolic links".into());
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            collect(&entry.map_err(|e| e.to_string())?.path(), output, depth + 1)?;
        }
    } else if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".jsonl") || n.ends_with(".jsonl.zstd"))
    {
        output.push(path.to_owned());
    }
    Ok(())
}

pub(crate) fn import(source: &Path, target_home: &Path) -> Result<usize, String> {
    let mut inputs = Vec::new();
    let mut media_files = HashMap::new();
    if source
        .extension()
        .is_some_and(|extension| extension == "zip")
    {
        let file = std::fs::File::open(source).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|_| "invalid history ZIP")?;
        if archive.len() > 4096 {
            return Err("history ZIP exceeds 4096 entries".into());
        }
        let mut total = 0usize;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|_| "invalid history ZIP entry")?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_owned();
            if name.ends_with(".jsonl")
                || name.ends_with(".jsonl.zstd")
                || name.starts_with("media/")
            {
                let data = bounded(&mut entry, MAX_LOG)?;
                total += data.len();
                if total > MAX_BATCH {
                    return Err("history ZIP exceeds 256 MiB expanded bytes".into());
                }
                if name.starts_with("media/") {
                    media_files.insert(name, data);
                } else {
                    inputs.push((name.ends_with("zstd"), data));
                }
            }
        }
    } else {
        let mut candidates = Vec::new();
        let log_source = if source.join("sessions").is_dir() {
            source.join("sessions")
        } else {
            source.to_owned()
        };
        collect(&log_source, &mut candidates, 0)?;
        candidates.sort();
        for path in candidates {
            inputs.push((
                path.extension().is_some_and(|e| e == "zstd"),
                bounded(
                    std::fs::File::open(&path).map_err(|error| error.to_string())?,
                    MAX_LOG,
                )?,
            ));
        }
    }
    if inputs.is_empty() || inputs.len() > 1000 {
        return Err("history import requires 1 to 1000 session artifacts".into());
    }
    let root = target_home.join("sessions");
    let mut prepared = Vec::new();
    let mut total = 0usize;
    let mut identities = HashSet::new();
    let mut references = HashMap::new();
    let mut existing = Vec::new();
    if root.exists() {
        collect(&root, &mut existing, 0)?;
    }
    // Validate the entire batch before publishing the first session.
    for (compressed, original) in inputs {
        total += original.len();
        if total > MAX_BATCH {
            return Err("history import batch exceeds 256 MiB".into());
        }
        let (header, bytes, images) = convert(&original, compressed)?;
        references.extend(images);
        if !identities.insert(header.id.to_string()) {
            return Err("duplicate session identity in import batch".into());
        }
        let target = log_path(
            &root.to_string_lossy(),
            header.cwd.as_deref(),
            &header.id,
            JsonlCompression::Zstd,
        );
        let plain = log_path(
            &root.to_string_lossy(),
            header.cwd.as_deref(),
            &header.id,
            JsonlCompression::None,
        );
        let encoded = dsh_session_persistence_jsonl::encode_segment(header.id.as_str())?;
        if existing.iter().any(|path| {
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == encoded.as_str())
                && path != &target
        }) {
            return Err(format!(
                "session {} already exists in another artifact",
                header.id
            ));
        }
        if plain.exists()
            || (target.exists()
                && bounded(
                    std::fs::File::open(&target).map_err(|error| error.to_string())?,
                    MAX_LOG,
                )? != bytes)
        {
            return Err(format!(
                "session {} already exists with different content",
                header.id
            ));
        }
        prepared.push((target, bytes, original));
    }
    if !references.is_empty() {
        let image_root = target_home.join("attachments/v1");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let mut pending = Vec::new();
        for reference in references.values() {
            let id = reference.attachment_id.as_str();
            let digest = id
                .strip_prefix("sha256:")
                .filter(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
                .ok_or("invalid image content identity")?;
            let entry = dsh_host_apiproxy::session_export::media_entry_path(reference);
            let source_root = if source.is_dir() {
                source
            } else {
                source.parent().ok_or("image source has no parent")?
            };
            let object = source_root
                .join("attachments/v1/objects")
                .join(&digest[..2])
                .join(digest);
            let target = image_root.join("objects").join(&digest[..2]).join(digest);
            let bytes = if let Some(data) = media_files.remove(&entry) {
                data
            } else {
                let path = if object.is_file() { object } else { target };
                bounded(
                    std::fs::File::open(path).map_err(|_| {
                        format!("missing attachment {id}; import its export ZIP or source home")
                    })?,
                    MAX_LOG,
                )?
            };
            use sha2::Digest;
            if format!("{:x}", sha2::Sha256::digest(&bytes)) != digest {
                return Err("attachment content hash mismatch".into());
            }
            total += bytes.len();
            if total > MAX_BATCH {
                return Err("history with attachments exceeds 256 MiB".into());
            }
            pending.push((
                reference.clone(),
                dsh_attachment::SaveImageAttachment {
                    data: bytes,
                    media_type: reference.media_type,
                    name: reference.name.clone(),
                },
            ));
        }
        let limits = dsh_attachment::ImageAttachmentLimits {
            max_image_bytes: MAX_LOG as u64,
            max_images_per_message: 1000,
            max_message_image_bytes: MAX_BATCH as u64,
            max_image_pixels: 40_000_000,
            media_types: vec![
                dsh_attachment::ImageMediaType::Png,
                dsh_attachment::ImageMediaType::Jpeg,
                dsh_attachment::ImageMediaType::Webp,
                dsh_attachment::ImageMediaType::Gif,
            ],
        };
        for (_, input) in &pending {
            runtime
                .block_on(dsh_attachment_local::store::validate_image_file(
                    input, &limits,
                ))
                .map_err(|error| error.to_string())?;
        }
        for (reference, input) in pending {
            let stored = runtime
                .block_on(dsh_attachment_local::store::save_image_file(
                    &image_root,
                    &input,
                    &limits,
                ))
                .map_err(|error| error.to_string())?;
            if stored.attachment_id != reference.attachment_id
                || stored.width != reference.width
                || stored.height != reference.height
                || stored.bytes != reference.bytes
            {
                return Err("attachment metadata mismatch; sessions were not published".into());
            }
        }
    }
    let count = prepared.len();
    for (target, bytes, original) in prepared {
        if target.exists() {
            continue;
        }
        let parent = target.parent().ok_or("invalid target directory")?;
        let mut ancestor = target_home.to_owned();
        for component in parent
            .strip_prefix(target_home)
            .map_err(|_| "invalid history target")?
            .components()
        {
            ancestor.push(component);
            if let Ok(meta) = std::fs::symlink_metadata(&ancestor) {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err("history target contains a link or non-directory".into());
                }
                if target_home.exists()
                    && !std::fs::canonicalize(&ancestor)
                        .map_err(|error| error.to_string())?
                        .starts_with(
                            std::fs::canonicalize(target_home)
                                .map_err(|error| error.to_string())?,
                        )
                {
                    return Err("history target escapes configured home".into());
                }
            }
        }
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let canonical_home = std::fs::canonicalize(target_home).map_err(|e| e.to_string())?;
        if !std::fs::canonicalize(parent)
            .map_err(|e| e.to_string())?
            .starts_with(&canonical_home)
        {
            return Err("history target escapes its configured home".into());
        }
        let temporary = parent.join(format!(".import-{}", uuid::Uuid::new_v4()));
        let publish = (|| {
            let backup = parent.join("import-source.original");
            if backup.exists()
                && bounded(
                    std::fs::File::open(&backup).map_err(|error| error.to_string())?,
                    MAX_LOG,
                )? != original
            {
                return Err("existing import source backup differs".into());
            }
            if !backup.exists() {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&backup)
                    .map_err(|e| e.to_string())?;
                file.write_all(&original).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            std::fs::hard_link(&temporary, &target).map_err(|e| e.to_string())
        })();
        let _ = std::fs::remove_file(&temporary);
        publish?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(version: u64) -> Vec<u8> {
        let header = json!({"type":"session","version":version,"id":"import-test","createdAt":1,"delegationDepth":0,"isSeeded":false});
        let rows = vec![
            json!({"type":"user/message","data":{"id":"user-one","role":"user","content":[{"type":"text","text":"hello"}],"source":{"kind":"user"}},"surfaceOp":"append"}),
            json!({"type":"turn/start","data":{"turn":1}}),
            json!({"type":"step/start","data":{"turn":1,"step":1}}),
            json!({"type":"request/header","data":{"reason":"initial","header":{"config":{"provider":"fixture","model":"fixture"},"system":"Keep this policy"}}}),
            json!({"type":"assistant/message","data":{"turn":1,"step":1,"message":{"id":"answer-one","role":"assistant","content":[{"type":"text","text":"reply"}],"source":{"kind":"model","provider":"fixture","model":"fixture"}}},"surfaceOp":"append"}),
            json!({"type":"step/end","data":{"turn":1,"step":1}}),
            json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"completed"}}}),
        ];
        let mut bytes = format!("{header}\n").into_bytes();
        for (seq, mut row) in rows.into_iter().enumerate() {
            row["seq"] = json!(seq);
            row["time"] = json!(seq + 1);
            if version == 3 && row["type"] == "request/header" {
                row["data"]["header"]
                    .as_object_mut()
                    .unwrap()
                    .remove("system");
            }
            if version == 0 && row["type"] == "user/message" {
                row["data"].as_object_mut().unwrap().remove("id");
                row["data"].as_object_mut().unwrap().remove("role");
            }
            bytes.extend(format!("{row}\n").as_bytes());
        }
        bytes
    }

    #[test]
    fn released_versions_restore_and_original_bytes_remain_unchanged() {
        for version in 0..=3 {
            let original = log(version);
            let before = original.clone();
            for compressed in [false, true] {
                let source = if compressed {
                    compress_zstd_frame(&original).unwrap()
                } else {
                    original.clone()
                };
                let (header, output, _) = convert(&source, compressed).unwrap();
                assert_eq!(header.version, 3);
                let decoded = bounded(
                    zstd::stream::read::Decoder::new(output.as_slice()).unwrap(),
                    MAX_LOG,
                )
                .unwrap();
                let text = String::from_utf8(decoded).unwrap();
                assert!(text.contains("reply"));
                if version < 3 {
                    assert!(text.contains("system/message"));
                    assert!(text.contains("Keep this policy"));
                }
            }
            assert_eq!(original, before);
        }
    }

    #[test]
    fn packed_legacy_runs_expand_before_normalization_and_preserve_offsets() {
        let mut source = String::from_utf8(log(0))
            .unwrap()
            .lines()
            .take(4)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        source.push(json!({"type":"text-chunks","seq0":3,"time0":4,"data":{"turn":1,"step":1,"index":0,"dt":[1,1],"texts":["one","two","three"]}}).to_string());
        source.push(
            json!({"type":"step/end","seq":6,"time":7,"data":{"turn":1,"step":1}}).to_string(),
        );
        source.push(json!({"type":"turn/end","seq":7,"time":8,"data":{"turn":1,"reason":{"kind":"completed"}}}).to_string());
        let (_, output, _) = convert((source.join("\n") + "\n").as_bytes(), false).unwrap();
        let decoded = bounded(
            zstd::stream::read::Decoder::new(output.as_slice()).unwrap(),
            MAX_LOG,
        )
        .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert_eq!(text.matches("assistant/chunk").count(), 3);
        assert!(text.contains("three"));
    }

    #[test]
    fn provenance_ranges_and_unknown_required_events_fail_closed() {
        let mut row = json!({"seq":5,"sourceEventSeqs":[[0,2],4]});
        provenance(&mut row).unwrap();
        assert_eq!(row["sourceEventSeqs"], json!([0, 1, 2, 4]));
        for ranges in [json!([[0, 5]]), json!([[0, 2], 2]), json!([[3, 4], 1])] {
            assert!(provenance(&mut json!({"seq":5,"sourceEventSeqs":ranges})).is_err());
        }
        let mut bad = log(3);
        bad.extend(b"{\"type\":\"future/required\",\"seq\":7,\"time\":8,\"data\":{}}\n");
        assert!(
            convert(&bad, false)
                .unwrap_err()
                .contains("unsupported required")
        );
        assert!(convert(&log(9), false).is_err());
        let mut incomplete = log(3);
        incomplete.pop();
        assert!(convert(&incomplete, false).is_err());
    }

    #[test]
    fn export_zip_restores_content_addressed_images_without_extracting_paths() {
        use sha2::Digest;
        let root =
            std::env::temp_dir().join(format!("dsh-import-zip-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let image: Vec<u8> = vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207,
            192, 240, 31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
            96, 130,
        ];
        let id = format!("sha256:{:x}", sha2::Sha256::digest(&image));
        let raw = String::from_utf8(log(3)).unwrap();
        let mut rows: Vec<Value> = raw
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        rows[1]["data"]["content"].as_array_mut().unwrap().push(json!({"type":"image","attachment":{"attachmentId":id,"mediaType":"image/png","bytes":image.len(),"width":1,"height":1,"name":"frame.png"}}));
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("session.jsonl", options).unwrap();
        for row in rows {
            writeln!(archive, "{row}").unwrap();
        }
        archive
            .start_file(format!("media/{id}.png"), options)
            .unwrap();
        archive.write_all(&image).unwrap();
        archive.start_file("../../outside.txt", options).unwrap();
        archive.write_all(b"must not be extracted").unwrap();
        let source = root.join("export.zip");
        std::fs::write(&source, archive.finish().unwrap().into_inner()).unwrap();
        let home = root.join("home");
        assert_eq!(import(&source, &home).unwrap(), 1);
        let digest = id.strip_prefix("sha256:").unwrap();
        assert_eq!(
            std::fs::read(
                home.join("attachments/v1/objects")
                    .join(&digest[..2])
                    .join(digest)
            )
            .unwrap(),
            image
        );
        assert!(!root.join("outside.txt").exists());
        assert_eq!(import(&source, &home).unwrap(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_preflight_retry_and_conflict_preserve_files() {
        let root = std::env::temp_dir().join(format!("dsh-import-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let home = root.join("home");
        std::fs::create_dir_all(&source).unwrap();
        let original = log(1);
        std::fs::write(source.join("a.jsonl"), &original).unwrap();
        std::fs::write(source.join("b.jsonl"), log(9)).unwrap();
        assert!(import(&source, &home).is_err());
        assert!(!home.exists());
        std::fs::remove_file(source.join("b.jsonl")).unwrap();
        assert_eq!(import(&source, &home).unwrap(), 1);
        assert_eq!(import(&source, &home).unwrap(), 1);
        assert_eq!(std::fs::read(source.join("a.jsonl")).unwrap(), original);
        let mut targets = Vec::new();
        collect(&home, &mut targets, 0).unwrap();
        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(
            std::fs::read(target.parent().unwrap().join("import-source.original")).unwrap(),
            original
        );
        std::fs::write(target, b"existing distinct content").unwrap();
        assert!(import(&source, &home).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"existing distinct content");
        std::fs::remove_dir_all(root).unwrap();
    }
}
