use super::*;
use dsh_attachment::{ImageAttachmentLimits, SaveImageAttachment};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
#[ignore = "private child codec entry, invoked by the supervisor"]
fn worker_entry() {
    run_worker().unwrap();
}

#[test]
#[ignore = "owned stalled worker fixture"]
fn stalled_worker_entry() {
    let mut gate = [0; 12];
    std::io::stdin().read_exact(&mut gate).unwrap();
    let root = PathBuf::from(std::env::var_os("DSH_IMAGE_CODEC_JOB").unwrap());
    std::fs::write(root.join("output"), std::process::id().to_string()).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
#[test]
#[ignore = "owned forged reply fixture"]
fn forged_worker_entry() {
    let mut gate = [0; 12];
    std::io::stdin().read_exact(&mut gate).unwrap();
    let root = PathBuf::from(std::env::var_os("DSH_IMAGE_CODEC_JOB").unwrap());
    write_metadata(
        &root.join("reply.json"),
        &Reply {
            version: VERSION,
            nonce: uuid::Uuid::new_v4().to_string(),
            image: None,
            error: Some(("INVALID_IMAGE".into(), "forged".into())),
        },
    )
    .unwrap();
}
struct Temp(PathBuf);
#[test]
fn missing_os_temporary_base_is_rejected() {
    let temporary = Temp::new();
    assert!(crate::store::temporary_codec_root(&temporary.0).is_err());
}

#[cfg(unix)]
#[test]
fn os_temporary_alias_is_resolved_without_trusting_owned_links() {
    let temporary = Temp::new();
    let physical = temporary.0.join("physical");
    std::fs::create_dir_all(&physical).unwrap();
    let alias = temporary.0.join("alias");
    std::os::unix::fs::symlink(&physical, &alias).unwrap();
    let root = crate::store::temporary_codec_root(&alias).unwrap();
    assert_eq!(root, physical.join("dsh-image-codec-v1"));
    assert!(crate::codec::check_path(&root).is_ok());
    let outside = temporary.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &root).unwrap();
    assert_eq!(
        crate::codec::check_path(&root).unwrap_err().code,
        "ATTACHMENT_UNSAFE_PATH"
    );
}

impl Temp {
    fn new() -> Self {
        Self(
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("codec-regression-{}", uuid::Uuid::new_v4())),
        )
    }
    fn root(&self) -> PathBuf {
        self.0.join("attachments/v1")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(width, height)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}
fn limits() -> ImageAttachmentLimits {
    ImageAttachmentLimits {
        max_image_bytes: 0,
        max_images_per_message: 20,
        max_message_image_bytes: 0,
        max_image_pixels: 40_000_000,
        media_types: vec![ImageMediaType::Png],
    }
}
async fn staged(root: &Path) -> CodecJob {
    CodecJob::stage(
        root,
        &mut png(8, 8).as_slice(),
        ImageMediaType::Png,
        u64::MAX,
        40_000_000,
        None,
    )
    .await
    .unwrap()
}
fn fixture(name: &str) -> CodecWorkerCommand {
    CodecWorkerCommand {
        program: std::env::current_exe().unwrap(),
        arguments: vec![
            "--exact".into(),
            format!("codec::tests::{name}"),
            "--ignored".into(),
            "--nocapture".into(),
        ],
    }
}
fn jobs_are_empty(root: &Path) -> bool {
    std::fs::read_dir(root.join("codec-jobs"))
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true)
}

#[tokio::test]
async fn damaged_bytes_and_mismatched_media_are_rejected_without_publication() {
    let temp = Temp::new();
    let good = png(16, 16);
    for (data, media_type, code) in [
        (
            good[..good.len() / 2].to_vec(),
            ImageMediaType::Png,
            "INVALID_IMAGE",
        ),
        (good.clone(), ImageMediaType::Jpeg, "IMAGE_TYPE_MISMATCH"),
    ] {
        let result = crate::store::save_image_file(
            &temp.root(),
            &SaveImageAttachment {
                data,
                media_type,
                name: None,
            },
            &limits(),
        )
        .await;
        assert_eq!(result.unwrap_err().code, code);
        assert!(jobs_are_empty(&temp.root()));
        assert!(!temp.root().join("objects").exists());
    }
    let mut policy = limits();
    policy.max_image_pixels = 4;
    assert_eq!(
        crate::store::save_image_file(
            &temp.root(),
            &SaveImageAttachment {
                data: good,
                media_type: ImageMediaType::Png,
                name: None
            },
            &policy
        )
        .await
        .unwrap_err()
        .code,
        "IMAGE_TOO_MANY_PIXELS"
    );
}

#[tokio::test]
async fn batches_exceeding_worker_count_finish_and_invalid_tail_publishes_nothing() {
    let temp = Temp::new();
    let input = SaveImageAttachment {
        data: png(8, 8),
        media_type: ImageMediaType::Png,
        name: Some("../../photo.png".into()),
    };
    let mut inputs = vec![input.clone(); 20];
    inputs.push(SaveImageAttachment {
        data: vec![0; 50],
        ..input
    });
    assert!(
        tokio::time::timeout(
            Duration::from_secs(20),
            crate::store::save_images_files(&temp.root(), &inputs, &limits())
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(!temp.root().join("objects").exists());
    assert!(jobs_are_empty(&temp.root()));
    inputs.pop();
    let references = crate::store::save_images_files(&temp.root(), &inputs, &limits())
        .await
        .unwrap();
    assert_eq!(references.len(), 20);
    assert!(
        references
            .iter()
            .all(|reference| reference.name.as_deref() == Some("photo.png"))
    );
    assert!(jobs_are_empty(&temp.root()));
}

#[tokio::test]
async fn stream_transform_keeps_master_and_matches_exact_resampler() {
    let temp = Temp::new();
    let data = png(1024, 600);
    let reference = crate::store::save_image_stream(
        &temp.root(),
        Box::pin(std::io::Cursor::new(data.clone())),
        ImageMediaType::Png,
        None,
        &limits(),
        None,
    )
    .await
    .unwrap();
    let policy = RequestImagePolicy {
        max_pixels: 10_000,
        max_bytes: 1_000_000,
        preferred_media_type: ImageMediaType::Png,
    };
    let expected = crate::request_image::transform(&data, &policy, None).unwrap();
    for _ in 0..2 {
        let mut stream =
            crate::request_image::open_request_image_file(&temp.root(), &reference, &policy, None)
                .await
                .unwrap();
        let mut actual = Vec::new();
        stream.reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, expected.0);
        assert_eq!((stream.width, stream.height), (expected.1, expected.2));
    }
    assert_eq!(
        crate::store::read_image_file(&temp.root(), &reference, None)
            .await
            .unwrap()
            .data,
        data
    );
    assert!(jobs_are_empty(&temp.root()));
}

#[tokio::test]
async fn oversized_or_cancelled_stream_never_starts_a_worker() {
    let temp = Temp::new();
    let mut bytes = &[1u8; 200][..];
    let error = match CodecJob::stage(
        &temp.root(),
        &mut bytes,
        ImageMediaType::Png,
        100,
        100,
        None,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("accepted oversize stream"),
    };
    assert_eq!(error.code, "IMAGE_TOO_LARGE");
    assert!(jobs_are_empty(&temp.root()));
    let signal: AttachmentAbort = Arc::new(|| true);
    assert!(
        CodecJob::stage(
            &temp.root(),
            &mut bytes,
            ImageMediaType::Png,
            100,
            100,
            Some(&signal)
        )
        .await
        .is_err()
    );
    assert!(jobs_are_empty(&temp.root()));
}

#[tokio::test]
async fn wrong_job_reply_and_worker_exit_are_not_accepted() {
    let temp = Temp::new();
    for command in [fixture("forged_worker_entry"), fixture("nonexistent_entry")] {
        let job = staged(&temp.root()).await;
        let (mut tx, _rx) = tokio::sync::oneshot::channel();
        let result = job
            .supervise(command, &mut tx, None, Duration::from_secs(5))
            .await;
        assert!(result.is_err());
        assert!(jobs_are_empty(&temp.root()));
    }
}

#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::*;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0;
    let alive = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == 259;
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(handle);
    }
    alive
}
#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[tokio::test]
async fn cancellation_and_dropped_callers_reap_worker_before_cleanup() {
    for drop_caller in [false, true] {
        let temp = Temp::new();
        let job = staged(&temp.root()).await;
        let folder = job.directory.path.clone();
        let flag = Arc::new(AtomicBool::new(false));
        let check = flag.clone();
        let signal: AttachmentAbort = Arc::new(move || check.load(Ordering::SeqCst));
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        let supervisor = tokio::spawn(async move {
            let result = job
                .supervise(
                    fixture("stalled_worker_entry"),
                    &mut tx,
                    Some(&signal),
                    Duration::from_secs(10),
                )
                .await;
            let _ = tx.send(result);
        });
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(folder.join("output")) {
                    if let Ok(pid) = text.parse::<u32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(is_alive(pid));
        if drop_caller {
            drop(rx);
        } else {
            flag.store(true, Ordering::SeqCst);
            match rx.await.unwrap() {
                Err(e) => assert_eq!(e.code, "ATTACHMENT_ABORTED"),
                Ok(_) => panic!("cancel accepted"),
            }
        }
        supervisor.await.unwrap();
        assert!(!is_alive(pid));
        assert!(!folder.exists());
    }
}
#[tokio::test]
async fn timeout_and_path_escape_do_not_publish() {
    let temp = Temp::new();
    let job = staged(&temp.root()).await;
    let (mut tx, _rx) = tokio::sync::oneshot::channel();
    let result = job
        .supervise(
            fixture("stalled_worker_entry"),
            &mut tx,
            None,
            Duration::from_millis(500),
        )
        .await;
    match result {
        Err(e) => assert_eq!(e.code, "CODEC_WORKER_TIMEOUT"),
        Ok(_) => panic!("timeout accepted"),
    }
    assert!(jobs_are_empty(&temp.root()));
    let reference = dsh_attachment::ImageAttachmentRef {
        attachment_id: dsh_attachment::attachment_id("sha256:../../secret"),
        media_type: ImageMediaType::Png,
        bytes: 1,
        width: 1,
        height: 1,
        name: None,
    };
    assert_eq!(
        crate::store::read_image_file(&temp.root(), &reference, None)
            .await
            .unwrap_err()
            .code,
        "INVALID_ATTACHMENT_REF"
    );
}

#[tokio::test]
async fn recovery_preserves_active_and_unrecognized_directories() {
    let temp = Temp::new();
    let parent = temp.root().join("codec-jobs");
    let active = staged(&temp.root()).await;
    let active_path = active.directory.path.clone();
    let stale_nonce = uuid::Uuid::new_v4().to_string();
    let stale = parent.join(&stale_nonce);
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(
        stale.join("owner.json"),
        serde_json::json!({"version":VERSION,"nonce":stale_nonce}).to_string(),
    )
    .unwrap();
    std::fs::write(stale.join("input"), b"abandoned private input").unwrap();
    let unknown = parent.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join("owner.json"), b"user content").unwrap();
    directory::recover(&parent).unwrap();
    assert!(!stale.exists());
    assert!(active_path.join("input").exists());
    assert_eq!(
        std::fs::read(unknown.join("owner.json")).unwrap(),
        b"user content"
    );
    drop(active);
    assert!(!active_path.exists());
}

#[tokio::test]
async fn png_sixteen_bit_is_preserved_and_late_animation_corruption_is_rejected() {
    use base64::Engine;
    let temp = Temp::new();
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba16(100, 100)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
    let bytes = cursor.into_inner();
    let reference = crate::store::save_image_file(
        &temp.root(),
        &SaveImageAttachment {
            data: bytes.clone(),
            media_type: ImageMediaType::Png,
            name: None,
        },
        &limits(),
    )
    .await
    .unwrap();
    assert_eq!(
        crate::store::read_image_file(&temp.root(), &reference, None)
            .await
            .unwrap()
            .data,
        bytes
    );
    let mut gif = base64::engine::general_purpose::STANDARD.decode("R0lGODlhAgABAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgABAAAIBQABAAgIACH5BAEKAAEALAAAAAACAAEAgQAA/wAAAAAAAAAAAAgFAAEACAgAOw==").unwrap();
    gif.truncate(gif.len() - 7);
    // The first image is decodable: failure must come from subsequent frames.
    assert!(crate::image::detect_image(&gif, Some(100)).is_ok());
    let result = crate::store::save_image_file(
        &temp.root(),
        &SaveImageAttachment {
            data: gif,
            media_type: ImageMediaType::Gif,
            name: None,
        },
        &limits(),
    )
    .await;
    assert_eq!(result.unwrap_err().code, "INVALID_IMAGE");
}
