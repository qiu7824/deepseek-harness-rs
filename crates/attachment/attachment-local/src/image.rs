//! Raster inspection: full decode at admission, header-only probe on
//! verified reads. Rust port of
//! `packages/attachment/attachment-local/src/image.ts` (the TS `sharp`
//! decoder becomes the `image` crate).

use dsh_attachment::{AttachmentError, ImageMediaType};
use image::ImageFormat;

/// Decoded metadata from a supported image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedImage {
    pub media_type: ImageMediaType,
    pub width: u64,
    pub height: u64,
    pub has_alpha: bool,
}

fn media_type_of(format: ImageFormat) -> Option<ImageMediaType> {
    match format {
        ImageFormat::Png => Some(ImageMediaType::Png),
        ImageFormat::Jpeg => Some(ImageMediaType::Jpeg),
        ImageFormat::WebP => Some(ImageMediaType::Webp),
        ImageFormat::Gif => Some(ImageMediaType::Gif),
        _ => None,
    }
}

/// Parse a supported raster's header and return its intrinsic metadata
/// without decoding pixels (TS `probeImage`).
pub fn probe_image(data: &[u8]) -> Result<DetectedImage, AttachmentError> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| {
            AttachmentError::new("INVALID_IMAGE", "Unsupported or malformed image data.")
        })?;
    let media_type = reader.format().and_then(media_type_of).ok_or_else(|| {
        AttachmentError::new("INVALID_IMAGE", "Unsupported or malformed image data.")
    })?;
    let (width, height) = reader.into_dimensions().map_err(|_| {
        AttachmentError::new("INVALID_IMAGE", "Unsupported or malformed image data.")
    })?;
    Ok(DetectedImage {
        media_type,
        width: u64::from(width),
        height: u64::from(height),
        has_alpha: false,
    })
}

/// Fully decode a supported raster and return its intrinsic metadata (TS
/// `detectImage`). The decoded-pixel admission limit is checked from the
/// header BEFORE the full raster decode, like the TS metadata-first order.
pub fn detect_image(
    data: &[u8],
    max_pixels: Option<u64>,
) -> Result<DetectedImage, AttachmentError> {
    let detected = probe_image(data)?;
    if let Some(max_pixels) = max_pixels
        && detected.width * detected.height > max_pixels
    {
        return Err(AttachmentError::new(
            "IMAGE_TOO_MANY_PIXELS",
            "Image exceeds the configured decoded-pixel limit.",
        ));
    }
    // Full decode: admission must prove these exact bytes decode completely
    // (the concrete pixel type is irrelevant to admission).
    let decoded = image::load_from_memory(data).map_err(|_| {
        AttachmentError::new("INVALID_IMAGE", "Unsupported or malformed image data.")
    })?;
    Ok(DetectedImage {
        has_alpha: decoded.color().has_alpha(),
        ..detected
    })
}

/// WebP encoders may omit an alpha plane when every source sample is opaque.
pub fn encoded_alpha_is_compatible(
    media_type: ImageMediaType,
    normalized_has_alpha: bool,
    normalized_is_fully_opaque: bool,
    encoded_has_alpha: bool,
) -> bool {
    normalized_has_alpha == encoded_has_alpha
        || (media_type == ImageMediaType::Webp
            && normalized_has_alpha
            && normalized_is_fully_opaque
            && !encoded_has_alpha)
}
