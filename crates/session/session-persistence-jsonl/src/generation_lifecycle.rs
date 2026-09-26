//! Adjacent upgrades under the same ownership used by the normal writer.
use super::*;
use crate::generations::{
    generation_path, migrate_recovered_v3_to_v4, migrate_v3_to_v4, select_generation,
};

/// Catalog discovery needs only the child's own descriptor count and first
/// descriptor. Never retain its conversation or expand packed token runs.
fn read_child_descriptor_fact(
    path: &Path,
    header: &SessionHeader,
) -> Result<serde_json::Value, String> {
    let first_line = read_authority_header(path, crate::format::compression_of(path))?
        .ok_or("missing historical child header")?;
    let storage = parse_header_storage(&first_line).ok_or("invalid historical child header")?;
    if storage.meta.id != header.id || storage.meta.created_at != header.created_at {
        return Err("historical child identity changed during discovery".into());
    }
    let mut count = 0usize;
    let mut first = None;
    let native = storage.meta.version == 4;
    let mut accept = |event: SessionEvent| {
        if native && event.type_ == "session/end-seed" && event.data["inherited"] == true {
            count = 0;
            first = None;
        }
        if event.seq.get() >= storage.inherited_event_count.get()
            && event.type_ == "subagent/descriptor"
        {
            if count == 0 {
                first = Some(event.data);
            }
            count = count.saturating_add(1);
        }
        Ok(true)
    };
    if native {
        crate::native_reader::visit(path, header.id.as_str(), &|| false, |event| {
            accept(event).map(|_| ())
        })?;
    } else if crate::format::compression_of(path) == JsonlCompression::Zstd {
        stream_zstd_nonpacked_events(path, &mut accept)?;
    } else {
        stream_jsonl_nonpacked_events(path, &mut accept)?;
    }
    Ok(
        serde_json::json!({"childId":header.id,"childCreatedAt":header.created_at,"descriptorCount":count,"descriptor":first}),
    )
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn streamed_child_facts_exclude_inherited_descriptors_and_preserve_sources() {
        for native in [false, true] {
            for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
                let root =
                    std::env::temp_dir().join(format!("child-facts-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir(&root).unwrap();
                let path = root.join(format!("session{}", crate::log_suffix(compression)));
                let header: SessionHeader = serde_json::from_value(json!({"version":if native {4} else {3},"id":"child","createdAt":1,"cwd":"C:/workspace","isSeeded":true,"parentSession":"parent","origin":"subagent","delegationDepth":1})).unwrap();
                let events: Vec<SessionEvent> = vec![
                    json!({"type":"subagent/descriptor","seq":0,"time":1,"data":{"version":1,"provider":"spawn","label":"inherited"}}),
                    json!({"type":"session/end-seed","seq":1,"time":2,"data":if native {json!({"inherited":true})} else {json!({})}}),
                    json!({"type":"subagent/descriptor","seq":2,"time":3,"data":{"version":1,"provider":"spawn","label":"own"}}),
                ].into_iter().map(|event| serde_json::from_value(event).unwrap()).collect();
                let (first, body) = if native {
                    let vocabulary = dsh_session::format_v4::V4Vocabulary::default();
                    let first = dsh_session::format_v4::encode_v4_header(
                        serde_json::to_value(&header).unwrap(),
                        1,
                    )
                    .unwrap();
                    let rows: Vec<Value> = events
                        .iter()
                        .map(|event| {
                            dsh_session::format_v4::encode_v4_event(
                                serde_json::to_value(event).unwrap(),
                                &vocabulary,
                            )
                            .unwrap()
                        })
                        .collect();
                    (
                        first.to_string() + "\n",
                        rows.iter()
                            .map(|row| row.to_string() + "\n")
                            .collect::<String>(),
                    )
                } else {
                    let first = serde_json::to_string(
                        &to_header_line(&header, Some(SessionLogOffset::new(1).unwrap())).unwrap(),
                    )
                    .unwrap();
                    (first + "\n", event_lines(&events, true) + "\n")
                };
                let bytes = match compression {
                    JsonlCompression::None => [first.as_bytes(), body.as_bytes()].concat(),
                    JsonlCompression::Zstd => [
                        compress_zstd_frame(first.as_bytes()).unwrap(),
                        compress_zstd_frame(body.as_bytes()).unwrap(),
                    ]
                    .concat(),
                };
                std::fs::write(&path, &bytes).unwrap();
                let fact = read_child_descriptor_fact(&path, &header).unwrap();
                assert_eq!(fact["descriptorCount"], 1);
                assert_eq!(fact["descriptor"]["label"], "own");
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}

impl JsonlSessionPersistence {
    pub(super) async fn upgrade_v0(&self, path: &Path, id: &SessionId) -> Result<PathBuf, String> {
        let first = read_authority_header(path, crate::format::compression_of(path))?
            .ok_or("missing historical Session header")?;
        let meta = parse_header_meta(&first).ok_or("invalid historical Session header")?;
        if meta.version != 0 {
            return Ok(path.to_owned());
        }
        let _gate = self.migration_gate.lock().await;
        let writer = self.writer_lease(&meta).await?;
        let selected = select_generation(writer.session.directory(), id.as_str())?
            .ok_or("Session generation disappeared")?;
        if selected.version != 0 {
            return Ok(selected.path);
        }
        let mut source =
            crate::generations::StableGenerationSource::open(&selected.path, &|| false)?;
        let prefix = self.read_prefix(&selected.path, Some(id)).await?;
        let content = self.encode_materialization_as(
            &prefix.meta,
            prefix.inherited_event_count,
            &prefix.events,
            selected.compression,
        )?;
        let target = generation_path(writer.session.directory(), 3, selected.compression)?;
        let temporary = self.write_synced_temp_file(&target, &content).await?;
        let result = (|| {
            source.assert_unchanged(&|| false)?;
            if select_generation(writer.session.directory(), id.as_str())?.as_ref()
                != Some(&selected)
            {
                return Err("Session generation changed before V3 publication".into());
            }
            std::fs::hard_link(&temporary, &target).map_err(|error| {
                format!("V3 successor was not published; original retained: {error}")
            })
        })();
        let _ = tokio::fs::remove_file(&temporary).await;
        result?;
        Ok(target)
    }

    pub(super) async fn upgrade_to_current(
        &self,
        path: &Path,
        id: &SessionId,
    ) -> Result<PathBuf, String> {
        let path = self.upgrade_v0(path, id).await?;
        if crate::native_reader::is_native(&path)? {
            return Ok(path);
        }
        let _gate = self.migration_gate.lock().await;
        let first = read_authority_header(&path, crate::format::compression_of(&path))?
            .ok_or("missing V3 header")?;
        let meta = parse_header_meta(&first).ok_or("invalid V3 header")?;
        let writer = self.writer_lease(&meta).await?;
        let selected = select_generation(writer.session.directory(), id.as_str())?
            .ok_or("Session generation disappeared")?;
        if selected.version == 4 {
            return Ok(selected.path);
        }
        if selected.version != 3 {
            return Err("Session requires its adjacent historical migration".into());
        }
        let mut children = Vec::new();
        for (header, path) in self.list_artifacts().await? {
            if header.origin.as_deref() != Some("subagent")
                || header.parent_session.as_ref() != Some(id)
            {
                continue;
            }
            children.push(
                tokio::task::spawn_blocking(move || read_child_descriptor_fact(&path, &header))
                    .await
                    .map_err(|error| error.to_string())??,
            );
        }
        let mut retained =
            crate::generations::StableGenerationSource::open(&selected.path, &|| false)?;
        let expected = id.to_string();
        let lease = writer.clone();
        let facts = children.clone();
        let compression = selected.compression;
        let converted = tokio::task::spawn_blocking(move || {
            migrate_v3_to_v4(&lease.session, &expected, facts, compression, &|| false)
        })
        .await
        .map_err(|error| error.to_string())?;
        match converted {
            Ok(result) => Ok(result.path),
            Err(original_error) => {
                let recovered = self.read_prefix(&selected.path, Some(id)).await?;
                if recovered.torn_marker.is_none() {
                    return Err(original_error);
                }
                retained.assert_unchanged(&|| false)?;
                let expected = id.to_string();
                let result = tokio::task::spawn_blocking(move || {
                    migrate_recovered_v3_to_v4(
                        &writer.session,
                        &expected,
                        children,
                        compression,
                        &recovered.events,
                        &|| false,
                    )
                })
                .await
                .map_err(|error| error.to_string())??;
                Ok(result.path)
            }
        }
    }
}
