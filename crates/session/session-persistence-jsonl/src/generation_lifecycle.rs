//! Adjacent upgrades under the same ownership used by the normal writer.
use super::*;
use crate::generations::{
    generation_path, migrate_recovered_v3_to_v4, migrate_v3_to_v4, select_generation,
};

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
            let child = self.read_prefix(&path, Some(&header.id)).await?;
            let mut descriptors = child.events.iter().filter(|event| {
                event.seq.get() >= child.inherited_event_count.get()
                    && event.type_ == "subagent/descriptor"
            });
            let first = descriptors.next().map(|event| event.data.clone());
            let count = usize::from(first.is_some()) + descriptors.count();
            children.push(serde_json::json!({"childId":header.id,"childCreatedAt":header.created_at,"descriptorCount":count,"descriptor":first}));
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
