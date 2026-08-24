//! Rust port of the core `attachment` behaviors: raster decoding
//! (image.spec), content-addressed storage (store.spec), and the service
//! boundary (index.spec). The POSIX-only fsync-ORDER assertion is deferred
//! (the implementation keeps the same sync structure; Windows cannot
//! exercise directory fsync, and the Rust port has no fs mock seam yet).

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dsh_attachment::{
    AttachmentError, AttachmentStore, ImageAttachmentLimits, ImageMediaType, RequestImagePolicy,
    SaveImageAttachment, attachment_id,
};
use dsh_attachment_local::{
    Config, DEFAULT_MAX_IMAGE_BYTES, DEFAULT_MAX_IMAGE_PIXELS, DEFAULT_MAX_IMAGES_PER_MESSAGE,
    DEFAULT_MAX_MESSAGE_IMAGE_BYTES, LocalAttachmentStore, detect_image,
    encoded_alpha_is_compatible, probe_image, read_image_file, save_image_file,
};
use image::{Rgba, RgbaImage};

fn png_1x1() -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .expect("fixture")
}

#[test]
fn webp_may_omit_only_an_all_opaque_alpha_plane() {
    assert!(encoded_alpha_is_compatible(
        ImageMediaType::Webp,
        true,
        true,
        false
    ));
    assert!(!encoded_alpha_is_compatible(
        ImageMediaType::Webp,
        true,
        false,
        false
    ));
    assert!(!encoded_alpha_is_compatible(
        ImageMediaType::Png,
        true,
        true,
        false
    ));
    assert!(encoded_alpha_is_compatible(
        ImageMediaType::Webp,
        false,
        true,
        false
    ));
}

fn raster(format: image::ImageFormat) -> Vec<u8> {
    let rgba = RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255]));
    let mut cursor = Cursor::new(Vec::new());
    if format == image::ImageFormat::Jpeg {
        image::DynamicImage::ImageRgba8(rgba)
            .to_rgb8()
            .write_to(&mut cursor, format)
            .expect("encode");
    } else {
        rgba.write_to(&mut cursor, format).expect("encode");
    }
    cursor.into_inner()
}

fn limits() -> ImageAttachmentLimits {
    ImageAttachmentLimits {
        max_image_bytes: 1024,
        max_images_per_message: 2,
        max_message_image_bytes: 2048,
        max_image_pixels: 16,
        media_types: vec![
            ImageMediaType::Png,
            ImageMediaType::Jpeg,
            ImageMediaType::Webp,
            ImageMediaType::Gif,
        ],
    }
}

fn temp_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsh-attachment-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp home");
    dir
}

fn input(data: Vec<u8>, media_type: ImageMediaType) -> SaveImageAttachment {
    SaveImageAttachment {
        data,
        media_type,
        name: None,
    }
}

fn code(error: &AttachmentError) -> &str {
    &error.code
}

#[test]
fn decodes_every_supported_format_with_intrinsic_dimensions() {
    for (format, media_type) in [
        (image::ImageFormat::Png, ImageMediaType::Png),
        (image::ImageFormat::Jpeg, ImageMediaType::Jpeg),
        (image::ImageFormat::WebP, ImageMediaType::Webp),
        (image::ImageFormat::Gif, ImageMediaType::Gif),
    ] {
        let detected = detect_image(&raster(format), None).expect("detect");
        assert_eq!(detected.media_type, media_type);
        assert_eq!((detected.width, detected.height), (3, 2));
    }
}

#[test]
fn rejects_excess_pixels_before_decoding_and_malformed_bytes() {
    let outcome = detect_image(&raster(image::ImageFormat::Png), Some(5));
    assert_eq!(
        code(&outcome.expect_err("pixel limit")),
        "IMAGE_TOO_MANY_PIXELS"
    );
    let outcome = detect_image(&[1, 2, 3], None);
    assert_eq!(code(&outcome.expect_err("malformed")), "INVALID_IMAGE");
    let tiff = raster_tiff();
    let outcome = detect_image(&tiff, None);
    assert_eq!(code(&outcome.expect_err("unsupported")), "INVALID_IMAGE");
    // A readable header with a truncated payload: probe succeeds, full decode
    // fails.
    let complete = raster(image::ImageFormat::Png);
    let truncated = &complete[..62];
    assert_eq!(
        probe_image(truncated).expect("header").media_type,
        ImageMediaType::Png
    );
    let outcome = detect_image(truncated, None);
    assert_eq!(code(&outcome.expect_err("truncated")), "INVALID_IMAGE");
    let outcome = probe_image(&[1, 2, 3]);
    assert_eq!(
        code(&outcome.expect_err("probe malformed")),
        "INVALID_IMAGE"
    );
    let outcome = probe_image(&tiff);
    assert_eq!(
        code(&outcome.expect_err("probe unsupported")),
        "INVALID_IMAGE"
    );
}

fn raster_tiff() -> Vec<u8> {
    let image = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
    let mut cursor = Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Tiff)
        .expect("encode tiff");
    cursor.into_inner()
}

#[tokio::test(flavor = "current_thread")]
async fn publishes_dedupes_and_reads_content_addressed_objects() {
    let home = temp_home();
    let root = home.join("attachments").join("v1");
    let png = png_1x1();
    let first = save_image_file(
        &root,
        &SaveImageAttachment {
            data: png.clone(),
            media_type: ImageMediaType::Png,
            name: Some("/private/tmp/pixel.png".to_string()),
        },
        &limits(),
    )
    .await
    .expect("first");
    let second = save_image_file(&root, &input(png.clone(), ImageMediaType::Png), &limits())
        .await
        .expect("second");
    let sha256 = {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&png);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        first.attachment_id,
        attachment_id(format!("sha256:{sha256}"))
    );
    assert_eq!(first.media_type, ImageMediaType::Png);
    assert_eq!(first.bytes, png.len() as u64);
    assert_eq!((first.width, first.height), (1, 1));
    assert_eq!(first.name.as_deref(), Some("pixel.png"));
    assert_eq!(second.attachment_id, first.attachment_id);
    assert_eq!(
        std::fs::read(root.join("objects").join(&sha256[..2]).join(&sha256)).expect("object"),
        png
    );
    let read = read_image_file(&root, &first, None).await.expect("read");
    assert_eq!(read.data, png);
    assert_eq!(read.reference, first);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test(flavor = "current_thread")]
async fn creates_a_missing_nested_home() {
    let base = temp_home();
    let root = base.join("home").join("attachments").join("v1");
    let png = png_1x1();
    let reference = save_image_file(&root, &input(png.clone(), ImageMediaType::Png), &limits())
        .await
        .expect("save");
    let read = read_image_file(&root, &reference, None)
        .await
        .expect("read");
    assert_eq!(read.data, png);
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_malformed_mismatched_and_oversized_inputs() {
    let home = temp_home();
    let root = home.join("attachments").join("v1");
    let png = png_1x1();
    let cases: Vec<(&str, SaveImageAttachment, ImageAttachmentLimits, &str)> = vec![
        (
            "empty",
            input(Vec::new(), ImageMediaType::Png),
            limits(),
            "INVALID_IMAGE",
        ),
        (
            "malformed",
            input(vec![1, 2, 3], ImageMediaType::Png),
            limits(),
            "INVALID_IMAGE",
        ),
        (
            "mismatch",
            input(png.clone(), ImageMediaType::Jpeg),
            limits(),
            "IMAGE_TYPE_MISMATCH",
        ),
        (
            "too large",
            input(png.clone(), ImageMediaType::Png),
            ImageAttachmentLimits {
                max_image_bytes: 1,
                ..limits()
            },
            "IMAGE_TOO_LARGE",
        ),
    ];
    for (label, input, limits, expected) in cases {
        let outcome = save_image_file(&root, &input, &limits).await;
        assert_eq!(code(&outcome.expect_err(label)), expected, "{label}");
    }
    // Decoded-pixel limit: a 5x5 raster exceeds 16 pixels.
    let wide = raster_at(5, 5);
    let outcome = save_image_file(&root, &input(wide, ImageMediaType::Png), &limits()).await;
    assert_eq!(code(&outcome.expect_err("pixels")), "IMAGE_TOO_MANY_PIXELS");
    // A control-char name strips to undefined.
    let unnamed = save_image_file(
        &root,
        &SaveImageAttachment {
            data: png.clone(),
            media_type: ImageMediaType::Png,
            name: Some("\u{0}".to_string()),
        },
        &limits(),
    )
    .await
    .expect("unnamed");
    assert_eq!(unnamed.name, None);
    let _ = std::fs::remove_dir_all(&home);
}

fn raster_at(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255]));
    let mut cursor = Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode");
    cursor.into_inner()
}

#[tokio::test(flavor = "current_thread")]
async fn fails_closed_on_missing_corrupted_and_invalid_references() {
    let home = temp_home();
    let root = home.join("attachments").join("v1");
    let png = png_1x1();
    let reference = save_image_file(&root, &input(png.clone(), ImageMediaType::Png), &limits())
        .await
        .expect("save");
    let sha256 = reference
        .attachment_id
        .as_str()
        .strip_prefix("sha256:")
        .expect("prefix")
        .to_string();
    let object = root.join("objects").join(&sha256[..2]).join(&sha256);

    std::fs::write(&object, vec![1, 2, 3]).expect("corrupt");
    let outcome = read_image_file(&root, &reference, None).await;
    assert_eq!(code(&outcome.expect_err("corrupt")), "ATTACHMENT_CORRUPT");

    let mut invalid = reference.clone();
    invalid.attachment_id = attachment_id("bad");
    let outcome = read_image_file(&root, &invalid, None).await;
    assert_eq!(
        code(&outcome.expect_err("invalid ref")),
        "INVALID_ATTACHMENT_REF"
    );

    let missing_home = temp_home();
    let missing_root = missing_home.join("attachments").join("v1");
    std::fs::create_dir_all(&missing_root).expect("missing root");
    let outcome = read_image_file(&missing_root, &reference, None).await;
    assert_eq!(code(&outcome.expect_err("missing")), "ATTACHMENT_NOT_FOUND");

    // A directory where the object file belongs reads as a storage failure.
    let unreadable_home = temp_home();
    let unreadable_root = unreadable_home.join("attachments").join("v1");
    let target = unreadable_root
        .join("objects")
        .join(&sha256[..2])
        .join(&sha256);
    std::fs::create_dir_all(&target).expect("directory target");
    let outcome = read_image_file(&unreadable_root, &reference, None).await;
    assert_eq!(
        code(&outcome.expect_err("unreadable")),
        "ATTACHMENT_READ_FAILED"
    );

    // A conflicting pre-existing object is rejected at publish time.
    let conflicting_home = temp_home();
    let conflicting_root = conflicting_home.join("attachments").join("v1");
    let conflicting_object = conflicting_root
        .join("objects")
        .join(&sha256[..2])
        .join(&sha256);
    std::fs::create_dir_all(conflicting_object.parent().expect("bucket")).expect("bucket");
    std::fs::write(&conflicting_object, vec![1, 2, 3]).expect("conflicting bytes");
    let outcome = save_image_file(
        &conflicting_root,
        &input(png.clone(), ImageMediaType::Png),
        &limits(),
    )
    .await;
    assert_eq!(code(&outcome.expect_err("conflict")), "ATTACHMENT_CORRUPT");

    // Reference metadata mismatch after bytes verified.
    std::fs::write(&object, &png).expect("restore");
    let mut shifted = reference.clone();
    shifted.width += 1;
    let outcome = read_image_file(&root, &shifted, None).await;
    assert_eq!(code(&outcome.expect_err("metadata")), "ATTACHMENT_CORRUPT");

    // An unexpected publication failure maps to the stable write error.
    let blocked_home = temp_home();
    let blocked_root = blocked_home.join("attachments").join("v1");
    let blocked_target = blocked_root
        .join("objects")
        .join(&sha256[..2])
        .join(&sha256);
    std::fs::create_dir_all(&blocked_target).expect("directory target");
    let outcome = save_image_file(&blocked_root, &input(png, ImageMediaType::Png), &limits()).await;
    assert_eq!(
        code(&outcome.expect_err("blocked")),
        "ATTACHMENT_WRITE_FAILED"
    );

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&missing_home);
    let _ = std::fs::remove_dir_all(&unreadable_home);
    let _ = std::fs::remove_dir_all(&conflicting_home);
    let _ = std::fs::remove_dir_all(&blocked_home);
}

#[tokio::test(flavor = "current_thread")]
async fn aborted_reads_surface_a_stable_cancellation_error() {
    let home = temp_home();
    let root = home.join("attachments").join("v1");
    let png = png_1x1();
    let reference = save_image_file(&root, &input(png.clone(), ImageMediaType::Png), &limits())
        .await
        .expect("save");
    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_signal = flag.clone();
    let signal: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || flag_for_signal.load(Ordering::SeqCst));
    let read = read_image_file(&root, &reference, Some(&signal)).await;
    assert!(read.is_ok(), "unaborted read resolves");
    flag.store(true, Ordering::SeqCst);
    let outcome = read_image_file(&root, &reference, Some(&signal)).await;
    assert_eq!(code(&outcome.expect_err("aborted")), "ATTACHMENT_ABORTED");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test(flavor = "current_thread")]
async fn service_boundary_resolves_defaults_and_validates_without_persisting() {
    assert_eq!(DEFAULT_MAX_IMAGE_BYTES, 5 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_IMAGES_PER_MESSAGE, 20);
    assert_eq!(DEFAULT_MAX_MESSAGE_IMAGE_BYTES, 100 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_IMAGE_PIXELS, 40_000_000);

    let ctx = cordis::Context::root();
    let home = temp_home();
    let service = LocalAttachmentStore::install(
        &ctx,
        Config {
            dsh_home: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    let limits = service.image_limits();
    assert_eq!(limits.max_image_bytes, DEFAULT_MAX_IMAGE_BYTES);
    assert_eq!(
        limits.max_images_per_message,
        DEFAULT_MAX_IMAGES_PER_MESSAGE
    );
    assert_eq!(
        limits.max_message_image_bytes,
        DEFAULT_MAX_MESSAGE_IMAGE_BYTES
    );
    assert_eq!(limits.max_image_pixels, DEFAULT_MAX_IMAGE_PIXELS);
    assert_eq!(
        limits.media_types,
        vec![
            ImageMediaType::Png,
            ImageMediaType::Jpeg,
            ImageMediaType::Webp,
            ImageMediaType::Gif
        ]
    );
    assert!(
        service
            .root
            .ends_with(["attachments", "v1"].iter().collect::<PathBuf>())
    );

    let png = png_1x1();
    let reference = service
        .save_image(&SaveImageAttachment {
            data: png.clone(),
            media_type: ImageMediaType::Png,
            name: None,
        })
        .await
        .expect("save");
    let read = service.read_image(&reference, None).await.expect("read");
    assert_eq!(read.data, png);
    assert_eq!(read.reference, reference);

    // Validation never touches storage: a rejecting validate leaves no root.
    let clean_home = temp_home();
    let clean_ctx = cordis::Context::root();
    let clean = LocalAttachmentStore::install(
        &clean_ctx,
        Config {
            dsh_home: Some(clean_home.to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    let outcome = clean
        .validate_image(&input(vec![1, 2, 3], ImageMediaType::Png))
        .await;
    assert_eq!(code(&outcome.expect_err("validate")), "INVALID_IMAGE");
    let limited_ctx = cordis::Context::root();
    let limited = LocalAttachmentStore::install(
        &limited_ctx,
        Config {
            dsh_home: Some(clean_home.to_string_lossy().to_string()),
            max_image_bytes: Some(1),
            ..Default::default()
        },
    );
    let outcome = limited
        .validate_image(&input(png.clone(), ImageMediaType::Png))
        .await;
    assert_eq!(
        code(&outcome.expect_err("validate limited")),
        "IMAGE_TOO_LARGE"
    );
    clean
        .validate_image(&input(png, ImageMediaType::Png))
        .await
        .expect("valid passes");
    assert!(!clean.root.exists(), "validation never persists");

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&clean_home);
}

#[tokio::test(flavor = "current_thread")]
async fn batch_save_validates_every_image_before_publishing_any_object() {
    let home = temp_home();
    let ctx = cordis::Context::root();
    let service = LocalAttachmentStore::install(
        &ctx,
        Config {
            dsh_home: Some(home.to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    let valid = input(png_1x1(), ImageMediaType::Png);
    let invalid = input(vec![1, 2, 3], ImageMediaType::Png);

    let outcome = service.save_images(&[valid.clone(), invalid]).await;
    assert_eq!(code(&outcome.expect_err("invalid batch")), "INVALID_IMAGE");
    assert!(
        !service.root.exists(),
        "a rejected batch must not publish an earlier valid image"
    );

    let references = service
        .save_images(&[valid.clone(), valid])
        .await
        .expect("valid batch");
    assert_eq!(references.len(), 2);
    assert_eq!(references[0].attachment_id, references[1].attachment_id);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test(flavor = "current_thread")]
async fn request_image_is_deterministic_and_respects_route_budgets() {
    let home = temp_home();
    let ctx = cordis::Context::root();
    let service = LocalAttachmentStore::install(
        &ctx,
        Config {
            dsh_home: Some(home.to_string_lossy().to_string()),
            max_image_bytes: Some(1024 * 1024),
            max_image_pixels: Some(1_000_000),
            ..Default::default()
        },
    );
    let reference = service
        .save_image(&input(raster_at(100, 100), ImageMediaType::Png))
        .await
        .expect("master");
    let policy = RequestImagePolicy {
        max_pixels: 400,
        max_bytes: 128 * 1024,
        preferred_media_type: ImageMediaType::Webp,
    };
    let first = service
        .read_image_request(&reference, &policy, None)
        .await
        .expect("request version");
    let second = service
        .read_image_request(&reference, &policy, None)
        .await
        .expect("cached request version");
    assert!(first.width * first.height <= policy.max_pixels);
    assert!(first.data.len() as u64 <= policy.max_bytes);
    assert_eq!(first.variant_id, second.variant_id);
    assert_eq!(first.data, second.data);
    let _ = std::fs::remove_dir_all(&home);
}
