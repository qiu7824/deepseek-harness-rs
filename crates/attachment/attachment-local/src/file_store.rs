//! Verbatim file storage with bounded hashing and no-overwrite publication.
use crate::codec::{check_path, io_error, read_handle};
use dsh_attachment::{
    AttachmentAbort, AttachmentError, FileAttachmentRef, FileAttachmentStream, attachment_id,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const BUFFER: usize = 64 * 1024;
const MAX_SAFE_BYTES: u64 = 9_007_199_254_740_991;

fn invalid(message: &str) -> AttachmentError {
    AttachmentError::new("INVALID_FILE_ATTACHMENT", message)
}
fn cancel(signal: Option<&AttachmentAbort>) -> Result<(), AttachmentError> {
    if signal.is_some_and(|signal| signal()) {
        Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "File attachment processing cancelled.",
        ))
    } else {
        Ok(())
    }
}
async fn cancelled(signal: Option<&AttachmentAbort>) {
    loop {
        if cancel(signal).is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn validate_name(name: &str) -> Result<(), AttachmentError> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 240
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
    {
        return Err(invalid(
            "File attachment name must be one safe display filename.",
        ));
    }
    Ok(())
}

pub(crate) fn path(root: &Path, reference: &FileAttachmentRef) -> Result<PathBuf, AttachmentError> {
    validate_name(&reference.name)?;
    let digest = reference
        .attachment_id
        .as_str()
        .strip_prefix("sha256:")
        .filter(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .ok_or_else(|| invalid("File attachment identity must be a SHA-256 digest."))?;
    if reference.bytes > MAX_SAFE_BYTES {
        return Err(invalid("File byte count exceeds the wire limit."));
    }
    Ok(root
        .join("files")
        .join(&digest[..2])
        .join(digest)
        .join(&reference.name))
}

async fn directory(path: &Path) -> Result<(), AttachmentError> {
    check_path(path)?;
    tokio::fs::create_dir_all(path).await.map_err(io_error)?;
    check_path(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(io_error)?;
    }
    Ok(())
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(crate) async fn save(
    root: &Path,
    mut reader: dsh_attachment::FileUploadReader<'_>,
    name: &str,
    signal: Option<&AttachmentAbort>,
) -> Result<FileAttachmentRef, AttachmentError> {
    validate_name(name)?;
    cancel(signal)?;
    let staging = root.join("file-staging");
    directory(&staging).await?;
    let stage = Stage(staging.join(uuid::Uuid::new_v4().to_string()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage.0)
        .await
        .map_err(io_error)?;
    let mut buffer = vec![0; BUFFER];
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    loop {
        cancel(signal)?;
        let count = tokio::select! {
            result = reader.read(&mut buffer) => result.map_err(io_error)?,
            _ = cancelled(signal) => return Err(AttachmentError::new("ATTACHMENT_ABORTED", "File attachment processing cancelled.")),
        };
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .filter(|bytes| *bytes <= MAX_SAFE_BYTES)
            .ok_or_else(|| invalid("File byte count exceeds the wire limit."))?;
        digest.update(&buffer[..count]);
        file.write_all(&buffer[..count]).await.map_err(io_error)?;
    }
    file.flush().await.map_err(io_error)?;
    file.sync_all().await.map_err(io_error)?;
    drop(file);
    cancel(signal)?;
    let reference = FileAttachmentRef {
        attachment_id: attachment_id(format!("sha256:{:x}", digest.finalize())),
        name: name.into(),
        bytes,
    };
    let target = path(root, &reference)?;
    directory(target.parent().unwrap()).await?;
    check_path(&target)?;
    match tokio::fs::hard_link(&stage.0, &target).await {
        Ok(()) => {
            tokio::fs::remove_file(&stage.0).await.map_err(io_error)?;
            let mut permissions = tokio::fs::metadata(&target)
                .await
                .map_err(io_error)?
                .permissions();
            permissions.set_readonly(true);
            tokio::fs::set_permissions(&target, permissions)
                .await
                .map_err(io_error)?;
            #[cfg(unix)]
            std::fs::File::open(target.parent().unwrap())
                .and_then(|file| file.sync_all())
                .map_err(io_error)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent save is reusable only after byte and identity verification.
            drop(open(root, &reference, signal).await?);
        }
        Err(error) => return Err(io_error(error)),
    }
    Ok(reference)
}

pub(crate) async fn open(
    root: &Path,
    reference: &FileAttachmentRef,
    signal: Option<&AttachmentAbort>,
) -> Result<FileAttachmentStream, AttachmentError> {
    cancel(signal)?;
    let target = path(root, reference)?;
    let mut file = tokio::fs::File::from_std(read_handle(&target)?);
    if file.metadata().await.map_err(io_error)?.len() != reference.bytes {
        return Err(AttachmentError::new(
            "ATTACHMENT_CORRUPT",
            "File attachment size does not match its recorded reference.",
        ));
    }
    let mut buffer = vec![0; BUFFER];
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    loop {
        cancel(signal)?;
        let count = file.read(&mut buffer).await.map_err(io_error)?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| invalid("File byte count overflow."))?;
        digest.update(&buffer[..count]);
    }
    if bytes != reference.bytes
        || format!("sha256:{:x}", digest.finalize()) != reference.attachment_id.as_str()
    {
        return Err(AttachmentError::new(
            "ATTACHMENT_CORRUPT",
            "File attachment bytes do not match its recorded digest.",
        ));
    }
    file.rewind().await.map_err(io_error)?;
    cancel(signal)?;
    Ok(FileAttachmentStream {
        reference: reference.clone(),
        reader: Box::pin(file),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_attachment::AttachmentReader;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        files: Vec<PathBuf>,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("dsh-verbatim-files-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self {
                root,
                files: vec![],
            }
        }
        fn track(&mut self, reference: &FileAttachmentRef) -> PathBuf {
            let path = path(&self.root, reference).unwrap();
            self.files.push(path.clone());
            path
        }
    }
    fn writable(path: &Path) {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        #[cfg(windows)]
        permissions.set_readonly(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
        }
        std::fs::set_permissions(path, permissions).unwrap();
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for file in &self.files {
                if file.is_file() {
                    writable(file);
                }
            }
            assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn reader(bytes: &[u8]) -> AttachmentReader {
        Box::pin(std::io::Cursor::new(bytes.to_vec()))
    }

    #[tokio::test]
    async fn bytes_are_verbatim_content_addressed_readonly_and_verified_on_every_open() {
        let mut fixture = Fixture::new();
        let bytes = b"\0\xffDOCX\r\nverbatim";
        let reference = save(&fixture.root, reader(bytes), "设计.docx", None)
            .await
            .unwrap();
        let target = fixture.track(&reference);
        assert_eq!(
            reference.attachment_id.as_str(),
            format!("sha256:{:x}", Sha256::digest(bytes))
        );
        assert!(std::fs::metadata(&target).unwrap().permissions().readonly());
        assert_eq!(
            reference,
            save(&fixture.root, reader(bytes), "设计.docx", None)
                .await
                .unwrap()
        );
        let second = save(&fixture.root, reader(bytes), "copy.docx", None)
            .await
            .unwrap();
        fixture.track(&second);
        assert_eq!(second.attachment_id, reference.attachment_id);
        let mut restored = open(&fixture.root, &reference, None).await.unwrap();
        let mut content = vec![];
        restored.reader.read_to_end(&mut content).await.unwrap();
        assert_eq!(content, bytes);
        drop(restored);
        writable(&target);
        std::fs::write(&target, vec![0x20; bytes.len()]).unwrap();
        assert_eq!(
            open(&fixture.root, &reference, None)
                .await
                .err()
                .unwrap()
                .code,
            "ATTACHMENT_CORRUPT"
        );
        assert_eq!(
            save(&fixture.root, reader(bytes), "设计.docx", None)
                .await
                .err()
                .unwrap()
                .code,
            "ATTACHMENT_CORRUPT"
        );
        assert_eq!(
            std::fs::read(target).unwrap(),
            vec![0x20; bytes.len()],
            "a retry must not overwrite an existing object"
        );
    }

    #[tokio::test]
    async fn cancelled_stalled_upload_leaves_no_published_or_staged_file() {
        let fixture = Fixture::new();
        let (input, _writer) = tokio::io::duplex(1);
        let aborted = Arc::new(AtomicBool::new(false));
        let signal: AttachmentAbort = {
            let aborted = aborted.clone();
            Arc::new(move || aborted.load(Ordering::Acquire))
        };
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            aborted.store(true, Ordering::Release);
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            save(&fixture.root, Box::pin(input), "pending.pdf", Some(&signal)),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "ATTACHMENT_ABORTED");
        assert!(!fixture.root.join("files").exists());
        assert_eq!(
            std::fs::read_dir(fixture.root.join("file-staging"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn references_cannot_escape_storage_or_alias_windows_device_names() {
        for name in [
            "../secret",
            "C:\\secret",
            "a/b",
            "NUL.txt",
            "NUL .txt",
            "CON",
            "x.",
            "",
            "a\nb",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        let reference = FileAttachmentRef {
            attachment_id: attachment_id("../../private"),
            name: "valid.txt".into(),
            bytes: 1,
        };
        assert!(path(Path::new("storage"), &reference).is_err());
    }
}
