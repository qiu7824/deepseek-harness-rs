//! Immutable attachment files, bounded verification and worker admission.
use crate::codec::{CodecJob, ImageInfo, check_cancel, check_path, error, io_error, read_handle};
use dsh_attachment::{
    AttachmentAbort, AttachmentError, AttachmentReader, ImageAttachmentLimits, ImageAttachmentRef,
    ImageAttachmentStream, ImageMediaType, SaveImageAttachment, StoredImageAttachment,
    attachment_id,
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
const ID_PATTERN: &str = r"^sha256:([a-f0-9]{64})$";
static DURABLE_HOMES: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
fn durable_homes() -> &'static Mutex<HashSet<PathBuf>> {
    DURABLE_HOMES.get_or_init(|| Mutex::new(HashSet::new()))
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
        let parent = level
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| level.clone());
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

async fn stage_bytes(
    root: &Path,
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
) -> Result<(CodecJob, ImageInfo), AttachmentError> {
    let mut reader = input.data.as_slice();
    CodecJob::stage(
        root,
        &mut reader,
        input.media_type,
        limits.image_byte_limit(),
        limits.max_image_pixels,
        None,
    )
    .await?
    .run(None, None)
    .await
}
pub(crate) fn temporary_codec_root(base: &Path) -> Result<PathBuf, AttachmentError> {
    // Resolve only the OS-supplied temporary base (macOS /var is an alias).
    // Owned codec descendants still pass the no-links storage checks.
    std::fs::canonicalize(base)
        .map(|base| base.join("dsh-image-codec-v1"))
        .map_err(io_error)
}

pub async fn validate_image_file(
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
) -> Result<(), AttachmentError> {
    let root = temporary_codec_root(&std::env::temp_dir())?;
    stage_bytes(&root, input, limits).await?;
    Ok(())
}
pub async fn save_image_file(
    root: &Path,
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
) -> Result<ImageAttachmentRef, AttachmentError> {
    let (job, info) = stage_bytes(root, input, limits).await?;
    publish_image(root, &job.input(), &info, input.name.as_deref(), None).await
}
pub async fn save_image_stream(
    root: &Path,
    mut reader: AttachmentReader,
    media_type: ImageMediaType,
    name: Option<String>,
    limits: &ImageAttachmentLimits,
    signal: Option<&AttachmentAbort>,
) -> Result<ImageAttachmentRef, AttachmentError> {
    let (job, info) = CodecJob::stage(
        root,
        &mut reader,
        media_type,
        limits.image_byte_limit(),
        limits.max_image_pixels,
        signal,
    )
    .await?
    .run(None, signal)
    .await?;
    publish_image(root, &job.input(), &info, name.as_deref(), signal).await
}
pub async fn save_images_files(
    root: &Path,
    inputs: &[SaveImageAttachment],
    limits: &ImageAttachmentLimits,
) -> Result<Vec<ImageAttachmentRef>, AttachmentError> {
    // No duplicate decode and no reference publication until every input passes.
    let mut validated = Vec::with_capacity(inputs.len());
    for input in inputs {
        validated.push(stage_bytes(root, input, limits).await?);
    }
    let mut references = Vec::with_capacity(inputs.len());
    for (input, (job, info)) in inputs.iter().zip(validated.iter()) {
        references
            .push(publish_image(root, &job.input(), info, input.name.as_deref(), None).await?);
    }
    Ok(references)
}

async fn verify_hash(
    handle: std::fs::File,
    sha256: &str,
    bytes: u64,
    signal: Option<&AttachmentAbort>,
) -> Result<std::fs::File, AttachmentError> {
    let mut input = tokio::fs::File::from_std(handle);
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        check_cancel(signal)?;
        let count = input.read(&mut buffer).await.map_err(io_error)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > bytes {
            return Err(error(
                "ATTACHMENT_CORRUPT",
                "Stored attachment length changed.",
            ));
        }
        digest.update(&buffer[..count]);
    }
    check_cancel(signal)?;
    if size != bytes || format!("{:x}", digest.finalize()) != sha256 {
        return Err(error(
            "ATTACHMENT_CORRUPT",
            "Stored attachment failed integrity verification.",
        ));
    }
    input.seek(SeekFrom::Start(0)).await.map_err(io_error)?;
    Ok(input.into_std().await)
}
fn probe_handle(handle: &mut std::fs::File) -> Result<crate::DetectedImage, AttachmentError> {
    let result = crate::image::probe_reader(std::io::BufReader::new(&mut *handle));
    handle.seek(SeekFrom::Start(0)).map_err(io_error)?;
    result
}
async fn publish_object(
    source: &Path,
    target: &Path,
    info: &ImageInfo,
    signal: Option<&AttachmentAbort>,
) -> Result<(), AttachmentError> {
    check_cancel(signal)?;
    check_path(source)?;
    check_path(target)?;
    std::fs::create_dir_all(target.parent().expect("object bucket")).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(source, std::fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
    }
    // Verify before linking and keep the immutable handle through publication.
    let _lease = verify_hash(read_handle(source)?, &info.sha256, info.bytes, signal).await?;
    check_cancel(signal)?;
    match std::fs::hard_link(source, target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_hash(read_handle(target)?, &info.sha256, info.bytes, signal).await?;
        }
        Err(e) => return Err(io_error(e)),
    }
    sync_directory(target.parent().expect("object bucket")).map_err(io_error)?;
    Ok(())
}
async fn publish_image(
    root: &Path,
    source: &Path,
    info: &ImageInfo,
    name: Option<&str>,
    signal: Option<&AttachmentAbort>,
) -> Result<ImageAttachmentRef, AttachmentError> {
    let home = root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| error("ATTACHMENT_WRITE_FAILED", "Invalid attachment root."))?;
    let boundary = ensure_durable_home(home)?;
    let target = object_path(root, &info.sha256);
    ensure_durable_directory(target.parent().expect("object bucket"), &boundary)?;
    publish_object(source, &target, info, signal).await?;
    check_cancel(signal)?;
    Ok(ImageAttachmentRef {
        attachment_id: attachment_id(format!("sha256:{}", info.sha256)),
        media_type: info.media_type,
        bytes: info.bytes,
        width: info.width,
        height: info.height,
        name: display_name(name),
    })
}
pub async fn open_image_file(
    root: &Path,
    reference: &ImageAttachmentRef,
    signal: Option<&AttachmentAbort>,
) -> Result<ImageAttachmentStream, AttachmentError> {
    check_cancel(signal)?;
    let sha256 = ensure_reference(reference)?;
    let path = object_path(root, &sha256);
    let file = read_handle(&path).map_err(|e| {
        if !path.exists() {
            error("ATTACHMENT_NOT_FOUND", "Image attachment is missing.")
        } else {
            e
        }
    })?;
    let mut file = verify_hash(file, &sha256, reference.bytes, signal).await?;
    let metadata = probe_handle(&mut file)?;
    if metadata.media_type != reference.media_type
        || metadata.width != reference.width
        || metadata.height != reference.height
    {
        return Err(error(
            "ATTACHMENT_CORRUPT",
            "Stored attachment metadata does not match its reference.",
        ));
    }
    Ok(ImageAttachmentStream {
        reference: reference.clone(),
        reader: Box::pin(tokio::fs::File::from_std(file)),
    })
}
pub async fn read_image_file(
    root: &Path,
    reference: &ImageAttachmentRef,
    signal: Option<&AttachmentAbort>,
) -> Result<StoredImageAttachment, AttachmentError> {
    let mut stream = open_image_file(root, reference, signal).await?;
    let mut data = Vec::new();
    stream
        .reader
        .read_to_end(&mut data)
        .await
        .map_err(io_error)?;
    check_cancel(signal)?;
    Ok(StoredImageAttachment {
        reference: stream.reference,
        data,
    })
}
pub(crate) async fn publish_variant(
    source: &Path,
    cached: &Path,
    info: &ImageInfo,
    signal: Option<&AttachmentAbort>,
) -> Result<(), AttachmentError> {
    // Content is immutable; a short manifest is the sole atomic commit point.
    let object = cached.with_file_name(format!("{}.blob", info.sha256));
    publish_object(source, &object, info, signal).await?;
    check_cancel(signal)?;
    let bytes = serde_json::to_vec(info)
        .map_err(|_| error("ATTACHMENT_WRITE_FAILED", "Cannot encode image metadata."))?;
    check_path(cached)?;
    dsh_atomic_write::write_file_atomic(
        cached,
        &bytes,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(io_error)
}
pub(crate) async fn open_variant(
    cached: &Path,
    signal: Option<&AttachmentAbort>,
) -> Result<Option<(std::fs::File, ImageInfo)>, AttachmentError> {
    check_cancel(signal)?;
    check_path(cached)?;
    if !cached.exists() {
        return Ok(None);
    }
    let mut data = Vec::new();
    read_handle(cached)?
        .take(4097)
        .read_to_end(&mut data)
        .map_err(io_error)?;
    if data.len() > 4096 {
        return Err(error(
            "ATTACHMENT_CORRUPT",
            "Image metadata exceeds its bound.",
        ));
    }
    let info: ImageInfo = serde_json::from_slice(&data)
        .map_err(|_| error("ATTACHMENT_CORRUPT", "Image metadata is invalid."))?;
    if info.sha256.len() != 64
        || !info
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(error(
            "ATTACHMENT_CORRUPT",
            "Image metadata digest is invalid.",
        ));
    }
    let mut file = verify_hash(
        read_handle(&cached.with_file_name(format!("{}.blob", info.sha256)))?,
        &info.sha256,
        info.bytes,
        signal,
    )
    .await?;
    let metadata = probe_handle(&mut file)?;
    if metadata.width != info.width
        || metadata.height != info.height
        || metadata.media_type != info.media_type
    {
        return Err(error(
            "ATTACHMENT_CORRUPT",
            "Image variant metadata does not match its bytes.",
        ));
    }
    Ok(Some((file, info)))
}
