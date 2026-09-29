//! Session-owned executable copies of immutable image inputs.
use super::Resources;
use dsh_attachment::{AttachmentAbort, AttachmentError, ImageAttachmentStream, ImageMediaType};
use dsh_workspace_resources::Store;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn failure(message: impl Into<String>) -> AttachmentError {
    AttachmentError::new("ATTACHMENT_INPUT_UNAVAILABLE", message)
}

async fn copy_image(
    store: Arc<Store>,
    owner: &str,
    project: &str,
    mut image: ImageAttachmentStream,
    signal: Option<&AttachmentAbort>,
) -> Result<PathBuf, AttachmentError> {
    let check_cancel = || {
        if signal.is_some_and(|signal| signal()) {
            Err(AttachmentError::new(
                "ATTACHMENT_ABORTED",
                "Image input preparation cancelled.",
            ))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let mut lease = store
        .allocate(owner, project, "copy", "图片附件输入")
        .map_err(failure)?;
    let extension = match image.reference.media_type {
        ImageMediaType::Png => "png",
        ImageMediaType::Jpeg => "jpg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Gif => "gif",
    };
    let target = store
        .path(lease.id(), &format!("input.{extension}"))
        .map_err(failure)?;
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .await
        .map_err(|error| failure(error.to_string()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    let mut hash = Sha256::new();
    loop {
        check_cancel()?;
        let count = image
            .reader
            .read(&mut buffer)
            .await
            .map_err(|error| failure(error.to_string()))?;
        if count == 0 {
            break;
        }
        copied = copied.saturating_add(count as u64);
        if copied > image.reference.bytes {
            return Err(failure("Image input exceeds its recorded size."));
        }
        hash.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .await
            .map_err(|error| failure(error.to_string()))?;
    }
    check_cancel()?;
    if copied != image.reference.bytes
        || format!("sha256:{:x}", hash.finalize()) != image.reference.attachment_id.as_str()
    {
        return Err(failure(
            "Image input does not match its immutable attachment.",
        ));
    }
    output
        .sync_all()
        .await
        .map_err(|error| failure(error.to_string()))?;
    drop(output);
    lease.finish(true).map_err(failure)?;
    Ok(target)
}

#[async_trait::async_trait]
impl dsh_attachment::SessionAttachmentMaterializer for Resources {
    async fn materialize_image(
        &self,
        owner: &str,
        workspace: Option<&str>,
        image: ImageAttachmentStream,
        signal: Option<&AttachmentAbort>,
    ) -> Result<PathBuf, AttachmentError> {
        let store = match workspace {
            Some(workspace) => self.current_for(workspace),
            None => self.current(),
        }
        .map_err(failure)?;
        copy_image(store, owner, workspace.unwrap_or(""), image, signal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_attachment::{ImageAttachmentRef, attachment_id};

    fn image(data: &[u8]) -> ImageAttachmentStream {
        ImageAttachmentStream {
            reference: ImageAttachmentRef {
                attachment_id: attachment_id(format!("sha256:{:x}", Sha256::digest(data))),
                media_type: ImageMediaType::Png,
                bytes: data.len() as u64,
                width: 1,
                height: 1,
                name: Some("../../foreign.png".into()),
            },
            reader: Box::pin(std::io::Cursor::new(data.to_vec())),
        }
    }

    #[tokio::test]
    async fn image_inputs_are_distinct_owned_copies_and_never_use_the_attachment_name_as_a_path() {
        let root = std::env::temp_dir().join(format!("dsh-image-inputs-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&root).unwrap();
        let first = copy_image(store.clone(), "owner", "", image(b"verified image"), None)
            .await
            .unwrap();
        let second = copy_image(store.clone(), "other", "", image(b"verified image"), None)
            .await
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(first.file_name().unwrap(), "input.png");
        assert_eq!(std::fs::read(&first).unwrap(), b"verified image");
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            let path = store.path(&row.id, "input.png").unwrap();
            assert_eq!(row.owner, if path == first { "owner" } else { "other" });
        }
        std::fs::write(&first, b"working edit").unwrap();
        assert_eq!(std::fs::read(&second).unwrap(), b"verified image");
        assert!(!root.join("foreign.png").exists());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn image_inputs_reject_cancellation_truncation_and_changed_bytes() {
        let root =
            std::env::temp_dir().join(format!("dsh-image-input-failure-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&root).unwrap();
        let cancelled: AttachmentAbort = Arc::new(|| true);
        assert!(
            copy_image(
                store.clone(),
                "owner",
                "",
                image(b"original"),
                Some(&cancelled)
            )
            .await
            .is_err()
        );
        assert!(store.list().unwrap().is_empty());
        for replacement in [
            b"changed!".to_vec(),
            b"short".to_vec(),
            b"longer-than-original".to_vec(),
        ] {
            let mut input = image(b"original");
            input.reader = Box::pin(std::io::Cursor::new(replacement));
            assert!(
                copy_image(store.clone(), "owner", "", input, None)
                    .await
                    .is_err()
            );
        }
        assert_eq!(store.list().unwrap().len(), 3);
        assert!(
            store
                .list()
                .unwrap()
                .iter()
                .all(|row| row.state == "interrupted")
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
