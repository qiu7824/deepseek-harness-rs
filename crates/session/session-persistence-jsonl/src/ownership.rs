use super::*;
use crate::generations::{SessionGenerationLease, select_generation};
use std::sync::{Weak, atomic::Ordering};

pub(super) const WRITER_DIRECTORY: &str = ".session-writer-leases";

pub(super) struct WriterOwnership {
    // The root-scoped identity lease also excludes same-id/different-cwd races.
    _identity: SessionGenerationLease,
    pub(super) session: SessionGenerationLease,
}

impl JsonlSessionPersistence {
    pub(super) async fn check_writer_available(&self, id: &SessionId) -> Result<(), String> {
        let writers = self.writers.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("Session persistence is closed; no writer was admitted".into());
        }
        if writers.get(id.as_str()).and_then(Weak::upgrade).is_some() {
            return Ok(());
        }
        let path = self
            .root
            .join(WRITER_DIRECTORY)
            .join(crate::format::encode_segment(id.as_str())?);
        tokio::task::spawn_blocking(move || SessionGenerationLease::acquire(&path).map(drop))
            .await
            .map_err(|e| e.to_string())?
    }

    pub(super) async fn writer_lease(
        &self,
        meta: &SessionHeader,
    ) -> Result<Arc<WriterOwnership>, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("Session persistence is closed; no writer was admitted".into());
        }
        let directory = session_dir(&self.root.to_string_lossy(), meta.cwd.as_deref(), &meta.id);
        let mut writers = self.writers.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("Session persistence is closed; no writer was admitted".into());
        }
        if let Some(existing) = writers.get(meta.id.as_str()).and_then(Weak::upgrade) {
            let canonical = std::fs::canonicalize(&directory).map_err(|e| e.to_string())?;
            if existing.session.directory() != canonical {
                return Err("Session writer is reserved at a different storage location".into());
            }
            return Ok(existing);
        }
        let identity = self
            .root
            .join(WRITER_DIRECTORY)
            .join(crate::format::encode_segment(meta.id.as_str())?);
        let id = meta.id.clone();
        let compression = self.compression;
        let lease = tokio::task::spawn_blocking(move || {
            let identity = SessionGenerationLease::acquire(&identity)?;
            let session = SessionGenerationLease::acquire(&directory)?;
            check_legacy_authority(session.directory(), &id, compression)?;
            Ok::<_, String>(Arc::new(WriterOwnership {
                _identity: identity,
                session,
            }))
        })
        .await
        .map_err(|e| e.to_string())??;
        // close() waits on this gate, so an admission cannot be published late.
        writers.retain(|_, value| value.strong_count() > 0);
        writers.insert(meta.id.as_str().to_owned(), Arc::downgrade(&lease));
        Ok(lease)
    }

    pub(super) async fn assert_legacy_authority(
        &self,
        directory: &Path,
        id: &SessionId,
    ) -> Result<(), String> {
        let directory = directory.to_owned();
        let id = id.clone();
        let compression = self.compression;
        tokio::task::spawn_blocking(move || check_legacy_authority(&directory, &id, compression))
            .await
            .map_err(|e| e.to_string())?
    }
}

fn check_legacy_authority(
    directory: &Path,
    id: &SessionId,
    compression: JsonlCompression,
) -> Result<(), String> {
    let Some(selected) = select_generation(directory, id.as_str())? else {
        return Ok(());
    };
    if ![0, 3, 4].contains(&selected.version) {
        return Err(dsh_session_persistence::session_format_version_refusal(
            id,
            selected.version,
        ));
    }
    let _ = compression;
    Ok(())
}
