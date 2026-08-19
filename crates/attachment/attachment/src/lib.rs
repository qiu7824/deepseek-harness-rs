//! Durable attachment storage seam (`ctx.attachments`). Rust port of
//! `packages/attachment/attachment/src/index.ts` +
//! `brand.ts` + `error.ts` + `types.ts`.

pub mod invariant;

use dsh_brand::Branded;
use serde::{Deserialize, Serialize};

/// The brand marker for [`AttachmentId`].
#[doc(hidden)]
pub enum AttachmentIdTag {}

/// Opaque content-addressed identifier for one immutable attachment object.
pub type AttachmentId = Branded<AttachmentIdTag>;

/// Brand a validated storage identifier (TS `AttachmentId`).
pub fn attachment_id(value: impl Into<String>) -> AttachmentId {
    AttachmentId::new(value)
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
    pub max_image_bytes: u64,
    pub max_images_per_message: u64,
    pub max_message_image_bytes: u64,
    pub max_image_pixels: u64,
    pub media_types: Vec<ImageMediaType>,
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

/// The cancellation seam for backend read and verification work (TS
/// `AbortSignal`; the workspace predicate convention carries no reason, so
/// an abort surfaces as `ATTACHMENT_ABORTED`).
pub type AttachmentAbort = Arc<dyn Fn() -> bool + Send + Sync>;

use std::sync::Arc;

/// Immutable binary attachment service. Implementations validate bytes
/// before publishing a reference (TS `AttachmentStore`).
#[async_trait::async_trait]
pub trait AttachmentStore: Send + Sync + 'static {
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

    /// Read one image and verify that bytes still match the recorded
    /// reference.
    async fn read_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError>;
}

impl cordis::Service for dyn AttachmentStore {
    fn service_name(&self) -> &'static str {
        "attachments"
    }
}
