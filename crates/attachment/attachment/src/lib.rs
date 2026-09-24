//! Durable attachment storage seam (`ctx.attachments`). Rust port of
//! `packages/attachment/attachment/src/index.ts` +
//! `brand.ts` + `error.ts` + `types.ts`.

pub mod invariant;
mod references;
pub use references::{file_references_for_event, find_image_reference};

use dsh_brand::Branded;
use serde::{Deserialize, Serialize};

/// The brand marker for [`AttachmentId`].
#[doc(hidden)]
pub enum AttachmentIdTag {}

/// Opaque content-addressed identifier for one immutable attachment object.
pub type AttachmentId = Branded<AttachmentIdTag>;

/// Verbatim bytes with an opaque content identity and a sanitized display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttachmentRef {
    pub attachment_id: AttachmentId,
    pub name: String,
    pub bytes: u64,
}

/// The brand marker for one deterministic model-request image variant.
#[doc(hidden)]
pub enum ImageVariantIdTag {}

/// Complete request transformation identity used by provider upload caches.
pub type ImageVariantId = Branded<ImageVariantIdTag>;

/// Brand a validated storage identifier (TS `AttachmentId`).
pub fn attachment_id(value: impl Into<String>) -> AttachmentId {
    AttachmentId::new(value)
}

/// Brand a validated request-image variant digest.
pub fn image_variant_id(value: impl Into<String>) -> ImageVariantId {
    ImageVariantId::new(value)
}

/// Provider-independent request-image transformation policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestImagePolicy {
    pub max_pixels: u64,
    pub max_bytes: u64,
    pub preferred_media_type: ImageMediaType,
}

/// Derive the stable cache identity for one attachment and route policy.
pub fn request_image_variant_id(
    attachment: &ImageAttachmentRef,
    policy: &RequestImagePolicy,
) -> ImageVariantId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"dsh-request-image-v2\0");
    hasher.update(attachment.attachment_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(policy.max_pixels.to_le_bytes());
    hasher.update(policy.max_bytes.to_le_bytes());
    hasher.update(policy.preferred_media_type.as_str().as_bytes());
    let digest = hasher.finalize();
    image_variant_id(format!("sha256:{digest:x}"))
}

/// Stable failures suitable for host RPC error mapping (the TS
/// `AttachmentError`; the `HarnessError` base is re-implemented to avoid the
/// llm→attachment dependency cycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentError {
    /// Stable machine-routing failure code.
    pub code: String,
    /// Human-readable failure description without raw bytes or host paths.
    pub message: String,
}

impl AttachmentError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AttachmentError {}

/// Raster image formats accepted by the version-one attachment path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/gif")]
    Gif,
}

impl ImageMediaType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ImageMediaType::Png => "image/png",
            ImageMediaType::Jpeg => "image/jpeg",
            ImageMediaType::Webp => "image/webp",
            ImageMediaType::Gif => "image/gif",
        }
    }
}

/// Durable, serializable metadata for one immutable image object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentRef {
    /// Opaque storage identifier; never a filesystem path or bearer URL.
    pub attachment_id: AttachmentId,
    /// Media type verified from the stored bytes.
    pub media_type: ImageMediaType,
    /// Exact encoded byte length.
    pub bytes: u64,
    /// Intrinsic encoded width in pixels.
    pub width: u64,
    /// Intrinsic encoded height in pixels.
    pub height: u64,
    /// Optional display name stripped of local path information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Deployment-resolved limits used by upload admission and request
/// buffering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachmentLimits {
    /// Maximum encoded bytes, or zero for no local byte limit.
    pub max_image_bytes: u64,
    pub max_images_per_message: u64,
    /// Maximum aggregate encoded bytes, or zero for no local byte limit.
    pub max_message_image_bytes: u64,
    pub max_image_pixels: u64,
    pub media_types: Vec<ImageMediaType>,
}

impl ImageAttachmentLimits {
    pub fn image_byte_limit(&self) -> u64 {
        if self.max_image_bytes == 0 {
            u64::MAX
        } else {
            self.max_image_bytes
        }
    }

    pub fn message_byte_limit(&self) -> u64 {
        if self.max_message_image_bytes == 0 {
            u64::MAX
        } else {
            self.max_message_image_bytes
        }
    }
}

/// Request to validate and durably commit one image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveImageAttachment {
    pub data: Vec<u8>,
    /// Caller-declared media type, checked against fully decoded bytes.
    pub media_type: ImageMediaType,
    /// Optional browser/provider display name; never interpreted as a path.
    pub name: Option<String>,
}

/// Stored image bytes returned after reference and digest verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImageAttachment {
    pub reference: ImageAttachmentRef,
    pub data: Vec<u8>,
}

/// Deterministic provider-request image derived from one durable master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestImageAttachment {
    pub attachment_id: AttachmentId,
    pub variant_id: ImageVariantId,
    pub media_type: ImageMediaType,
    pub data: Vec<u8>,
    pub width: u64,
    pub height: u64,
}

/// A bounded reader retaining the backend's immutable file lease.
pub type AttachmentReader = std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>;
pub type FileUploadReader<'a> = std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send + 'a>>;
pub struct ImageAttachmentStream {
    pub reference: ImageAttachmentRef,
    pub reader: AttachmentReader,
}
pub struct FileAttachmentStream {
    pub reference: FileAttachmentRef,
    pub reader: AttachmentReader,
}
pub struct RequestImageStream {
    pub attachment_id: AttachmentId,
    pub variant_id: ImageVariantId,
    pub media_type: ImageMediaType,
    pub bytes: u64,
    pub width: u64,
    pub height: u64,
    pub reader: AttachmentReader,
}

/// The cancellation seam for backend read and verification work (TS
/// `AbortSignal`; the workspace predicate convention carries no reason, so
/// an abort surfaces as `ATTACHMENT_ABORTED`).
pub type AttachmentAbort = Arc<dyn Fn() -> bool + Send + Sync>;

use std::sync::Arc;

/// Immutable binary attachment service. Implementations validate bytes
/// before publishing a reference (TS `AttachmentStore`).
#[async_trait::async_trait]
pub trait AttachmentStore: Send + Sync + 'static {
    /// Publish verbatim file bytes without buffering the complete input.
    async fn save_file_stream(
        &self,
        _reader: FileUploadReader<'_>,
        _name: String,
        _signal: Option<&AttachmentAbort>,
    ) -> Result<FileAttachmentRef, AttachmentError> {
        Err(AttachmentError::new(
            "FILE_ATTACHMENTS_UNAVAILABLE",
            "This attachment backend does not store files.",
        ))
    }

    /// Verify content identity before returning a reader with its immutable lease.
    async fn open_file(
        &self,
        _reference: &FileAttachmentRef,
        _signal: Option<&AttachmentAbort>,
    ) -> Result<FileAttachmentStream, AttachmentError> {
        Err(AttachmentError::new(
            "FILE_ATTACHMENTS_UNAVAILABLE",
            "This attachment backend does not read files.",
        ))
    }

    /// Local read-only handle path; callers must authorize the reference first.
    fn file_host_path(&self, _reference: &FileAttachmentRef) -> Option<std::path::PathBuf> {
        None
    }

    /// Deployment-resolved image policy used by authoritative and fast-path
    /// validation.
    fn image_limits(&self) -> &ImageAttachmentLimits;

    /// Validate one image without persisting it.
    async fn validate_image(&self, input: &SaveImageAttachment) -> Result<(), AttachmentError>;

    /// Validate and durably commit one image before its owning session event
    /// is appended.
    async fn save_image(
        &self,
        input: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError>;

    /// File/stream admission. The default compatibility implementation buffers;
    /// first-party storage overrides it with bounded staging and worker decode.
    async fn save_image_stream(
        &self,
        mut reader: AttachmentReader,
        media_type: ImageMediaType,
        name: Option<String>,
        signal: Option<&AttachmentAbort>,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        use tokio::io::AsyncReadExt;
        let mut data = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            if signal.is_some_and(|signal| signal()) {
                return Err(AttachmentError::new(
                    "ATTACHMENT_ABORTED",
                    "Image processing cancelled.",
                ));
            }
            let count = reader
                .read(&mut buffer)
                .await
                .map_err(|e| AttachmentError::new("ATTACHMENT_READ_FAILED", e.to_string()))?;
            if count == 0 {
                break;
            }
            if (data.len() as u64).saturating_add(count as u64)
                > self.image_limits().image_byte_limit()
            {
                return Err(AttachmentError::new(
                    "IMAGE_TOO_LARGE",
                    "Image exceeds the configured byte limit.",
                ));
            }
            data.extend_from_slice(&buffer[..count]);
        }
        self.save_image(&SaveImageAttachment {
            data,
            media_type,
            name,
        })
        .await
    }

    /// Validate the entire batch before publishing any object, then preserve
    /// input order in the returned references.
    async fn save_images(
        &self,
        inputs: &[SaveImageAttachment],
    ) -> Result<Vec<ImageAttachmentRef>, AttachmentError> {
        for input in inputs {
            self.validate_image(input).await?;
        }
        let mut references = Vec::with_capacity(inputs.len());
        for input in inputs {
            references.push(self.save_image(input).await?);
        }
        Ok(references)
    }

    /// Read one image and verify that bytes still match the recorded
    /// reference.
    async fn read_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError>;

    /// Backends may override the buffering compatibility path with a verified,
    /// immutable reader. Consumers should not collect this reader without need.
    async fn open_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<ImageAttachmentStream, AttachmentError> {
        let stored = self.read_image(reference, signal).await?;
        Ok(ImageAttachmentStream {
            reference: stored.reference,
            reader: Box::pin(std::io::Cursor::new(stored.data)),
        })
    }

    async fn open_image_request(
        &self,
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
        signal: Option<&AttachmentAbort>,
    ) -> Result<RequestImageStream, AttachmentError> {
        let stored = self.read_image_request(reference, policy, signal).await?;
        Ok(RequestImageStream {
            attachment_id: stored.attachment_id,
            variant_id: stored.variant_id,
            media_type: stored.media_type,
            bytes: stored.data.len() as u64,
            width: stored.width,
            height: stored.height,
            reader: Box::pin(std::io::Cursor::new(stored.data)),
        })
    }

    /// Read or derive one deterministic model-request image under route budgets.
    async fn read_image_request(
        &self,
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
        signal: Option<&AttachmentAbort>,
    ) -> Result<RequestImageAttachment, AttachmentError> {
        if reference.width * reference.height > policy.max_pixels
            || reference.bytes > policy.max_bytes
            || reference.media_type != policy.preferred_media_type
        {
            return Err(AttachmentError::new(
                "REQUEST_IMAGE_TRANSFORM_UNAVAILABLE",
                "This attachment backend cannot derive the requested image variant.",
            ));
        }
        let stored = self.read_image(reference, signal).await?;
        Ok(RequestImageAttachment {
            attachment_id: reference.attachment_id.clone(),
            variant_id: request_image_variant_id(reference, policy),
            media_type: reference.media_type,
            data: stored.data,
            width: reference.width,
            height: reference.height,
        })
    }
}

impl cordis::Service for dyn AttachmentStore {
    fn service_name(&self) -> &'static str {
        "attachments"
    }
}
