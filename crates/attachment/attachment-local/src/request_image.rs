use std::io::Cursor;
use std::path::{Path, PathBuf};

use dsh_attachment::{
    AttachmentAbort, AttachmentError, ImageAttachmentRef, ImageMediaType, RequestImageAttachment,
    RequestImagePolicy, request_image_variant_id,
};
use image::imageops::FilterType;

use crate::image::probe_image;
use crate::store::read_image_file;

fn cache_path(root: &Path, variant: &dsh_attachment::ImageVariantId) -> PathBuf {
    let digest = variant
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or("invalid");
    root.join("request-images").join(&digest[..2]).join(digest)
}

fn encode(
    image: &image::DynamicImage,
    media_type: ImageMediaType,
) -> Result<Vec<u8>, AttachmentError> {
    let format = match media_type {
        ImageMediaType::Png => image::ImageFormat::Png,
        ImageMediaType::Jpeg => image::ImageFormat::Jpeg,
        ImageMediaType::Webp => image::ImageFormat::WebP,
        ImageMediaType::Gif => image::ImageFormat::Gif,
    };
    let mut cursor = Cursor::new(Vec::new());
    let result = if media_type == ImageMediaType::Jpeg {
        image::DynamicImage::ImageRgb8(image.to_rgb8()).write_to(&mut cursor, format)
    } else {
        image.write_to(&mut cursor, format)
    };
    result
        .map_err(|error| AttachmentError::new("REQUEST_IMAGE_ENCODE_FAILED", error.to_string()))?;
    Ok(cursor.into_inner())
}

fn aborted(signal: Option<&AttachmentAbort>) -> bool {
    signal.is_some_and(|signal| signal())
}

pub async fn read_request_image_file(
    root: &Path,
    reference: &ImageAttachmentRef,
    policy: &RequestImagePolicy,
    signal: Option<&AttachmentAbort>,
) -> Result<RequestImageAttachment, AttachmentError> {
    if policy.max_pixels == 0 || policy.max_bytes == 0 {
        return Err(AttachmentError::new(
            "INVALID_REQUEST_IMAGE_POLICY",
            "Request image budgets must be positive.",
        ));
    }
    let variant_id = request_image_variant_id(reference, policy);
    let cached = cache_path(root, &variant_id);
    if let Ok(data) = std::fs::read(&cached) {
        let metadata = probe_image(&data)?;
        if metadata.media_type == policy.preferred_media_type
            && metadata.width * metadata.height <= policy.max_pixels
            && data.len() as u64 <= policy.max_bytes
        {
            return Ok(RequestImageAttachment {
                attachment_id: reference.attachment_id.clone(),
                variant_id,
                media_type: metadata.media_type,
                data,
                width: metadata.width,
                height: metadata.height,
            });
        }
    }
    if aborted(signal) {
        return Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "attachment read cancelled",
        ));
    }
    let master = read_image_file(root, reference, signal).await?;
    let mut image = image::load_from_memory(&master.data)
        .map_err(|error| AttachmentError::new("INVALID_IMAGE", error.to_string()))?;
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if pixels > policy.max_pixels {
        let scale = (policy.max_pixels as f64 / pixels as f64).sqrt();
        let width = (f64::from(image.width()) * scale).floor().max(1.0) as u32;
        let height = (f64::from(image.height()) * scale).floor().max(1.0) as u32;
        image = image.resize_exact(width, height, FilterType::Lanczos3);
    }
    let (data, width, height) = loop {
        if aborted(signal) {
            return Err(AttachmentError::new(
                "ATTACHMENT_ABORTED",
                "attachment read cancelled",
            ));
        }
        let data = encode(&image, policy.preferred_media_type)?;
        if data.len() as u64 <= policy.max_bytes {
            break (data, u64::from(image.width()), u64::from(image.height()));
        }
        if image.width() == 1 && image.height() == 1 {
            return Err(AttachmentError::new(
                "REQUEST_IMAGE_TOO_LARGE",
                "Request image cannot satisfy the encoded-byte budget.",
            ));
        }
        image = image.resize_exact(
            (image.width() * 3 / 4).max(1),
            (image.height() * 3 / 4).max(1),
            FilterType::Lanczos3,
        );
    };
    if let Some(parent) = cached.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string()))?;
    }
    dsh_atomic_write::write_file_atomic(
        &cached,
        &data,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|error| AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string()))?;
    Ok(RequestImageAttachment {
        attachment_id: reference.attachment_id.clone(),
        variant_id,
        media_type: policy.preferred_media_type,
        data,
        width,
        height,
    })
}
