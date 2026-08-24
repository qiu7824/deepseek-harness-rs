use dsh_attachment::{
    ImageAttachmentRef, ImageMediaType, RequestImagePolicy, attachment_id, request_image_variant_id,
};

fn reference() -> ImageAttachmentRef {
    ImageAttachmentRef {
        attachment_id: attachment_id(format!("sha256:{}", "a".repeat(64))),
        media_type: ImageMediaType::Png,
        bytes: 1024,
        width: 32,
        height: 32,
        name: None,
    }
}

#[test]
fn request_variant_identity_is_deterministic_and_policy_complete() {
    let policy = RequestImagePolicy {
        max_pixels: 640_000,
        max_bytes: 1024 * 1024,
        preferred_media_type: ImageMediaType::Webp,
    };
    let first = request_image_variant_id(&reference(), &policy);
    let second = request_image_variant_id(&reference(), &policy);
    assert_eq!(first, second);
    assert!(first.as_str().starts_with("sha256:"));

    let changed = request_image_variant_id(
        &reference(),
        &RequestImagePolicy {
            max_bytes: policy.max_bytes / 2,
            ..policy
        },
    );
    assert_ne!(first, changed);
}
