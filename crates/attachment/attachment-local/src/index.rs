//! Local durable attachment backend rooted below `DSH_HOME`. Rust port of
//! `packages/attachment/attachment-local/src/index.ts`.

use std::path::PathBuf;
use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_attachment::{
    AttachmentAbort, AttachmentError, AttachmentStore, ImageAttachmentLimits, ImageAttachmentRef,
    ImageMediaType, RequestImageAttachment, RequestImagePolicy, SaveImageAttachment,
    StoredImageAttachment,
};
use dsh_home_paths::resolve_dsh_home;

pub use crate::image::{DetectedImage, detect_image, encoded_alpha_is_compatible, probe_image};
pub use crate::store::{read_image_file, save_image_file, validate_image_file};

/// Zero disables the local encoded-byte limit; provider request limits remain separate.
pub const DEFAULT_MAX_IMAGE_BYTES: u64 = 0;
/// Default maximum images in one prompt.
pub const DEFAULT_MAX_IMAGES_PER_MESSAGE: u64 = 20;
/// Default maximum aggregate image bytes in one prompt.
pub const DEFAULT_MAX_MESSAGE_IMAGE_BYTES: u64 = 0;
/// Default maximum intrinsic pixels for one image.
pub const DEFAULT_MAX_IMAGE_PIXELS: u64 = 40_000_000;

/// Local attachment backend configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Explicit harness home; omitted follows `DSH_HOME`, then `~/.dsh`.
    pub dsh_home: Option<String>,
    /// Maximum encoded bytes accepted for one image; zero means unlimited.
    pub max_image_bytes: Option<u64>,
    /// Maximum image count accepted in one submitted message.
    pub max_images_per_message: Option<u64>,
    /// Maximum aggregate encoded image bytes accepted in one submitted
    /// message; zero means unlimited.
    pub max_message_image_bytes: Option<u64>,
    /// Maximum intrinsic width multiplied by height accepted for one image.
    pub max_image_pixels: Option<u64>,
}

/// The schemastery config schema (TS `LocalAttachmentStore.Config`).
pub fn config_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::{Data, Schema};
    use indexmap::IndexMap;
    Schema::object(IndexMap::from([
        (
            "maxImageBytes".to_string(),
            Schema::number()
                .step(1.0)
                .min(0.0)
                .default(Data::Number(DEFAULT_MAX_IMAGE_BYTES as f64)),
        ),
        (
            "maxImagesPerMessage".to_string(),
            Schema::number()
                .step(1.0)
                .min(1.0)
                .default(Data::Number(DEFAULT_MAX_IMAGES_PER_MESSAGE as f64)),
        ),
        (
            "maxMessageImageBytes".to_string(),
            Schema::number()
                .step(1.0)
                .min(0.0)
                .default(Data::Number(DEFAULT_MAX_MESSAGE_IMAGE_BYTES as f64)),
        ),
        (
            "maxImagePixels".to_string(),
            Schema::number()
                .step(1.0)
                .min(1.0)
                .default(Data::Number(DEFAULT_MAX_IMAGE_PIXELS as f64)),
        ),
    ]))
}

/// Persistent content-addressed local attachment store (TS
/// `LocalAttachmentStore`).
pub struct LocalAttachmentStore {
    /// Absolute versioned storage root.
    pub root: PathBuf,
    limits: ImageAttachmentLimits,
}

#[cfg(test)]
mod upload_limit_tests {
    use super::*;
    #[tokio::test]
    async fn default_accepts_images_over_five_mib_and_explicit_limit_is_enforced() {
        let mut data = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut data, image::ImageFormat::Png)
            .unwrap();
        let mut data = data.into_inner();
        data.resize(6 * 1024 * 1024, 0);
        let input = SaveImageAttachment {
            data,
            media_type: ImageMediaType::Png,
            name: None,
        };
        let ctx = Context::root();
        let store = LocalAttachmentStore::install(&ctx, Config::default());
        assert_eq!(store.image_limits().max_image_bytes, 0);
        assert_eq!(store.image_limits().image_byte_limit(), u64::MAX);
        store.validate_image(&input).await.unwrap();
        let limited_ctx = Context::root();
        let limited = LocalAttachmentStore::install(
            &limited_ctx,
            Config {
                max_image_bytes: Some(5 * 1024 * 1024),
                ..Config::default()
            },
        );
        assert_eq!(
            limited.validate_image(&input).await.unwrap_err().code,
            "IMAGE_TOO_LARGE"
        );
    }
}

impl LocalAttachmentStore {
    /// Create the store and register it as the `attachments` service.
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let env = |name: &str| std::env::var(name).ok();
        let home = resolve_dsh_home(config.dsh_home.as_deref(), &env);
        let root = home.join("attachments").join("v1");
        let limits = ImageAttachmentLimits {
            max_image_bytes: config.max_image_bytes.unwrap_or(DEFAULT_MAX_IMAGE_BYTES),
            max_images_per_message: config
                .max_images_per_message
                .unwrap_or(DEFAULT_MAX_IMAGES_PER_MESSAGE),
            max_message_image_bytes: config
                .max_message_image_bytes
                .unwrap_or(DEFAULT_MAX_MESSAGE_IMAGE_BYTES),
            max_image_pixels: config.max_image_pixels.unwrap_or(DEFAULT_MAX_IMAGE_PIXELS),
            media_types: vec![
                ImageMediaType::Png,
                ImageMediaType::Jpeg,
                ImageMediaType::Webp,
                ImageMediaType::Gif,
            ],
        };
        let service = Arc::new(Self { root, limits });
        let erased: Arc<dyn AttachmentStore> = service.clone();
        ctx.register_service(erased);
        service
    }
}

#[async_trait::async_trait]
impl AttachmentStore for LocalAttachmentStore {
    async fn save_file_stream(
        &self,
        reader: dsh_attachment::FileUploadReader<'_>,
        name: String,
        signal: Option<&AttachmentAbort>,
    ) -> Result<dsh_attachment::FileAttachmentRef, AttachmentError> {
        crate::file_store::save(&self.root, reader, &name, signal).await
    }

    async fn open_file(
        &self,
        reference: &dsh_attachment::FileAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<dsh_attachment::FileAttachmentStream, AttachmentError> {
        crate::file_store::open(&self.root, reference, signal).await
    }

    fn file_host_path(&self, reference: &dsh_attachment::FileAttachmentRef) -> Option<PathBuf> {
        crate::file_store::path(&self.root, reference).ok()
    }

    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    async fn validate_image(&self, input: &SaveImageAttachment) -> Result<(), AttachmentError> {
        validate_image_file(input, &self.limits).await
    }

    async fn save_image(
        &self,
        input: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        save_image_file(&self.root, input, &self.limits).await
    }

    async fn save_images(
        &self,
        inputs: &[SaveImageAttachment],
    ) -> Result<Vec<ImageAttachmentRef>, AttachmentError> {
        crate::store::save_images_files(&self.root, inputs, &self.limits).await
    }

    async fn save_image_stream(
        &self,
        reader: dsh_attachment::AttachmentReader,
        media_type: ImageMediaType,
        name: Option<String>,
        signal: Option<&AttachmentAbort>,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        crate::store::save_image_stream(&self.root, reader, media_type, name, &self.limits, signal)
            .await
    }

    async fn open_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<dsh_attachment::ImageAttachmentStream, AttachmentError> {
        crate::store::open_image_file(&self.root, reference, signal).await
    }

    async fn open_image_request(
        &self,
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
        signal: Option<&AttachmentAbort>,
    ) -> Result<dsh_attachment::RequestImageStream, AttachmentError> {
        crate::request_image::open_request_image_file(&self.root, reference, policy, signal).await
    }

    async fn read_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError> {
        read_image_file(&self.root, reference, signal).await
    }

    async fn read_image_request(
        &self,
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
        signal: Option<&AttachmentAbort>,
    ) -> Result<RequestImageAttachment, AttachmentError> {
        crate::request_image::read_request_image_file(&self.root, reference, policy, signal).await
    }
}

/// The Cordis plugin form of the service (the TS class default export).
pub struct LocalAttachmentStorePlugin;

#[async_trait::async_trait]
impl Plugin for LocalAttachmentStorePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("attachment-local")
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        LocalAttachmentStore::install(ctx, config);
        Ok(())
    }
}
