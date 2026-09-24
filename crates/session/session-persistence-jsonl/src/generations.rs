//! Version-qualified artifacts and per-Session writer ownership. A newer
//! unsupported generation is a refusal, never permission to resume an old log.

use crate::{
    JsonlCompression,
    format::{log_suffix, parse_header_storage},
    index::read_authority_header,
};
use dsh_session::format_v4::{V3Dialect, decode_v3_header, decode_v4_header};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

mod source;
pub(crate) use source::StableGenerationSource;
mod migration;
pub use migration::{V4MigrationResult, migrate_recovered_v3_to_v4, migrate_v3_to_v4};

#[derive(Clone, Debug, PartialEq)]
pub struct SessionGeneration {
    pub version: u64,
    pub path: PathBuf,
    pub compression: JsonlCompression,
    pub physical_header: Value,
}

fn version(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| {
                    n.is_finite()
                        && !n.is_sign_negative()
                        && *n <= 9_007_199_254_740_991.0
                        && n.fract() == 0.0
                })
                .map(|n| n as u64)
        })
        .ok_or_else(|| "invalid Session generation version".into())
}

fn filename(name: &str) -> Result<Option<(Option<u64>, JsonlCompression)>, String> {
    for compression in [JsonlCompression::Zstd, JsonlCompression::None] {
        let suffix = log_suffix(compression);
        if name == format!("session{suffix}") {
            return Ok(Some((None, compression)));
        }
        if let Some(number) = name
            .strip_prefix("session.v")
            .and_then(|name| name.strip_suffix(suffix))
        {
            if number.is_empty()
                || !number.bytes().all(|c| c.is_ascii_digit())
                || number.len() > 1 && number.starts_with('0')
            {
                return Err(format!("unrecognized versioned Session artifact {name}"));
            }
            let version = number
                .parse::<u64>()
                .map_err(|_| "Session filename generation overflows")?;
            return Ok(Some((Some(version), compression)));
        }
    }
    Ok(None)
}

fn regular(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err(format!(
            "Session artifact must be a regular file: {}",
            path.display()
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("Session artifact must not be a reparse point".into());
        }
    }
    Ok(())
}

/// Inspect one owned Session directory. Older generations remain present, but
/// two files claiming the same generation or a filename/header disagreement
/// require intervention rather than an arbitrary winner.
pub fn select_generation(
    directory: &Path,
    expected_id: &str,
) -> Result<Option<SessionGeneration>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut generations = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((claimed, compression)) = filename(name)? else {
            continue;
        };
        if claimed.is_some_and(|version| version > 4) {
            return Err(format!(
                "unsupported newer Session generation v{}; retain all generations",
                claimed.unwrap()
            ));
        }
        let path = entry.path();
        regular(&path)?;
        let line = read_authority_header(&path, compression)?
            .ok_or("Session generation has no complete header")?;
        let header: Value = serde_json::from_str(&line)
            .map_err(|e| format!("invalid Session generation header: {e}"))?;
        if header["type"] != "session" || header["id"].as_str() != Some(expected_id) {
            return Err("Session generation identity does not match its directory".into());
        }
        let actual = version(&header["version"])?;
        if claimed.is_some_and(|claimed| claimed != actual) {
            return Err("Session filename generation disagrees with its header".into());
        }
        if actual > 4 {
            return Err(format!(
                "unsupported newer Session generation v{actual}; retain all generations"
            ));
        }
        let generation = SessionGeneration {
            version: actual,
            path,
            compression,
            physical_header: header,
        };
        if generations.insert(actual, generation).is_some() {
            return Err(format!(
                "ambiguous Session generation v{actual}; multiple artifacts must not be merged implicitly"
            ));
        }
    }
    let Some((_, selected)) = generations.pop_last() else {
        return Ok(None);
    };
    match selected.version {
        4 => {
            decode_v4_header(selected.physical_header.clone())?;
        }
        3 => {
            decode_v3_header(selected.physical_header.clone(), V3Dialect::Rust)?;
        }
        0 => {
            parse_header_storage(&selected.physical_header.to_string())
                .ok_or("invalid legacy Session header")?;
        }
        version => {
            return Err(format!(
                "Session generation v{version} requires its historical import edge"
            ));
        }
    }
    Ok(Some(selected))
}

/// Discover the owned identity while retaining the same ambiguity and version
/// checks used by direct lookup. Staging files are not generation candidates.
pub fn discover_generation(directory: &Path) -> Result<Option<SessionGeneration>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((_, compression)) = filename(&name)? else {
            continue;
        };
        regular(&entry.path())?;
        let line = read_authority_header(&entry.path(), compression)?
            .ok_or("missing Session authority header")?;
        let header: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        let id = header["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Session authority header has no identity")?;
        if directory.file_name().and_then(|name| name.to_str())
            != Some(crate::format::encode_segment(id)?.as_str())
        {
            return Err("Session header identity does not match its directory".into());
        }
        return select_generation(directory, id);
    }
    Ok(None)
}

pub fn generation_path(
    directory: &Path,
    version: u64,
    compression: JsonlCompression,
) -> Result<PathBuf, String> {
    if version > 9_007_199_254_740_991 {
        return Err("invalid generation version".into());
    }
    Ok(directory.join(format!("session.v{version}{}", log_suffix(compression))))
}

/// Held for a writable Session's whole residency, including migration and
/// repair. Readers do not acquire this lease. The lock file is never unlinked:
/// another process must contend on the same inode after owner termination.
pub struct SessionGenerationLease {
    directory: PathBuf,
    _file: File,
}
impl SessionGenerationLease {
    pub fn acquire(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory)
            .map_err(|e| format!("cannot create Session ownership directory: {e}"))?;
        let meta = std::fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("Session ownership directory must not be a symlink".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("Session ownership directory must not be a reparse point".into());
            }
        }
        let directory = std::fs::canonicalize(directory).map_err(|e| e.to_string())?;
        let path = directory.join(".session-writer.lock");
        if path.exists() {
            regular(&path)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0x1 | 0x2);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|e| format!("SESSION_OWNERSHIP_FAILED: cannot open writer lease: {e}"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                directory,
                _file: file,
            }),
            Err(std::fs::TryLockError::WouldBlock) => Err(
                "SESSION_IN_USE: another process owns this Session; no writer was admitted".into(),
            ),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("SESSION_OWNERSHIP_FAILED: {error}"))
            }
        }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[cfg(test)]
mod tests;
