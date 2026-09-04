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

fn png_is_animated(data: &[u8]) -> bool {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8_usize;
    loop {
        let Some(header_end) = offset.checked_add(8) else {
            return false;
        };
        let Some(header) = data.get(offset..header_end) else {
            return false;
        };
        let length =
            u32::from_be_bytes(header[..4].try_into().expect("four-byte PNG length")) as usize;
        let kind = &header[4..8];
        let Some(payload_end) = header_end.checked_add(length) else {
            return false;
        };
        let Some(chunk_end) = payload_end.checked_add(4) else {
            return false;
        };
        if chunk_end > data.len() {
            return false;
        }
        if kind == b"acTL" {
            if length < 8 {
                return false;
            }
            let frames = u32::from_be_bytes(
                data[header_end..header_end + 4]
                    .try_into()
                    .expect("four-byte APNG frame count"),
            );
            return frames > 1;
        }
        if kind == b"IEND" {
            return false;
        }
        offset = chunk_end;
    }
}

fn skip_gif_sub_blocks(data: &[u8], offset: &mut usize) -> bool {
    loop {
        let Some(size) = data.get(*offset).copied() else {
            return false;
        };
        *offset += 1;
        if size == 0 {
            return true;
        }
        let Some(next) = offset.checked_add(size as usize) else {
            return false;
        };
        if next > data.len() {
            return false;
        }
        *offset = next;
    }
}

fn gif_is_animated(data: &[u8]) -> bool {
    if data.len() < 13 || !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return false;
    }
    let mut offset = 13_usize;
    let global_packed = data[10];
    if global_packed & 0x80 != 0 {
        let entries = 1_usize << (usize::from(global_packed & 0x07) + 1);
        let Some(next) = offset.checked_add(entries * 3) else {
            return false;
        };
        if next > data.len() {
            return false;
        }
        offset = next;
    }
    let mut frames = 0_u8;
    while let Some(marker) = data.get(offset).copied() {
        match marker {
            0x2c => {
                let Some(descriptor_end) = offset.checked_add(10) else {
                    return false;
                };
                let Some(descriptor) = data.get(offset..descriptor_end) else {
                    return false;
                };
                offset = descriptor_end;
                let local_packed = descriptor[9];
                if local_packed & 0x80 != 0 {
                    let entries = 1_usize << (usize::from(local_packed & 0x07) + 1);
                    let Some(next) = offset.checked_add(entries * 3) else {
                        return false;
                    };
                    if next > data.len() {
                        return false;
                    }
                    offset = next;
                }
                if data.get(offset).is_none() {
                    return false;
                }
                offset += 1; // LZW minimum code size.
                if !skip_gif_sub_blocks(data, &mut offset) {
                    return false;
                }
                frames += 1;
                if frames > 1 {
                    return true;
                }
            }
            0x21 => {
                let Some(extension_data) = offset.checked_add(2) else {
                    return false;
                };
                if extension_data > data.len() {
                    return false;
                }
                offset = extension_data;
                if !skip_gif_sub_blocks(data, &mut offset) {
                    return false;
                }
            }
            0x3b => return false,
            _ => return false,
        }
    }
    false
}

fn webp_is_animated(data: &[u8]) -> bool {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return false;
    }
    let riff_size =
        u32::from_le_bytes(data[4..8].try_into().expect("four-byte RIFF size")) as usize;
    let Some(limit) = 8_usize.checked_add(riff_size) else {
        return false;
    };
    if limit > data.len() || limit < 12 {
        return false;
    }
    let mut offset = 12_usize;
    loop {
        let Some(header_end) = offset.checked_add(8) else {
            return false;
        };
        let Some(header) = data.get(offset..header_end) else {
            return false;
        };
        let kind = &header[..4];
        let size =
            u32::from_le_bytes(header[4..8].try_into().expect("four-byte WebP size")) as usize;
        let Some(payload_end) = header_end.checked_add(size) else {
            return false;
        };
        if payload_end > limit {
            return false;
        }
        if matches!(kind, b"ANIM" | b"ANMF") {
            return true;
        }
        let Some(next) = payload_end.checked_add(size & 1) else {
            return false;
        };
        if next > limit {
            return false;
        }
        offset = next;
        if offset == limit {
            return false;
        }
    }
}

/// Whether the encoded source carries multiple animation frames. Animated
/// sources must never enter the single-frame `DynamicImage` transform path.
fn is_animated(data: &[u8], media_type: ImageMediaType) -> bool {
    match media_type {
        ImageMediaType::Png => png_is_animated(data),
        ImageMediaType::Gif => gif_is_animated(data),
        ImageMediaType::Webp => webp_is_animated(data),
        ImageMediaType::Jpeg => false,
    }
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
    if is_animated(&master.data, master.reference.media_type) {
        if master.reference.width * master.reference.height <= policy.max_pixels
            && master.data.len() as u64 <= policy.max_bytes
        {
            return Ok(RequestImageAttachment {
                attachment_id: reference.attachment_id.clone(),
                variant_id,
                media_type: master.reference.media_type,
                data: master.data,
                width: master.reference.width,
                height: master.reference.height,
            });
        }
        return Err(AttachmentError::new(
            "ANIMATED_REQUEST_IMAGE_TRANSFORM_UNAVAILABLE",
            "Animated images cannot be resized or transcoded without losing frames.",
        ));
    }
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

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use dsh_attachment::{ImageAttachmentLimits, SaveImageAttachment, image_variant_id};
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::store::save_image_file;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dsh-attachment-animated-request-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).expect("create temporary attachment root");
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn assert_animated_bytes_are_preserved(media_type: ImageMediaType, encoded: &str) {
        let temporary = TempRoot::new();
        let root = temporary.0.join("attachments").join("v1");
        let data = STANDARD.decode(encoded).expect("valid fixture base64");
        let reference = save_image_file(
            &root,
            &SaveImageAttachment {
                data: data.clone(),
                media_type,
                name: Some("animated-fixture".to_string()),
            },
            &ImageAttachmentLimits {
                max_image_bytes: 1_000_000,
                max_images_per_message: 4,
                max_message_image_bytes: 1_000_000,
                max_image_pixels: 1_000_000,
                media_types: vec![media_type],
            },
        )
        .await
        .expect("save animated image");
        let policy = RequestImagePolicy {
            max_pixels: reference.width * reference.height,
            max_bytes: data.len() as u64,
            preferred_media_type: media_type,
        };
        let variant_id = request_image_variant_id(&reference, &policy);

        let request = read_request_image_file(&root, &reference, &policy, None)
            .await
            .expect("read animated request image without a lossy transform");

        assert_eq!(request.media_type, media_type);
        assert_eq!(request.data, data);
        assert_eq!(request.width, reference.width);
        assert_eq!(request.height, reference.height);
        assert!(
            !cache_path(&root, &variant_id).exists(),
            "preserved animation must not be cached as a re-encoded static image"
        );
    }

    async fn assert_animated_bytes_override_webp_preference(
        media_type: ImageMediaType,
        encoded: &str,
    ) {
        let temporary = TempRoot::new();
        let root = temporary.0.join("attachments").join("v1");
        let data = STANDARD.decode(encoded).expect("valid fixture base64");
        let reference = save_image_file(
            &root,
            &SaveImageAttachment {
                data: data.clone(),
                media_type,
                name: None,
            },
            &ImageAttachmentLimits {
                max_image_bytes: 1_000_000,
                max_images_per_message: 4,
                max_message_image_bytes: 1_000_000,
                max_image_pixels: 1_000_000,
                media_types: vec![media_type],
            },
        )
        .await
        .unwrap();
        let request = read_request_image_file(
            &root,
            &reference,
            &RequestImagePolicy {
                max_pixels: reference.width * reference.height,
                max_bytes: data.len() as u64,
                preferred_media_type: ImageMediaType::Webp,
            },
            None,
        )
        .await
        .expect("animation-safe request path must override static WebP preference");

        assert_eq!(request.media_type, media_type);
        assert_eq!(request.data, data);
    }

    fn legacy_v1_variant_id(
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
    ) -> dsh_attachment::ImageVariantId {
        let mut hasher = Sha256::new();
        hasher.update(b"dsh-request-image-v1\0");
        hasher.update(reference.attachment_id.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(policy.max_pixels.to_le_bytes());
        hasher.update(policy.max_bytes.to_le_bytes());
        hasher.update(policy.preferred_media_type.as_str().as_bytes());
        image_variant_id(format!("sha256:{:x}", hasher.finalize()))
    }

    #[test]
    fn animated_png_gif_and_webp_are_detected_before_static_decode() {
        let png = STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAACGFjVEwAAAACAAAAAPONk3AAAAAaZmNUTAAAAAAAAAACAAAAAQAAAAAAAAAAAAEACgAA+Sm2eQAAABFJREFUeJxj/M/A8J+BgYEBAA0FAgDgB2sCAAAAGmZjVEwAAAABAAAAAgAAAAEAAAAAAAAAAAABAAoAAGJaXK0AAAAVZmRBVAAAAAJ4nGNkYPj/n4GBgQEACwcCAEgTs00AAAAASUVORK5CYII=")
            .unwrap();
        let gif = STANDARD
            .decode("R0lGODlhAgABAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgABAAAIBQABAAgIACH5BAEKAAEALAAAAAACAAEAgQAA/wAAAAAAAAAAAAgFAAEACAgAOw==")
            .unwrap();
        let webp = STANDARD
            .decode("UklGRoQAAABXRUJQVlA4WAoAAAACAAAAAQAAAAAAQU5JTQYAAAAAAAAAAABBTk1GKAAAAAAAAAAAAAEAAAAAAGQAAAJWUDhMDwAAAC8BAAAABxD9j/4HIqL/AQBBTk1GKAAAAAAAAAAAAAEAAAAAAGQAAABWUDhMDwAAAC8BAAAABxDR//4HIqL/AQA=")
            .unwrap();

        assert!(is_animated(&png, ImageMediaType::Png));
        assert!(is_animated(&gif, ImageMediaType::Gif));
        assert!(is_animated(&webp, ImageMediaType::Webp));

        let mut static_png_with_marker = STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP8z8Dwn4GBgQEADQUCAOAHawIAAAAASUVORK5CYII=")
            .unwrap();
        static_png_with_marker.extend_from_slice(b"acTL");
        let mut static_gif_with_comment_comma = STANDARD
            .decode("R0lGODdhAgABAIEAAP8AAAAAAAAAAAAAACwAAAAAAgABAAAIBQABAAgIADs=")
            .unwrap();
        let image_descriptor = static_gif_with_comment_comma[13..]
            .iter()
            .position(|byte| *byte == 0x2c)
            .unwrap()
            + 13;
        static_gif_with_comment_comma.splice(
            image_descriptor..image_descriptor,
            [0x21, 0xfe, 0x01, 0x2c, 0x00],
        );
        let webp_marker_payload = b"RIFF\x10\x00\x00\x00WEBPXMP \x04\x00\x00\x00ANIM";

        assert!(!is_animated(&static_png_with_marker, ImageMediaType::Png));
        assert!(!is_animated(
            &static_gif_with_comment_comma,
            ImageMediaType::Gif
        ));
        assert!(!is_animated(webp_marker_payload, ImageMediaType::Webp));
    }

    #[tokio::test]
    async fn animated_png_gif_and_webp_keep_original_bytes_and_media_type() {
        assert_animated_bytes_are_preserved(
            ImageMediaType::Png,
            "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAACGFjVEwAAAACAAAAAPONk3AAAAAaZmNUTAAAAAAAAAACAAAAAQAAAAAAAAAAAAEACgAA+Sm2eQAAABFJREFUeJxj/M/A8J+BgYEBAA0FAgDgB2sCAAAAGmZjVEwAAAABAAAAAgAAAAEAAAAAAAAAAAABAAoAAGJaXK0AAAAVZmRBVAAAAAJ4nGNkYPj/n4GBgQEACwcCAEgTs00AAAAASUVORK5CYII=",
        )
        .await;
        assert_animated_bytes_are_preserved(
            ImageMediaType::Gif,
            "R0lGODlhAgABAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgABAAAIBQABAAgIACH5BAEKAAEALAAAAAACAAEAgQAA/wAAAAAAAAAAAAgFAAEACAgAOw==",
        )
        .await;
        assert_animated_bytes_are_preserved(
            ImageMediaType::Webp,
            "UklGRoQAAABXRUJQVlA4WAoAAAACAAAAAQAAAAAAQU5JTQYAAAAAAAAAAABBTk1GKAAAAAAAAAAAAAEAAAAAAGQAAAJWUDhMDwAAAC8BAAAABxD9j/4HIqL/AQBBTk1GKAAAAAAAAAAAAAEAAAAAAGQAAABWUDhMDwAAAC8BAAAABxDR//4HIqL/AQA=",
        )
        .await;
    }

    #[tokio::test]
    async fn animated_png_and_gif_override_static_webp_preference() {
        assert_animated_bytes_override_webp_preference(
            ImageMediaType::Png,
            "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAACGFjVEwAAAACAAAAAPONk3AAAAAaZmNUTAAAAAAAAAACAAAAAQAAAAAAAAAAAAEACgAA+Sm2eQAAABFJREFUeJxj/M/A8J+BgYEBAA0FAgDgB2sCAAAAGmZjVEwAAAABAAAAAgAAAAEAAAAAAAAAAAABAAoAAGJaXK0AAAAVZmRBVAAAAAJ4nGNkYPj/n4GBgQEACwcCAEgTs00AAAAASUVORK5CYII=",
        )
        .await;
        assert_animated_bytes_override_webp_preference(
            ImageMediaType::Gif,
            "R0lGODlhAgABAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgABAAAIBQABAAgIACH5BAEKAAEALAAAAAACAAEAgQAA/wAAAAAAAAAAAAgFAAEACAgAOw==",
        )
        .await;
    }

    #[tokio::test]
    async fn animation_fix_ignores_static_cache_from_legacy_transform_version() {
        let temporary = TempRoot::new();
        let root = temporary.0.join("attachments").join("v1");
        let animated = STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAACGFjVEwAAAACAAAAAPONk3AAAAAaZmNUTAAAAAAAAAACAAAAAQAAAAAAAAAAAAEACgAA+Sm2eQAAABFJREFUeJxj/M/A8J+BgYEBAA0FAgDgB2sCAAAAGmZjVEwAAAABAAAAAgAAAAEAAAAAAAAAAAABAAoAAGJaXK0AAAAVZmRBVAAAAAJ4nGNkYPj/n4GBgQEACwcCAEgTs00AAAAASUVORK5CYII=")
            .unwrap();
        let reference = save_image_file(
            &root,
            &SaveImageAttachment {
                data: animated.clone(),
                media_type: ImageMediaType::Png,
                name: None,
            },
            &ImageAttachmentLimits {
                max_image_bytes: 1_000_000,
                max_images_per_message: 4,
                max_message_image_bytes: 1_000_000,
                max_image_pixels: 1_000_000,
                media_types: vec![ImageMediaType::Png],
            },
        )
        .await
        .unwrap();
        let policy = RequestImagePolicy {
            max_pixels: reference.width * reference.height,
            max_bytes: animated.len() as u64,
            preferred_media_type: ImageMediaType::Png,
        };
        let stale = cache_path(&root, &legacy_v1_variant_id(&reference, &policy));
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(
            stale,
            STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP8z8Dwn4GBgQEADQUCAOAHawIAAAAASUVORK5CYII=")
                .unwrap(),
        )
        .unwrap();

        let request = read_request_image_file(&root, &reference, &policy, None)
            .await
            .unwrap();

        assert_eq!(request.data, animated);
    }
}
