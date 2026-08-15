//! Content-addressed, owner-private local attachment storage. Rust port of
//! `packages/attachment/attachment-local/src/store.ts`.
//!
//! # Deviations
//!
//! - Directory fsync is a no-op on Windows (NTFS metadata journaling owns
//!   entry durability), exactly like the TS `win32` early return.
//! - The abort seam is a predicate without a reason payload, so an aborted
//!   read surfaces as `ATTACHMENT_ABORTED` ("attachment read cancelled")
//!   instead of the caller's own error object.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dsh_attachment::{
    AttachmentAbort, AttachmentError, ImageAttachmentLimits, ImageAttachmentRef,
    ImageMediaType, SaveImageAttachment, StoredImageAttachment, attachment_id,
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::image::{detect_image, probe_image};

const ID_PATTERN: &str = r"^sha256:([a-f0-9]{64})$";

static DURABLE_HOMES: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> =
    std::sync::OnceLock::new();

fn durable_homes() -> &'static Mutex<HashSet<PathBuf>> {
    DURABLE_HOMES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Strip both separator styles by hand (TS `displayName`).
fn display_name(value: Option<&str>) -> Option<String> {
    let value = value?;
    let slash = value.rfind('/');
    let backslash = value.rfind('\\');
    let start = match (slash, backslash) {
        (Some(s), Some(b)) => s.max(b) + 1,
        (Some(s), None) => s + 1,
        (None, Some(b)) => b + 1,
        (None, None) => 0,
    };
    let leaf = &value[start..];
    let clean: String = leaf
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    let truncated: String = clean.chars().take(255).collect();
    if truncated.is_empty() {
        None
    } else {
        Some(truncated)
    }
}

fn object_path(root: &Path, sha256: &str) -> PathBuf {
    root.join("objects").join(&sha256[..2]).join(sha256)
}

fn ensure_reference(reference: &ImageAttachmentRef) -> Result<String, AttachmentError> {
    let pattern = regex::Regex::new(ID_PATTERN).expect("static pattern");
    let value = reference.attachment_id.as_str();
    pattern
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str().to_string())
        .ok_or_else(|| {
            AttachmentError::new("INVALID_ATTACHMENT_REF", "Attachment reference is invalid.")
        })
}

async fn inspect_metadata(
    data: &[u8],
    declared_media_type: ImageMediaType,
    max_pixels: Option<u64>,
) -> Result<(ImageMediaType, u64, u64, u64), AttachmentError> {
    if data.is_empty() {
        return Err(AttachmentError::new("INVALID_IMAGE", "Image is empty."));
    }
    let detected = detect_image(data, max_pixels)?;
    if detected.media_type != declared_media_type {
        return Err(AttachmentError::new(
            "IMAGE_TYPE_MISMATCH",
            "Declared image type does not match its bytes.",
        ));
    }
    Ok((detected.media_type, data.len() as u64, detected.width, detected.height))
}

/// Run the full admission policy for one image without touching storage (TS
/// `validateImageFile`).
pub async fn validate_image_file(
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
) -> Result<(), AttachmentError> {
    if input.data.len() as u64 > limits.max_image_bytes {
        return Err(AttachmentError::new(
            "IMAGE_TOO_LARGE",
            "Image exceeds the configured byte limit.",
        ));
    }
    inspect_metadata(&input.data, input.media_type, Some(limits.max_image_pixels)).await?;
    Ok(())
}

/// Make a directory's entries durable (fsync on a read-only directory
/// handle); a no-op on Windows (TS `syncDirectory`).
fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let handle = std::fs::File::open(path)?;
        handle.sync_all()
    }
}

/// Create one private directory tree and persist every ancestor entry up to
/// a caller-vouched durable boundary (TS `ensureDurableDirectory`).
fn ensure_durable_directory(path: &Path, boundary: &Path) -> Result<(), AttachmentError> {
    std::fs::create_dir_all(path)
        .map_err(|error| AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).ok();
    }
    let mut level = path.to_path_buf();
    while level != boundary {
        let parent = level.parent().map(Path::to_path_buf).unwrap_or_else(|| level.clone());
        let _ = sync_directory(&parent);
        if parent == level {
            return Ok(());
        }
        level = parent;
    }
    Ok(())
}

/// Establish this process's proof that one DSH_HOME entry and every ancestor
/// below the filesystem root are durable (TS `ensureDurableHome`).
fn ensure_durable_home(path: &Path) -> Result<PathBuf, AttachmentError> {
    let home = path.to_path_buf();
    if durable_homes().lock().contains(&home) {
        return Ok(home);
    }
    let mut root = home.clone();
    while let Some(parent) = root.parent() {
        if parent == root {
            break;
        }
        root = parent.to_path_buf();
    }
    ensure_durable_directory(&home, &root)?;
    durable_homes().lock().insert(home.clone());
    Ok(home)
}

/// Save and verify immutable image bytes below a versioned attachment root
/// (TS `saveImageFile`).
pub async fn save_image_file(
    root: &Path,
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
) -> Result<ImageAttachmentRef, AttachmentError> {
    if input.data.len() as u64 > limits.max_image_bytes {
        return Err(AttachmentError::new(
            "IMAGE_TOO_LARGE",
            "Image exceeds the configured byte limit.",
        ));
    }
    let (media_type, bytes, width, height) =
        inspect_metadata(&input.data, input.media_type, Some(limits.max_image_pixels)).await?;
    let sha256 = digest(&input.data);
    let bucket = root.join("objects").join(&sha256[..2]);
    let staging = root.join("tmp");
    // Establish DSH_HOME itself against the filesystem root once per process.
    let home = root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| AttachmentError::new("ATTACHMENT_WRITE_FAILED", "invalid attachment root"))?;
    let boundary = ensure_durable_home(home)?;
    ensure_durable_directory(&bucket, &boundary)?;
    ensure_durable_directory(&staging, &boundary)?;
    let temporary = staging.join(uuid::Uuid::new_v4().to_string());
    let target = object_path(root, &sha256);

    let persist = (|| -> Result<(), AttachmentError> {
        let mut handle = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string()))?;
        handle.write_all(&input.data).map_err(|error| {
            AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string())
        })?;
        handle
            .sync_all()
            .map_err(|error| AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string()))?;
        drop(handle);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).ok();
        }
        match std::fs::hard_link(&temporary, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // The dedup path: another writer published first. Verify the
                // existing object bytes.
                let existing = std::fs::read(&target).map_err(|error| {
                    AttachmentError::new("ATTACHMENT_WRITE_FAILED", error.to_string())
                })?;
                if digest(&existing) != sha256 {
                    return Err(AttachmentError::new(
                        "ATTACHMENT_CORRUPT",
                        "Stored attachment failed integrity verification.",
                    ));
                }
            }
            Err(error) => {
                return Err(AttachmentError::new(
                    "ATTACHMENT_WRITE_FAILED",
                    error.to_string(),
                ));
            }
        }
        // Persist the target entry and close a concurrent bucket-creation
        // window before the reference can reach a session checkpoint.
        let _ = sync_directory(&bucket);
        let _ = sync_directory(&root.join("objects"));
        Ok(())
    })();

    match persist {
        Ok(()) => {
            let _ = std::fs::remove_file(&temporary);
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    }
    let name = display_name(input.name.as_deref());
    let mut reference = ImageAttachmentRef {
        attachment_id: attachment_id(format!("sha256:{sha256}")),
        media_type,
        bytes,
        width,
        height,
        name: None,
    };
    if let Some(name) = name {
        reference.name = Some(name);
    }
    Ok(reference)
}

/// Read and verify one content-addressed image (TS `readImageFile`).
pub async fn read_image_file(
    root: &Path,
    reference: &ImageAttachmentRef,
    signal: Option<&AttachmentAbort>,
) -> Result<StoredImageAttachment, AttachmentError> {
    let aborted = |signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>| {
        signal.is_some_and(|signal| signal())
    };
    if aborted(signal) {
        return Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "attachment read cancelled",
        ));
    }
    let sha256 = ensure_reference(reference)?;
    let data = match std::fs::read(object_path(root, &sha256)) {
        Ok(data) => data,
        Err(error) => {
            if aborted(signal) {
                return Err(AttachmentError::new(
                    "ATTACHMENT_ABORTED",
                    "attachment read cancelled",
                ));
            }
            if error.kind() == std::io::ErrorKind::NotFound {
                return Err(AttachmentError::new(
                    "ATTACHMENT_NOT_FOUND",
                    "Attachment object is missing.",
                ));
            }
            return Err(AttachmentError::new(
                "ATTACHMENT_READ_FAILED",
                "Unable to read image attachment.",
            ));
        }
    };
    if aborted(signal) {
        return Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "attachment read cancelled",
        ));
    }
    if digest(&data) != sha256 {
        return Err(AttachmentError::new(
            "ATTACHMENT_CORRUPT",
            "Stored attachment failed integrity verification.",
        ));
    }
    // The digest proves these are the exact bytes admission fully decoded,
    // so the read path only re-derives the header fields.
    let metadata = probe_image(&data)?;
    if aborted(signal) {
        return Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "attachment read cancelled",
        ));
    }
    if metadata.media_type != reference.media_type
        || data.len() as u64 != reference.bytes
        || metadata.width != reference.width
        || metadata.height != reference.height
    {
        return Err(AttachmentError::new(
            "ATTACHMENT_CORRUPT",
            "Stored attachment metadata does not match its reference.",
        ));
    }
    Ok(StoredImageAttachment {
        reference: reference.clone(),
        data,
    })
}
