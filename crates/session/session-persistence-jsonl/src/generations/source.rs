use super::regular;
use crate::v4_artifact::check_cancel;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

fn identity(metadata: &Metadata) -> Vec<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        vec![
            metadata.file_size(),
            metadata.creation_time(),
            metadata.last_write_time(),
        ]
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        vec![
            metadata.len(),
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ]
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    identity: Vec<u64>,
    hash: [u8; 32],
}

fn fingerprint(file: &mut File, cancelled: &impl Fn() -> bool) -> Result<Fingerprint, String> {
    let before = identity(&file.metadata().map_err(|e| e.to_string())?);
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancelled)?;
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if before != identity(&file.metadata().map_err(|e| e.to_string())?) {
        return Err("Session source changed during fingerprinting".into());
    }
    Ok(Fingerprint {
        identity: before,
        hash: hash.finalize().into(),
    })
}

fn open(path: &Path) -> Result<File, String> {
    regular(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Protect the bytes and path until publication. Ordinary readers work.
        options.share_mode(0x1);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("cannot reserve Session source: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Session source is not a regular file".into());
    }
    Ok(file)
}

pub(crate) struct StableGenerationSource {
    path: PathBuf,
    file: File,
    fingerprint: Fingerprint,
}
impl StableGenerationSource {
    pub(crate) fn open(path: &Path, cancelled: &impl Fn() -> bool) -> Result<Self, String> {
        check_cancel(cancelled)?;
        let mut file = open(path)?;
        let fingerprint = fingerprint(&mut file, cancelled)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            fingerprint,
        })
    }
    pub(crate) fn file(&mut self) -> &mut File {
        &mut self.file
    }
    pub(crate) fn sha256(&self) -> String {
        self.fingerprint
            .hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    pub(crate) fn assert_unchanged(&mut self, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        if fingerprint(&mut self.file, cancelled)? != self.fingerprint
            || fingerprint(&mut open(&self.path)?, cancelled)? != self.fingerprint
        {
            return Err("Session source changed before publication; original retained".into());
        }
        Ok(())
    }
}
