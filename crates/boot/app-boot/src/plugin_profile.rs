//! Cross-process profile ownership and recoverable package/config publication.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const LIMIT: usize = 4 * 1024 * 1024;
const JOURNAL: &str = ".dsh-plugin-transaction.json";
const GOOD: &str = ".dsh-plugins-last-good.json";
static NEXT: AtomicU64 = AtomicU64::new(0);
fn nonce() -> String {
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn io(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn regular(path: &Path, directory: bool) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(io)?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(format!(
            "profile path is not a regular {}: {}",
            if directory { "directory" } else { "file" },
            path.display()
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "profile path is a reparse point: {}",
                path.display()
            ));
        }
    }
    Ok(())
}
fn read_optional(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    regular(path, false)?;
    let mut data = String::new();
    File::open(path)
        .map_err(io)?
        .take(LIMIT as u64 + 1)
        .read_to_string(&mut data)
        .map_err(io)?;
    if data.len() > LIMIT {
        return Err(format!(
            "profile document exceeds 4 MiB: {}",
            path.display()
        ));
    }
    Ok(Some(data))
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        regular(path, false)?;
    }
    let temp = path.with_file_name(format!(".dsh-plugin-write-{}", nonce()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io)?;
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match std::fs::rename(&temp, path) {
                Ok(()) => break,
                Err(error)
                    if cfg!(windows)
                        && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(error) => return Err(io(error)),
            }
        }
        #[cfg(unix)]
        File::open(path.parent().ok_or("profile document has no parent")?)
            .and_then(|file| file.sync_all())
            .map_err(io)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}
fn persist(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io)?;
    if bytes.len() > LIMIT {
        return Err(
            "plugin configuration/recovery record exceeds 4 MiB; no oversized record was published"
                .into(),
        );
    }
    atomic(path, &bytes)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Documents {
    pub manifest: Value,
    pub entries: Vec<Value>,
}
fn validate(documents: &Documents) -> Result<(), String> {
    serde_json::from_value::<super::ProfileManifest>(documents.manifest.clone()).map_err(io)?;
    let manifest = documents
        .manifest
        .as_object()
        .ok_or("profile package.json must be an object")?;
    if manifest
        .get("dependencies")
        .is_some_and(|value| !value.is_object())
    {
        return Err("profile dependencies must be an object".into());
    }
    for row in &documents.entries {
        let row = row
            .as_object()
            .ok_or("plugin configuration entries must be objects")?;
        if row
            .get("id")
            .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
            || row
                .get("name")
                .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
        {
            return Err("plugin id/name must be nonempty strings".into());
        }
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct RuntimeDocuments {
    pub documents: Documents,
    pub issue: Option<String>,
}
fn decode(manifest: Option<&str>, entries: Option<&str>) -> Result<Documents, String> {
    let documents = Documents {
        manifest: serde_json::from_str(
            manifest.unwrap_or("{\"private\":true,\"dependencies\":{}}"),
        )
        .map_err(io)?,
        entries: serde_json::from_str(entries.unwrap_or("[]")).map_err(io)?,
    };
    validate(&documents)?;
    Ok(documents)
}
fn primary(profile: &Path) -> Result<Documents, String> {
    decode(
        read_optional(&profile.join("package.json"))?.as_deref(),
        read_optional(&profile.join("plugins.json"))?.as_deref(),
    )
}

/// Malformed user documents are retained verbatim. Boot uses the last validated
/// snapshot, or the built-in UI alone when no good external snapshot exists.
pub fn read_runtime(profile: &Path) -> RuntimeDocuments {
    let loaded = if profile.join(JOURNAL).exists() {
        Err("plugin operation recovery is pending".to_string())
    } else {
        primary(profile)
    };
    match loaded {
        Ok(documents) => RuntimeDocuments {
            documents,
            issue: None,
        },
        Err(error) => {
            let backup = read_optional(&profile.join(GOOD))
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<Documents>(&raw).ok())
                .filter(|value| validate(value).is_ok());
            RuntimeDocuments {
                documents: backup.unwrap_or_else(|| Documents {
                    manifest: json!({"private":true,"dependencies":{}}),
                    entries: vec![],
                }),
                issue: Some(format!("插件配置无法载入，原文件保留：{error}")),
            }
        }
    }
}

pub fn package_path(profile: &Path, name: &str) -> Result<PathBuf, String> {
    let parts: Vec<_> = name.split('/').collect();
    if !(parts.len() == 1 || parts.len() == 2 && parts[0].starts_with('@'))
        || parts.iter().enumerate().any(|(index, part)| {
            let value = if index == 0 && parts.len() == 2 {
                part.strip_prefix('@').unwrap_or("")
            } else {
                part
            };
            matches!(value, "" | "." | "..")
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err("invalid plugin package name".into());
    }
    let mut path = profile.join("node_modules");
    if path.exists() {
        regular(&path, true)?;
    }
    for part in parts {
        path.push(part);
        if path.exists() {
            regular(&path, true)?;
        }
    }
    Ok(path)
}

/// Bounded package hashing also rejects links before copy, move or cleanup.
pub fn package_digest(root: &Path) -> Result<String, String> {
    fn walk(
        root: &Path,
        relative: &Path,
        digest: &mut Sha256,
        total: &mut u64,
        count: &mut usize,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("plugin package exceeds directory depth limit".into());
        }
        regular(&root.join(relative), true)?;
        let mut entries = std::fs::read_dir(root.join(relative))
            .map_err(io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
            *count += 1;
            if *count > 4096 {
                return Err("plugin package exceeds 4096 files/directories".into());
            }
            let path = entry.path();
            let kind = entry.file_type().map_err(io)?;
            let name = relative.join(entry.file_name());
            let encoded = name.to_string_lossy().replace('\\', "/");
            digest.update((encoded.len() as u64).to_le_bytes());
            digest.update(encoded.as_bytes());
            if kind.is_dir() {
                digest.update(b"d");
                walk(root, &name, digest, total, count, depth + 1)?;
            } else {
                regular(&path, false)?;
                digest.update(b"f");
                let size = std::fs::metadata(&path).map_err(io)?.len();
                *total = total.checked_add(size).ok_or("plugin size overflow")?;
                if *total > 64 * 1024 * 1024 {
                    return Err("plugin package exceeds 64 MiB".into());
                }
                digest.update(size.to_le_bytes());
                let mut file = File::open(path).map_err(io)?;
                let mut buffer = [0_u8; 32 * 1024];
                let mut observed = 0;
                loop {
                    let count = file.read(&mut buffer).map_err(io)?;
                    if count == 0 {
                        break;
                    }
                    observed += count as u64;
                    if observed > size {
                        return Err("plugin changed while hashing".into());
                    }
                    digest.update(&buffer[..count]);
                }
                if observed != size {
                    return Err("plugin changed while hashing".into());
                }
            }
        }
        Ok(())
    }
    let mut digest = Sha256::new();
    walk(root, Path::new(""), &mut digest, &mut 0, &mut 0, 0)?;
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u8,
    operation: String,
    committed: bool,
    package: Option<String>,
    old_package: Option<String>,
    new_package: Option<String>,
    before_manifest: Option<String>,
    before_entries: Option<String>,
    after_manifest: String,
    after_entries: String,
    #[serde(default)]
    tag: Option<OperationTag>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationTag {
    pub operation_id: String,
    pub action: String,
    pub spec: String,
}
pub struct Profile {
    root: PathBuf,
    _lock: File,
    tag: Option<OperationTag>,
    expected: parking_lot::Mutex<Option<[Option<String>; 2]>>,
}
fn stamp(manifest: &Option<String>, entries: &Option<String>) -> [Option<String>; 2] {
    [
        manifest
            .as_ref()
            .map(|text| format!("{:x}", Sha256::digest(text.as_bytes()))),
        entries
            .as_ref()
            .map(|text| format!("{:x}", Sha256::digest(text.as_bytes()))),
    ]
}
impl Profile {
    pub fn open(profile: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(profile).map_err(io)?;
        regular(profile, true)?;
        let root = profile.canonicalize().map_err(io)?;
        let path = root.join(".dsh-plugins.lock");
        if path.exists() {
            regular(&path, false)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(io)?;
        lock.try_lock()
            .map_err(|error| format!("另一项插件配置或安装操作正在执行：{error}"))?;
        let own = Self {
            root,
            _lock: lock,
            tag: None,
            expected: parking_lot::Mutex::new(None),
        };
        own.recover()?;
        own.cleanup_staging()?;
        Ok(own)
    }
    pub fn set_operation(&mut self, tag: OperationTag) -> Result<(), String> {
        if tag.operation_id.is_empty()
            || tag.operation_id.len() > 64
            || !tag
                .operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("invalid plugin operation identity".into());
        }
        self.tag = Some(tag);
        Ok(())
    }
    pub fn cleanup_staging(&self) -> Result<(), String> {
        for entry in std::fs::read_dir(&self.root).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".dsh-plugin-operation-") {
                self.discard_stage(&entry.path())?;
            } else if name.starts_with(".dsh-plugin-write-") {
                regular(&entry.path(), false)?;
                std::fs::remove_file(entry.path()).map_err(io)?;
            }
        }
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn documents(&self) -> Result<Documents, String> {
        let manifest = read_optional(&self.root.join("package.json"))?;
        let entries = read_optional(&self.root.join("plugins.json"))?;
        let documents = decode(manifest.as_deref(), entries.as_deref())?;
        *self.expected.lock() = Some(stamp(&manifest, &entries));
        Ok(documents)
    }
    pub fn checkpoint(&self) -> Result<(), String> {
        persist(&self.root.join(GOOD), &self.documents()?)
    }
    pub fn operation_dir(&self) -> Result<PathBuf, String> {
        let path = self.root.join(format!(".dsh-plugin-operation-{}", nonce()));
        std::fs::create_dir(&path).map_err(io)?;
        Ok(path)
    }
    fn owned_operation(&self, name: &str) -> Result<PathBuf, String> {
        if !name.starts_with(".dsh-plugin-operation-")
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        {
            return Err("invalid plugin recovery operation path".into());
        }
        let path = self.root.join(name);
        regular(&path, true)?;
        if path.canonicalize().map_err(io)?.parent() != Some(self.root.as_path()) {
            return Err("plugin operation escapes its profile".into());
        }
        Ok(path)
    }
    pub fn discard_stage(&self, path: &Path) -> Result<(), String> {
        let owned = self.owned_operation(
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid plugin stage name")?,
        )?;
        if owned != path {
            return Err("plugin stage belongs to another profile".into());
        }
        std::fs::remove_dir_all(owned).map_err(io)
    }
    pub fn replace(
        &self,
        documents: Documents,
        package: Option<(&str, Option<&Path>)>,
    ) -> Result<(), String> {
        self.commit(documents, package, false)
    }
    fn commit(
        &self,
        documents: Documents,
        package: Option<(&str, Option<&Path>)>,
        recover_invalid: bool,
    ) -> Result<(), String> {
        validate(&documents)?;
        if self.root.join(JOURNAL).exists() {
            return Err("插件操作仍需恢复，不能覆盖现有恢复记录".into());
        }
        let operation = self.operation_dir()?;
        let result = (|| {
            let before_manifest = read_optional(&self.root.join("package.json"))?;
            let before_entries = read_optional(&self.root.join("plugins.json"))?;
            if !recover_invalid {
                decode(before_manifest.as_deref(), before_entries.as_deref())?;
            }
            if self
                .expected
                .lock()
                .as_ref()
                .is_some_and(|expected| expected != &stamp(&before_manifest, &before_entries))
            {
                return Err("插件配置已被外部修改，原文件未覆盖；请重新读取后操作".into());
            }
            let after_manifest = serde_json::to_string_pretty(&documents.manifest).map_err(io)?;
            let after_entries = serde_json::to_string_pretty(&documents.entries).map_err(io)?;
            let (package, old_package, new_package) = match package {
                None => (None, None, None),
                Some((name, new)) => {
                    let target = package_path(&self.root, name)?;
                    let old = target
                        .exists()
                        .then(|| package_digest(&target))
                        .transpose()?;
                    let staged = if let Some(source) = new {
                        let hash = package_digest(source)?;
                        copy_package(source, &operation.join("next"))?;
                        if package_digest(&operation.join("next"))? != hash {
                            return Err("plugin staging changed during copy".into());
                        }
                        Some(hash)
                    } else {
                        None
                    };
                    (Some(name.to_string()), old, staged)
                }
            };
            let mut journal = Journal {
                version: 1,
                operation: operation
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                committed: false,
                package,
                old_package,
                new_package,
                before_manifest,
                before_entries,
                after_manifest,
                after_entries,
                tag: self.tag.clone(),
            };
            if read_optional(&self.root.join("package.json"))? != journal.before_manifest
                || read_optional(&self.root.join("plugins.json"))? != journal.before_entries
            {
                return Err("plugin configuration changed during preparation".into());
            }
            persist(&self.root.join(JOURNAL), &journal)?;
            if let Some(name) = &journal.package {
                let target = package_path(&self.root, name)?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(io)?;
                }
                if target
                    .exists()
                    .then(|| package_digest(&target))
                    .transpose()?
                    != journal.old_package
                {
                    return Err("plugin package changed during preparation".into());
                }
                if journal.old_package.is_some() {
                    std::fs::rename(&target, operation.join("previous")).map_err(io)?;
                }
                if journal.new_package.is_some() {
                    std::fs::rename(operation.join("next"), &target).map_err(io)?;
                }
            }
            atomic(
                &self.root.join("package.json"),
                journal.after_manifest.as_bytes(),
            )?;
            atomic(
                &self.root.join("plugins.json"),
                journal.after_entries.as_bytes(),
            )?;
            persist(&self.root.join(GOOD), &documents)?;
            journal.committed = true;
            persist(&self.root.join(JOURNAL), &journal)?;
            self.recover()?;
            *self.expected.lock() = Some(stamp(
                &Some(journal.after_manifest),
                &Some(journal.after_entries),
            ));
            Ok(())
        })();
        if result.is_err() && self.root.join(JOURNAL).exists() {
            if let Err(recovery) = self.recover() {
                return Err(format!(
                    "{}；恢复未完成，保留原件和操作记录：{recovery}",
                    result.unwrap_err()
                ));
            }
        }
        if operation.exists() && !self.root.join(JOURNAL).exists() {
            self.discard_stage(&operation)?;
        }
        result
    }
    pub fn restore_last_good(&self) -> Result<(), String> {
        let raw = read_optional(&self.root.join(GOOD))?.ok_or("没有已验证的插件配置快照")?;
        let documents: Documents = serde_json::from_str(&raw).map_err(io)?;
        validate(&documents)?;
        let retained = self.root.join(format!(".dsh-plugin-rejected-{}", nonce()));
        std::fs::create_dir(&retained).map_err(io)?;
        let current_manifest = read_optional(&self.root.join("package.json"))?;
        let current_entries = read_optional(&self.root.join("plugins.json"))?;
        *self.expected.lock() = Some(stamp(&current_manifest, &current_entries));
        for (name, raw) in [
            ("package.json", current_manifest),
            ("plugins.json", current_entries),
        ] {
            if let Some(raw) = raw {
                atomic(&retained.join(name), raw.as_bytes())?;
            }
        }
        // Explicit recovery retains the rejected documents as a separate artifact.
        self.commit(documents, None, true)
    }
    fn recover(&self) -> Result<(), String> {
        let Some(raw) = read_optional(&self.root.join(JOURNAL))? else {
            return Ok(());
        };
        let journal: Journal = serde_json::from_str(&raw).map_err(io)?;
        if journal.version != 1 {
            return Err("unsupported plugin recovery version".into());
        }
        let operation = self.owned_operation(&journal.operation)?;
        if !journal.committed {
            // Detect concurrent manual edits before restoring any component.
            for (name, before, after) in [
                (
                    "package.json",
                    &journal.before_manifest,
                    &journal.after_manifest,
                ),
                (
                    "plugins.json",
                    &journal.before_entries,
                    &journal.after_entries,
                ),
            ] {
                let current = read_optional(&self.root.join(name))?;
                if &current != before && current.as_deref() != Some(after) {
                    return Err(format!("插件恢复遇到外部配置修改，未覆盖：{name}"));
                }
            }
            if let Some(name) = &journal.package {
                let target = package_path(&self.root, name)?;
                let previous = operation.join("previous");
                let next = operation.join("next");
                let existing = target
                    .exists()
                    .then(|| package_digest(&target))
                    .transpose()?;
                if previous.exists() {
                    if Some(package_digest(&previous)?) != journal.old_package {
                        return Err("previous plugin content changed; recovery retained".into());
                    }
                    if existing.is_some() {
                        if existing != journal.new_package || next.exists() {
                            return Err("installed plugin changed during recovery; retained".into());
                        }
                        std::fs::rename(&target, operation.join("discarded")).map_err(io)?;
                    }
                    std::fs::rename(previous, &target).map_err(io)?;
                } else if journal.old_package.is_none() && existing.is_some() {
                    if existing != journal.new_package || next.exists() {
                        return Err("new plugin changed during recovery; retained".into());
                    }
                    std::fs::rename(&target, operation.join("discarded")).map_err(io)?;
                } else if existing != journal.old_package {
                    return Err("original plugin unavailable during recovery; retained".into());
                }
            }
            for (name, before) in [
                ("package.json", &journal.before_manifest),
                ("plugins.json", &journal.before_entries),
            ] {
                let path = self.root.join(name);
                if let Some(before) = before {
                    atomic(&path, before.as_bytes())?;
                } else if path.exists() {
                    regular(&path, false)?;
                    std::fs::remove_file(path).map_err(io)?;
                }
            }
            if let Ok(before) = decode(
                journal.before_manifest.as_deref(),
                journal.before_entries.as_deref(),
            ) {
                persist(&self.root.join(GOOD), &before)?;
            }
        }
        if journal.committed
            && let Some(tag) = &journal.tag
        {
            persist(
                &self.root.join(".dsh-plugin-last-operation.json"),
                &json!({"operationId":tag.operation_id,"action":tag.action,"spec":tag.spec,"committed":true,"package":journal.package}),
            )?;
        }
        // The journal is cleared only after recovery or the committed state is
        // complete; stage cleanup is then safe to retry independently.
        std::fs::remove_file(self.root.join(JOURNAL)).map_err(io)?;
        self.discard_stage(&operation)
    }
}

#[cfg(test)]
#[path = "plugin_profile_tests.rs"]
mod tests;

pub fn copy_package(source: &Path, destination: &Path) -> Result<(), String> {
    package_digest(source)?;
    fn copy(source: &Path, destination: &Path) -> Result<(), String> {
        regular(source, true)?;
        std::fs::create_dir_all(destination).map_err(io)?;
        for entry in std::fs::read_dir(source).map_err(io)? {
            let entry = entry.map_err(io)?;
            if entry.file_name() == ".git" {
                continue;
            }
            let kind = entry.file_type().map_err(io)?;
            let target = destination.join(entry.file_name());
            if kind.is_dir() {
                copy(&entry.path(), &target)?;
            } else {
                regular(&entry.path(), false)?;
                std::fs::copy(entry.path(), target).map_err(io)?;
            }
        }
        Ok(())
    }
    copy(source, destination)
}
