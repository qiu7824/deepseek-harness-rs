//! Request-only file handles; durable messages retain their structured references.
use crate::{ContentBlock, GenerateOptions, LlmFailure};
use cordis::Context;
use dsh_attachment::AttachmentStore;
use std::{collections::HashMap, sync::Arc};

fn failure(code: &str, message: impl Into<String>) -> LlmFailure {
    LlmFailure {
        code: code.into(),
        message: message.into(),
        offload_images: None,
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}
fn references(blocks: &[ContentBlock], files: &mut Vec<dsh_attachment::FileAttachmentRef>) {
    for block in blocks {
        match block {
            ContentBlock::File { attachment } if !files.contains(attachment) => {
                files.push(attachment.clone())
            }
            ContentBlock::ToolResult { content, .. } => references(content, files),
            _ => {}
        }
    }
}
fn replace(blocks: &mut [ContentBlock], handles: &HashMap<(String, String, u64), String>) {
    for block in blocks {
        match block {
            ContentBlock::File { attachment } => {
                let key = (
                    attachment.attachment_id.to_string(),
                    attachment.name.clone(),
                    attachment.bytes,
                );
                *block = ContentBlock::Text {
                    text: handles[&key].clone(),
                };
            }
            ContentBlock::ToolResult { content, .. } => replace(content, handles),
            _ => {}
        }
    }
}
pub(crate) async fn project(
    ctx: &Context,
    options: &mut GenerateOptions,
) -> Result<(), LlmFailure> {
    let mut files = Vec::new();
    for message in &options.messages {
        references(&message.content, &mut files);
    }
    if files.is_empty() {
        return Ok(());
    }
    let store = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .map(|service| service.as_ref().clone())
        .ok_or_else(|| {
            failure(
                "ATTACHMENT_SERVICE_UNAVAILABLE",
                "File references require the attachment service.",
            )
        })?;
    let mut handles = HashMap::new();
    for reference in files {
        let verified = store
            .open_file(&reference, options.signal.as_ref())
            .await
            .map_err(|error| failure(&error.code, error.message))?;
        let path = store.file_host_path(&reference).ok_or_else(|| {
            failure(
                "ATTACHMENT_PATH_UNAVAILABLE",
                "File attachment has no local read-only handle.",
            )
        })?;
        let text = format!(
            "Attached file: {}\nPath: {}\nSize: {} bytes\nThis is a read-only input; write edited documents to a new output path.",
            reference.name,
            path.display(),
            reference.bytes
        );
        handles.insert(
            (
                reference.attachment_id.to_string(),
                reference.name,
                reference.bytes,
            ),
            text,
        );
        drop(verified);
    }
    for message in &mut options.messages {
        replace(&mut message.content, &handles);
    }
    Ok(())
}
