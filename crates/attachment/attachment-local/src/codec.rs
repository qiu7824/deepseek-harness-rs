//! Versioned, file-backed image codec. Host admission and transforms never
//! decode pixels. Cancellation reaps the owned worker before releasing its slot.
use dsh_attachment::{AttachmentAbort, AttachmentError, ImageMediaType, RequestImagePolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[path = "codec_directory.rs"]
mod directory;
#[path = "codec_job.rs"]
mod process_job;
use directory::Directory;
const VERSION: u32 = 1;
const META_LIMIT: u64 = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);
static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static COMMAND: OnceLock<CodecWorkerCommand> = OnceLock::new();

/// Embedders with their own entry point explicitly supply an equivalent worker.
/// The product uses its own executable and never falls back to in-process decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecWorkerCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}
pub fn configure_worker(command: CodecWorkerCommand) -> Result<(), AttachmentError> {
    if COMMAND.get().is_some_and(|installed| installed == &command) {
        return Ok(());
    }
    COMMAND
        .set(command)
        .or_else(|command| {
            if COMMAND.get() == Some(&command) {
                Ok(())
            } else {
                Err(command)
            }
        })
        .map_err(|_| {
            error(
                "CODEC_CONFIGURATION_CONFLICT",
                "Image worker is already configured.",
            )
        })
}
fn command() -> Result<CodecWorkerCommand, AttachmentError> {
    if let Some(command) = COMMAND.get() {
        return Ok(command.clone());
    }
    #[cfg(test)]
    let arguments = vec![
        "--exact".into(),
        "codec::tests::worker_entry".into(),
        "--ignored".into(),
        "--nocapture".into(),
    ];
    #[cfg(not(test))]
    let arguments = vec!["__dsh-image-codec".into()];
    Ok(CodecWorkerCommand {
        program: std::env::current_exe().map_err(io_error)?,
        arguments,
    })
}
pub(crate) fn error(code: &str, message: &str) -> AttachmentError {
    AttachmentError::new(code, message)
}
pub(crate) fn io_error(e: std::io::Error) -> AttachmentError {
    AttachmentError::new("ATTACHMENT_IO_FAILED", e.to_string())
}
pub(crate) fn check_cancel(signal: Option<&AttachmentAbort>) -> Result<(), AttachmentError> {
    if signal.is_some_and(|signal| signal()) {
        Err(error("ATTACHMENT_ABORTED", "Image processing cancelled."))
    } else {
        Ok(())
    }
}
async fn cancelled(signal: Option<&AttachmentAbort>) {
    loop {
        if check_cancel(signal).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageInfo {
    pub sha256: String,
    pub bytes: u64,
    pub media_type: ImageMediaType,
    pub width: u64,
    pub height: u64,
    pub has_alpha: bool,
    pub preserved: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    nonce: String,
    sha256: String,
    bytes: u64,
    media_type: ImageMediaType,
    max_pixels: u64,
    policy: Option<RequestImagePolicy>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u32,
    nonce: String,
    image: Option<ImageInfo>,
    error: Option<(String, String)>,
}

// Reparse points are refused at every owned path boundary, including ancestors.
pub(crate) fn check_path(path: &Path) -> Result<(), AttachmentError> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let linked = metadata.file_type().is_symlink();
                if linked {
                    return Err(error(
                        "ATTACHMENT_UNSAFE_PATH",
                        "Image storage must not traverse links or reparse points.",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}
pub(crate) fn read_handle(path: &Path) -> Result<std::fs::File, AttachmentError> {
    check_path(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Immutable lease: deny write/delete while this handle is alive.
        options.share_mode(1);
    }
    let handle = options.open(path).map_err(io_error)?;
    if !handle.metadata().map_err(io_error)?.is_file() {
        return Err(error(
            "ATTACHMENT_UNSAFE_PATH",
            "Image input must be a regular file.",
        ));
    }
    Ok(handle)
}
pub(crate) fn hash_reader(mut input: impl Read) -> Result<(String, u64), AttachmentError> {
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok((format!("{:x}", digest.finalize()), bytes))
}
fn read_metadata<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, AttachmentError> {
    let mut bytes = Vec::new();
    read_handle(path)?
        .take(META_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > META_LIMIT {
        return Err(error(
            "CODEC_PROTOCOL_ERROR",
            "Image worker metadata exceeds its bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| error("CODEC_PROTOCOL_ERROR", "Invalid image worker metadata."))
}
fn write_metadata(path: &Path, value: &impl Serialize) -> Result<(), AttachmentError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        error(
            "CODEC_PROTOCOL_ERROR",
            "Cannot encode image worker metadata.",
        )
    })?;
    if bytes.len() as u64 > META_LIMIT {
        return Err(error(
            "CODEC_PROTOCOL_ERROR",
            "Image worker metadata exceeds its bound.",
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

pub(crate) struct CodecJob {
    directory: Directory,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
    request: Request,
}
impl CodecJob {
    pub(crate) fn input(&self) -> PathBuf {
        self.directory.path.join("input")
    }
    pub(crate) fn output(&self) -> PathBuf {
        self.directory.path.join("output")
    }
    pub(crate) async fn stage(
        root: &Path,
        reader: &mut (impl AsyncRead + Unpin + Send + ?Sized),
        media_type: ImageMediaType,
        max_bytes: u64,
        max_pixels: u64,
        signal: Option<&AttachmentAbort>,
    ) -> Result<Self, AttachmentError> {
        check_cancel(signal)?;
        let acquire = SLOTS
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
            .clone()
            .acquire_owned();
        let permit = tokio::select! {
            permit = acquire => permit.map_err(|_| error("ATTACHMENT_UNAVAILABLE", "Image worker pool closed."))?,
            _ = cancelled(signal) => return Err(error("ATTACHMENT_ABORTED", "Image processing cancelled.")),
        };
        let nonce = uuid::Uuid::new_v4().to_string();
        let directory = Directory::create(&root.join("codec-jobs"), &nonce)?;
        let mut output = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.path.join("input"))
            .await
            .map_err(io_error)?;
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let count = tokio::select! {
                count = reader.read(&mut buffer) => count.map_err(io_error)?,
                _ = cancelled(signal) => return Err(error("ATTACHMENT_ABORTED", "Image processing cancelled.")),
            };
            if count == 0 {
                break;
            }
            bytes = bytes.checked_add(count as u64).ok_or_else(|| {
                error(
                    "IMAGE_TOO_LARGE",
                    "Image exceeds the configured byte limit.",
                )
            })?;
            if bytes > max_bytes {
                return Err(error(
                    "IMAGE_TOO_LARGE",
                    "Image exceeds the configured byte limit.",
                ));
            }
            output.write_all(&buffer[..count]).await.map_err(io_error)?;
            digest.update(&buffer[..count]);
        }
        output.flush().await.map_err(io_error)?;
        output.sync_all().await.map_err(io_error)?;
        drop(output);
        check_cancel(signal)?;
        Ok(Self {
            directory,
            _permit: Some(permit),
            request: Request {
                version: VERSION,
                nonce,
                sha256: format!("{:x}", digest.finalize()),
                bytes,
                media_type,
                max_pixels,
                policy: None,
            },
        })
    }
    pub(crate) async fn run(
        mut self,
        policy: Option<RequestImagePolicy>,
        signal: Option<&AttachmentAbort>,
    ) -> Result<(Self, ImageInfo), AttachmentError> {
        check_cancel(signal)?;
        self.request.policy = policy;
        write_metadata(&self.directory.path.join("request.json"), &self.request)?;
        let command = command()?;
        let signal = signal.cloned();
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        // The supervisor retains the slot and directory when its caller's future
        // is dropped. Receiver closure cancels and reaps the worker before cleanup.
        tokio::spawn(async move {
            let result = self
                .supervise(command, &mut tx, signal.as_ref(), TIMEOUT)
                .await;
            let _ = tx.send(result);
        });
        rx.await
            .map_err(|_| error("CODEC_WORKER_FAILED", "Image worker supervisor stopped."))?
    }
    async fn supervise(
        mut self,
        command: CodecWorkerCommand,
        tx: &mut tokio::sync::oneshot::Sender<Result<(Self, ImageInfo), AttachmentError>>,
        signal: Option<&AttachmentAbort>,
        timeout: Duration,
    ) -> Result<(Self, ImageInfo), AttachmentError> {
        check_cancel(signal)?;
        if tx.is_closed() {
            return Err(error("ATTACHMENT_ABORTED", "Image processing cancelled."));
        }
        let mut cmd = tokio::process::Command::new(command.program);
        cmd.args(command.arguments)
            .env("DSH_IMAGE_CODEC_JOB", &self.directory.path)
            .env("DSH_IMAGE_CODEC_NONCE", &self.request.nonce)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| AttachmentError::new("CODEC_WORKER_UNAVAILABLE", e.to_string()))?;
        let owned = match process_job::ProcessJob::attach(&child) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(io_error(e));
            }
        };
        let mut stdin = child.stdin.take().expect("piped codec stdin");
        if let Err(e) = stdin.write_all(b"DSH-CODEC-1\n").await {
            drop(owned);
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(io_error(e));
        }
        drop(stdin);
        let outcome = tokio::select! {
            status = child.wait() => status.map_err(io_error),
            _ = tx.closed() => Err(error("ATTACHMENT_ABORTED", "Image processing cancelled.")),
            _ = cancelled(signal) => Err(error("ATTACHMENT_ABORTED", "Image processing cancelled.")),
            _ = tokio::time::sleep(timeout) => Err(error("CODEC_WORKER_TIMEOUT", "Image worker exceeded its time limit.")),
        };
        drop(owned);
        if outcome.is_err() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let status = outcome?;
        if !status.success() {
            return Err(error(
                "CODEC_WORKER_FAILED",
                "Image worker exited without a completed result.",
            ));
        }
        check_cancel(signal)?;
        let reply: Reply = read_metadata(&self.directory.path.join("reply.json"))?;
        if reply.version != VERSION || reply.nonce != self.request.nonce {
            return Err(error(
                "CODEC_PROTOCOL_ERROR",
                "Image worker result belongs to a different job or protocol.",
            ));
        }
        match (reply.image, reply.error) {
            (Some(image), None) => {
                let path = if self.request.policy.is_none() || image.preserved {
                    self.input()
                } else {
                    self.output()
                };
                let (digest, bytes) = hash_reader(read_handle(&path)?)?;
                if digest != image.sha256
                    || bytes != image.bytes
                    || image.width == 0
                    || image.height == 0
                {
                    return Err(error(
                        "ATTACHMENT_CORRUPT",
                        "Image worker result failed integrity verification.",
                    ));
                }
                if let Some(policy) = &self.request.policy {
                    if image.bytes > policy.max_bytes
                        || image
                            .width
                            .checked_mul(image.height)
                            .is_none_or(|pixels| pixels > policy.max_pixels)
                        || (!image.preserved && image.media_type != policy.preferred_media_type)
                    {
                        return Err(error(
                            "CODEC_PROTOCOL_ERROR",
                            "Image worker exceeded the requested policy.",
                        ));
                    }
                } else if image.sha256 != self.request.sha256
                    || image.bytes != self.request.bytes
                    || image.media_type != self.request.media_type
                    || image
                        .width
                        .checked_mul(image.height)
                        .is_none_or(|pixels| pixels > self.request.max_pixels)
                {
                    return Err(error(
                        "CODEC_PROTOCOL_ERROR",
                        "Image worker did not validate the staged input.",
                    ));
                }
                check_cancel(signal)?;
                self._permit.take();
                Ok((self, image))
            }
            (None, Some((code, message))) => Err(AttachmentError::new(code, message)),
            _ => Err(error(
                "CODEC_PROTOCOL_ERROR",
                "Image worker result is incomplete.",
            )),
        }
    }
}

/// Private CLI dispatch, called before constructing a Host runtime.
pub fn run_worker() -> Result<(), AttachmentError> {
    let mut gate = [0u8; 12];
    std::io::stdin().read_exact(&mut gate).map_err(io_error)?;
    if &gate != b"DSH-CODEC-1\n" {
        return Err(error(
            "CODEC_PROTOCOL_ERROR",
            "Image worker launch gate is invalid.",
        ));
    }
    let folder = PathBuf::from(
        std::env::var_os("DSH_IMAGE_CODEC_JOB")
            .ok_or_else(|| error("CODEC_PROTOCOL_ERROR", "Missing image worker job."))?,
    );
    let nonce = std::env::var("DSH_IMAGE_CODEC_NONCE")
        .map_err(|_| error("CODEC_PROTOCOL_ERROR", "Missing image worker identity."))?;
    if uuid::Uuid::parse_str(&nonce).is_err()
        || folder.file_name().and_then(|s| s.to_str()) != Some(nonce.as_str())
    {
        return Err(error(
            "CODEC_PROTOCOL_ERROR",
            "Invalid image worker identity.",
        ));
    }
    check_path(&folder)?;
    let request: Request = read_metadata(&folder.join("request.json"))?;
    if request.version != VERSION || request.nonce != nonce {
        return Err(error(
            "CODEC_PROTOCOL_ERROR",
            "Incompatible image worker request.",
        ));
    }
    let result = process(&folder, &request);
    let (image, error) = match result {
        Ok(image) => (Some(image), None),
        Err(e) => (None, Some((e.code, e.message))),
    };
    write_metadata(
        &folder.join("reply.json"),
        &Reply {
            version: VERSION,
            nonce,
            image,
            error,
        },
    )
}
fn process(folder: &Path, request: &Request) -> Result<ImageInfo, AttachmentError> {
    let mut handle = read_handle(&folder.join("input"))?;
    let mut data = Vec::new();
    handle.read_to_end(&mut data).map_err(io_error)?;
    let digest = format!("{:x}", Sha256::digest(&data));
    if digest != request.sha256 || data.len() as u64 != request.bytes {
        return Err(error("ATTACHMENT_CORRUPT", "Image worker input changed."));
    }
    let detected = crate::image::detect_image(&data, Some(request.max_pixels))?;
    if detected.media_type != request.media_type {
        return Err(error(
            "IMAGE_TYPE_MISMATCH",
            "Declared image type does not match its bytes.",
        ));
    }
    validate_all_frames(&data, detected.media_type)?;
    let master = ImageInfo {
        sha256: digest,
        bytes: data.len() as u64,
        media_type: detected.media_type,
        width: detected.width,
        height: detected.height,
        has_alpha: detected.has_alpha,
        preserved: true,
    };
    let Some(policy) = &request.policy else {
        return Ok(master);
    };
    if policy.max_pixels == 0 || policy.max_bytes == 0 {
        return Err(error(
            "INVALID_REQUEST_IMAGE_POLICY",
            "Request image budgets must be positive.",
        ));
    }
    if crate::request_image::is_animated(&data, detected.media_type) {
        if master.width * master.height <= policy.max_pixels && master.bytes <= policy.max_bytes {
            return Ok(master);
        }
        return Err(error(
            "ANIMATED_REQUEST_IMAGE_TRANSFORM_UNAVAILABLE",
            "Animated images cannot be resized or transcoded without losing frames.",
        ));
    }
    let (data, width, height) = crate::request_image::transform(&data, policy, None)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join("output"))
        .map_err(io_error)?;
    output.write_all(&data).map_err(io_error)?;
    output.sync_all().map_err(io_error)?;
    Ok(ImageInfo {
        sha256: format!("{:x}", Sha256::digest(&data)),
        bytes: data.len() as u64,
        media_type: policy.preferred_media_type,
        width,
        height,
        has_alpha: false,
        preserved: false,
    })
}

fn validate_all_frames(data: &[u8], media_type: ImageMediaType) -> Result<(), AttachmentError> {
    use image::AnimationDecoder;
    let invalid = |_| {
        error(
            "INVALID_IMAGE",
            "Image contains an invalid or truncated frame.",
        )
    };
    match media_type {
        ImageMediaType::Png => {
            // Raw PNG frames retain 16-bit support; image's compositing APNG
            // iterator supports only 8-bit, so it cannot be the admission gate.
            let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
            decoder.set_limits(png::Limits {
                bytes: 512 * 1024 * 1024,
            });
            let mut reader = decoder
                .read_info()
                .map_err(|_| error("INVALID_IMAGE", "Invalid PNG image."))?;
            let frames = reader
                .info()
                .animation_control
                .map(|a| u64::from(a.num_frames) + u64::from(reader.info().frame_control.is_none()))
                .unwrap_or(1);
            let bytes = reader
                .output_buffer_size()
                .ok_or_else(|| error("INVALID_IMAGE", "Invalid PNG buffer dimensions."))?;
            let mut buffer = vec![0; bytes];
            for _ in 0..frames {
                reader.next_frame(&mut buffer).map_err(|_| {
                    error(
                        "INVALID_IMAGE",
                        "Image contains an invalid or truncated frame.",
                    )
                })?;
            }
            reader
                .finish()
                .map_err(|_| error("INVALID_IMAGE", "PNG image did not finish correctly."))?;
        }
        ImageMediaType::Gif => {
            for frame in image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data))
                .map_err(invalid)?
                .into_frames()
            {
                frame.map_err(invalid)?;
            }
        }
        ImageMediaType::Webp => {
            let decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(data))
                .map_err(invalid)?;
            if decoder.has_animation() {
                for frame in decoder.into_frames() {
                    frame.map_err(invalid)?;
                }
            }
        }
        ImageMediaType::Jpeg => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
