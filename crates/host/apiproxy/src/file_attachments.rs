use super::*;
use dsh_attachment::{AttachmentAbort, AttachmentError, AttachmentStore, FileAttachmentRef};
use dsh_session::SessionId;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "file_attachment_tests.rs"]
mod tests;

impl ApiProxyService {
    async fn find_file_attachment(
        &self,
        session: &SessionId,
        store: &Arc<dyn AttachmentStore>,
        expected: Option<&FileAttachmentRef>,
        path: Option<&Path>,
        signal: Option<&AttachmentAbort>,
    ) -> Result<Option<FileAttachmentRef>, AttachmentError> {
        let inspect = |events: &[dsh_session::SessionEvent]| {
            events
                .iter()
                .flat_map(|event| {
                    dsh_attachment::file_references_for_event(&event.type_, &event.data)
                })
                .find(|reference| {
                    expected.is_some_and(|expected| expected == reference)
                        || path.is_some_and(|path| {
                            store.file_host_path(reference).as_deref() == Some(path)
                        })
                })
        };
        if let Some(session) = self.sessions().and_then(|sessions| sessions.get(session)) {
            let mut found = None;
            session
                .visit_events(0, None, |event| {
                    if signal.is_some_and(|signal| signal()) {
                        return Err("File attachment lookup cancelled".into());
                    }
                    found = inspect(std::slice::from_ref(event));
                    Ok(found.is_none())
                })
                .map_err(|error| AttachmentError::new("ATTACHMENT_HISTORY_UNAVAILABLE", error))?;
            return Ok(found);
        }
        let persistence = self
            .ctx
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
            .map(|service| service.as_ref().clone())
            .ok_or_else(|| {
                AttachmentError::new(
                    "ATTACHMENT_NOT_REFERENCED",
                    "Session history is unavailable.",
                )
            })?;
        let found = Arc::new(Mutex::new(None));
        let answer = found.clone();
        let expected = expected.cloned();
        let path = path.map(Path::to_owned);
        let store = store.clone();
        let signal = signal.cloned();
        let visitor: dsh_session_persistence::NonpackedEventVisitor = Arc::new(move |events| {
            for event in events {
                if signal.as_ref().is_some_and(|signal| signal()) {
                    return Err("File attachment lookup cancelled".into());
                }
                for reference in
                    dsh_attachment::file_references_for_event(&event.type_, &event.data)
                {
                    if expected.as_ref() == Some(&reference)
                        || path.as_ref().is_some_and(|path| {
                            store.file_host_path(&reference).as_ref() == Some(path)
                        })
                    {
                        *answer.lock() = Some(reference);
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        });
        persistence
            .visit_nonpacked_events(session, visitor)
            .await
            .map_err(|error| AttachmentError::new("ATTACHMENT_HISTORY_UNAVAILABLE", error))?;
        let result = found.lock().clone();
        Ok(result)
    }

    /// Only an exact file already admitted into this Session can authorize an external preview path.
    pub async fn authorized_attachment_path(
        &self,
        session: &SessionId,
        path: &Path,
        signal: Option<&AttachmentAbort>,
    ) -> Result<Option<PathBuf>, AttachmentError> {
        let Some(store) = self
            .ctx
            .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
            .map(|service| service.as_ref().clone())
        else {
            return Ok(None);
        };
        let Some(reference) = self
            .find_file_attachment(session, &store, None, Some(path), signal)
            .await?
        else {
            return Ok(None);
        };
        let verified = store.open_file(&reference, signal).await?;
        let path = store.file_host_path(&reference);
        drop(verified);
        Ok(path)
    }

    pub(super) async fn session_file_attachment(
        &self,
        request: RpcRequest<crate::api::sessions::SessionFileAttachmentRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let result = async {
            let store = self
                .ctx
                .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
                .map(|service| service.as_ref().clone())
                .ok_or_else(|| {
                    AttachmentError::new(
                        "ATTACHMENT_SERVICE_UNAVAILABLE",
                        "File attachment storage is unavailable.",
                    )
                })?;
            let aborted: AttachmentAbort = Arc::new(move || signal.aborted());
            let reference = self
                .find_file_attachment(
                    &request.payload.session_id,
                    &store,
                    Some(&request.payload.attachment),
                    None,
                    Some(&aborted),
                )
                .await?
                .ok_or_else(|| {
                    AttachmentError::new(
                        "ATTACHMENT_NOT_REFERENCED",
                        "File is not referenced by this session.",
                    )
                })?;
            let verified = store.open_file(&reference, Some(&aborted)).await?;
            let path = store.file_host_path(&reference).ok_or_else(|| {
                AttachmentError::new(
                    "ATTACHMENT_PATH_UNAVAILABLE",
                    "File attachment has no local path.",
                )
            })?;
            drop(verified);
            Ok::<_, AttachmentError>(crate::api::sessions::SessionFileAttachmentResult {
                attachment: reference,
                path: path.to_string_lossy().into_owned(),
            })
        }
        .await;
        match result {
            Ok(value) => ok(request.rpc_id, value),
            Err(error) => err(
                request.rpc_id,
                RpcError::AttachmentError(RpcErrorBody {
                    message: error.message,
                    details: crate::api::rpc::ReasonDetails { reason: error.code },
                }),
            ),
        }
    }
}
