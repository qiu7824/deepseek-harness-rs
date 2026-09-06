//! Resource directories are created and registered together. Collection never
//! adopts files from a parent directory, and process leases survive Host crashes
//! through operating-system locks rather than a stale in-memory counter.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const DAY: u64 = 86_400;
const FORMAT: &str = "dsh-resource-v1";
type Result<T> = std::result::Result<T, String>;
mod execution;
mod worktree_meta;
pub use execution::ExecutionResources;

pub struct ExecutionWorkspace {
    pub root: String,
    pub project: String,
    pub read_only_roots: Vec<String>,
}
pub trait ManagedWorkspaces: Send + Sync {
    fn resolve(
        &self,
        owner: &str,
        workdir: &str,
    ) -> std::result::Result<Option<ExecutionWorkspace>, String>;
}
impl cordis::Service for dyn ManagedWorkspaces {
    fn service_name(&self) -> &'static str {
        "managedWorkspaces"
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Policy {
    pub enabled: bool,
    pub auto_clean: bool,
    pub reduce_context: bool,
    pub keep_days: u64,
    pub failed_days: u64,
    pub recovery_days: u64,
    pub soft_limit_gib: u64,
    pub location: String,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_clean: true,
            reduce_context: true,
            keep_days: 7,
            failed_days: 14,
            recovery_days: 3,
            soft_limit_gib: 20,
            location: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub format: String,
    pub id: String,
    pub owner: String,
    pub project: String,
    pub kind: String,
    pub label: String,
    pub state: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub pinned: bool,
    pub error: Option<String>,
    pub bytes: u64,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub origin: Option<serde_json::Value>,
    #[serde(default = "size_unknown")]
    pub size_pending: bool,
    #[serde(default)]
    pub process_id: Option<u32>,
}
fn size_unknown() -> bool {
    true
}

fn process_present(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError},
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        let handle = unsafe { OpenProcess(0x00100000, 0, pid) };
        if handle.is_null() {
            return unsafe { GetLastError() } != 87;
        }
        let status = unsafe { WaitForSingleObject(handle, 0) };
        unsafe { CloseHandle(handle) };
        status != 0
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, 0) == 0
                || libc::kill(-(pid as i32), 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

pub struct Store {
    root: PathBuf,
    gate: Mutex<()>,
}

pub struct Lease {
    store: Arc<Store>,
    id: String,
    file: Option<File>,
    finished: bool,
}
impl Lease {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn path(&self) -> PathBuf {
        self.store.content_object(&self.id).join("data")
    }
    pub fn attach_process(&self, pid: u32) -> Result<()> {
        self.store.edit(&self.id, |row| {
            row.process_id = Some(pid);
            Ok(())
        })
    }
    pub fn finish(&mut self, success: bool) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.store.edit(&self.id, |row| {
            row.state = if success { "retained" } else { "failed" }.into();
            row.process_id = None;
            row.updated_at = now();
            Ok(())
        })?;
        self.finished = true;
        self.file.take();
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.store.edit(&self.id, |row| {
                row.state = "interrupted".into();
                row.updated_at = now();
                Ok(())
            });
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_system_alias(path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let expected = match path.to_str() {
        Some("/var") => Path::new("/private/var"),
        Some("/tmp") => Path::new("/private/tmp"),
        Some("/etc") => Path::new("/private/etc"),
        _ => return false,
    };
    metadata.uid() == 0
        && fs::read_link(path).is_ok_and(|target| Path::new("/").join(target) == expected)
}

/// Refuse user links and reparse points, including existing ancestors.
pub fn checked_path(path: &Path) -> Result<()> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                let linked = meta.file_type().is_symlink();
                // macOS exposes the system temporary directory through /var.
                // Only its root-owned, exact system aliases are trusted; links
                // within a managed directory still fail the same checks.
                #[cfg(target_os = "macos")]
                if linked && macos_system_alias(ancestor, &meta) {
                    continue;
                }
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    linked || meta.file_attributes() & 0x400 != 0
                };
                if linked {
                    return Err(format!("受管路径不能经过链接：{}", ancestor.display()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    checked_path(path)?;
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    checked_path(path)
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    checked_path(path)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let outcome = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}
impl Policy {
    pub fn from_json(mut value: serde_json::Value) -> Result<Self> {
        for key in ["keepDays", "failedDays", "recoveryDays", "softLimitGib"] {
            if let Some(number) = value.get(key) {
                let number = number
                    .as_f64()
                    .filter(|number| {
                        number.is_finite()
                            && *number >= 1.0
                            && number.fract() == 0.0
                            && *number <= 4096.0
                    })
                    .ok_or_else(|| format!("{key} 必须为范围内的整数"))?;
                value[key] = serde_json::json!(number as u64);
            }
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

pub fn persist_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    atomic(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}

fn lock_file(path: &Path) -> Result<File> {
    checked_path(path)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())
}

fn read_record(path: &Path) -> Result<Resource> {
    checked_path(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("资源记录过大".into());
    }
    let row: Resource = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if row.format != FORMAT || !valid_id(&row.id) {
        return Err("资源归属记录无效".into());
    }
    Ok(row)
}
fn valid_id(id: &str) -> bool {
    (id.len() == 36 || id.len() == 64) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn tree_bytes(path: &Path) -> Result<u64> {
    checked_path(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut bytes = 0u64;
    let mut pending = vec![path.to_path_buf()];
    let mut seen = 0;
    while let Some(directory) = pending.pop() {
        checked_path(&directory)?;
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            checked_path(&path)?;
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.is_dir() {
                pending.push(path);
            } else {
                bytes = bytes.saturating_add(meta.len());
            }
            seen += 1;
            if seen > 1_000_000 {
                return Err("资源文件数量过多，保留待检查".into());
            }
        }
    }
    Ok(bytes)
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Arc<Self>> {
        let root = std::path::absolute(root.as_ref()).map_err(|e| e.to_string())?;
        private_dir(&root)?;
        let marker = root.join(".dsh-resources");
        checked_path(&marker)?;
        if marker.exists() {
            if fs::read_to_string(&marker).map_err(|e| e.to_string())? != FORMAT {
                return Err("存储位置已有未知归属记录".into());
            }
        } else {
            if fs::read_dir(&root)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            {
                return Err("垃圾槽必须使用新的空目录或已有受管目录".into());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
                .map_err(|e| e.to_string())?;
            file.write_all(FORMAT.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        private_dir(&root.join("objects"))?;
        private_dir(&root.join("content"))?;
        private_dir(&root.join("vault"))?;
        private_dir(&root.join("recovery"))?;
        Ok(Arc::new(Self {
            root,
            gate: Mutex::new(()),
        }))
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn object(&self, id: &str) -> PathBuf {
        self.root.join("objects").join(id)
    }
    fn content_object(&self, id: &str) -> PathBuf {
        let vault = read_record(&self.object(id).join("record.json"))
            .is_ok_and(|row| ["log", "trash"].contains(&row.kind.as_str()));
        self.root
            .join(if vault { "vault" } else { "content" })
            .join(id)
    }
    fn recovery_directory(&self, id: impl AsRef<str>) -> PathBuf {
        self.root.join("recovery").join(id.as_ref())
    }
    pub fn writable_root(&self) -> PathBuf {
        self.root.join("content")
    }
    fn verify(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err("资源 ID 无效".into());
        }
        let object = self.object(id);
        checked_path(&object)?;
        let record = read_record(&object.join("record.json"))?;
        if record.id != id {
            return Err("资源归属不匹配".into());
        }
        Ok(object)
    }
    fn write_record(&self, row: &Resource) -> Result<()> {
        atomic(
            &self.object(&row.id).join("record.json"),
            &serde_json::to_vec_pretty(row).map_err(|e| e.to_string())?,
        )
    }
    fn edit<T>(&self, id: &str, operation: impl FnOnce(&mut Resource) -> Result<T>) -> Result<T> {
        let _gate = self.gate.lock();
        self.edit_locked(id, operation)
    }
    fn edit_locked<T>(
        &self,
        id: &str,
        operation: impl FnOnce(&mut Resource) -> Result<T>,
    ) -> Result<T> {
        let object = self.verify(id)?;
        let content = self.content_object(id);
        checked_path(&content)?;
        let lock = lock_file(&object.join("record.lock"))?;
        lock.lock().map_err(|e| e.to_string())?;
        let mut row = read_record(&object.join("record.json"))?;
        let result = operation(&mut row)?;
        self.write_record(&row)?;
        Ok(result)
    }
    pub fn allocate(
        self: &Arc<Self>,
        owner: &str,
        project: &str,
        kind: &str,
        label: &str,
    ) -> Result<Lease> {
        self.allocate_id(
            owner,
            project,
            kind,
            label,
            &uuid::Uuid::new_v4().to_string(),
        )
    }
    pub fn cache(self: &Arc<Self>, project: &str, key: &str) -> Result<Lease> {
        let id = digest(format!("{project}\0{key}").as_bytes());
        self.allocate_id("shared", project, "cache", "构建缓存", &id)
    }
    fn allocate_id(
        self: &Arc<Self>,
        owner: &str,
        project: &str,
        kind: &str,
        label: &str,
        id: &str,
    ) -> Result<Lease> {
        if owner.is_empty()
            || owner.len() > 256
            || project.len() > 8192
            || label.len() > 1024
            || ![
                "run",
                "cache",
                "log",
                "script",
                "candidate",
                "copy",
                "trash",
            ]
            .contains(&kind)
        {
            return Err("资源登记参数无效".into());
        }
        let _gate = self.gate.lock();
        let registry = lock_file(&self.root.join("registry.lock"))?;
        registry.lock().map_err(|e| e.to_string())?;
        let object = self.object(id);
        private_dir(&object)?;
        let content = self
            .root
            .join(if ["log", "trash"].contains(&kind) {
                "vault"
            } else {
                "content"
            })
            .join(id);
        private_dir(&content)?;
        let lease = lock_file(&object.join("lease"))?;
        lease.lock_shared().map_err(|e| e.to_string())?;
        let record_lock = lock_file(&object.join("record.lock"))?;
        record_lock.lock().map_err(|e| e.to_string())?;
        let record = object.join("record.json");
        let mut row = if record.exists() {
            let row = read_record(&record)?;
            if row.id != id || row.kind != kind || row.project != project {
                return Err("资源标识冲突".into());
            }
            row
        } else {
            Resource {
                format: FORMAT.into(),
                id: id.into(),
                owner: owner.into(),
                project: project.into(),
                kind: kind.into(),
                label: label.into(),
                state: "active".into(),
                created_at: now(),
                updated_at: now(),
                pinned: false,
                error: None,
                bytes: 0,
                busy: false,
                path: String::new(),
                origin: None,
                size_pending: true,
                process_id: None,
            }
        };
        if self.recovery_directory(&id).exists() {
            if kind != "cache" || content.join("data").exists() {
                return Err("资源正在恢复队列中，请先恢复".into());
            }
            checked_path(&self.recovery_directory(&id))?;
            fs::rename(self.recovery_directory(&id), content.join("data"))
                .map_err(|e| e.to_string())?;
        }
        private_dir(&content.join("data"))?;
        row.state = "active".into();
        row.updated_at = now();
        row.error = None;
        row.size_pending = true;
        self.write_record(&row)?;
        Ok(Lease {
            store: self.clone(),
            id: id.into(),
            file: Some(lease),
            finished: false,
        })
    }
    pub fn path(&self, id: &str, relative: &str) -> Result<PathBuf> {
        let object = self.verify(id)?;
        let content = self.content_object(id);
        checked_path(&content)?;
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.to_string_lossy().contains([':', '\0'])
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("资源内路径无效".into());
        }
        let row = read_record(&object.join("record.json"))?;
        let base = if row.state == "quarantined" {
            self.recovery_directory(id)
        } else {
            content.join("data")
        };
        let path = base.join(relative);
        checked_path(&path)?;
        Ok(path)
    }
    pub fn get(&self, id: &str) -> Result<Resource> {
        self.verify(id)?;
        read_record(&self.object(id).join("record.json"))
    }
    pub fn set_origin(&self, id: &str, origin: serde_json::Value) -> Result<()> {
        self.edit(id, |row| {
            row.origin = Some(origin);
            row.updated_at = now();
            Ok(())
        })
    }
    pub fn retain(&self, id: &str, pinned: bool, candidate: bool) -> Result<()> {
        self.edit(id, |row| {
            if row.state == "quarantined" {
                return Err("请先恢复资源".into());
            }
            row.pinned = pinned;
            row.state = if candidate { "candidate" } else { "retained" }.into();
            row.updated_at = now();
            Ok(())
        })
    }
    pub fn list(&self) -> Result<Vec<Resource>> {
        self.list_with_sizes(true)
    }
    pub fn list_brief(&self) -> Result<Vec<Resource>> {
        self.list_with_sizes(false)
    }
    pub fn refresh_sizes(&self) -> Result<()> {
        for measured in self.list()? {
            self.edit(&measured.id, |row| {
                if row.updated_at == measured.updated_at {
                    row.bytes = measured.bytes;
                    row.size_pending = measured.error.is_some();
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    fn list_with_sizes(&self, measure: bool) -> Result<Vec<Resource>> {
        checked_path(&self.root.join("objects"))?;
        let mut rows = Vec::new();
        for directory in fs::read_dir(self.root.join("objects")).map_err(|e| e.to_string())? {
            let directory = directory.map_err(|e| e.to_string())?;
            let id = directory.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            let Ok(object) = self.verify(&id) else {
                continue;
            };
            let content = self.content_object(&id);
            if checked_path(&content).is_err() {
                continue;
            }
            let mut row = read_record(&object.join("record.json"))?;
            if row.state == "reclaimed" {
                row.bytes = 0;
                row.path = String::new();
                rows.push(row);
                continue;
            }
            let lease = lock_file(&object.join("lease"))?;
            row.busy = lease.try_lock().is_err() || row.process_id.is_some_and(process_present);
            drop(lease);
            if !row.busy && row.state == "active" {
                row.state = "interrupted".into();
            }
            if self.recovery_directory(&id).exists() && !content.join("data").exists() {
                row.state = "quarantined".into();
            }
            let data = if row.state == "quarantined" {
                self.recovery_directory(&id)
            } else {
                content.join("data")
            };
            row.path = data.to_string_lossy().into_owned();
            if measure {
                match tree_bytes(&data) {
                    Ok(bytes) => {
                        row.bytes = bytes;
                        row.size_pending = false
                    }
                    Err(error) => row.error = Some(error),
                }
            }
            rows.push(row);
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        Ok(rows)
    }
    pub fn quarantine(&self, id: &str) -> Result<()> {
        let _gate = self.gate.lock();
        let object = self.verify(id)?;
        let content = self.content_object(id);
        checked_path(&content)?;
        let lease = lock_file(&object.join("lease"))?;
        lease
            .try_lock()
            .map_err(|_| "资源仍被运行中的进程使用".to_string())?;
        self.edit_locked(id, |row| {
            if row.process_id.is_some_and(process_present) {
                return Err("资源仍被进程使用".into());
            }
            if row.kind == "run" && row.state == "active" && row.process_id.is_none() {
                return Err("运行归属尚未恢复，请先核对并标记完成".into());
            }
            if row.pinned || row.state == "candidate" {
                return Err("固定项或待交付产物不能清理".into());
            }
            if row.state == "quarantined" {
                return Ok(());
            }
            tree_bytes(&content.join("data"))?;
            if self.recovery_directory(&id).exists() {
                return Err("已有恢复事务，保留原数据".into());
            }
            worktree_meta::move_payload(row, &content.join("data"), &self.recovery_directory(&id))?;
            row.state = "quarantined".into();
            row.updated_at = now();
            row.error = None;
            Ok(())
        })
    }
    pub fn restore(&self, id: &str) -> Result<()> {
        let _gate = self.gate.lock();
        let object = self.verify(id)?;
        let content = self.content_object(id);
        checked_path(&content)?;
        let lease = lock_file(&object.join("lease"))?;
        lease.try_lock().map_err(|_| "资源仍在使用".to_string())?;
        self.edit_locked(id, |row| {
            if !self.recovery_directory(&id).exists() {
                return Err("资源不在恢复队列中".into());
            }
            if content.join("data").exists() {
                return Err("目标已存在，不能覆盖恢复".into());
            }
            tree_bytes(&self.recovery_directory(&id))?;
            worktree_meta::move_payload(row, &self.recovery_directory(&id), &content.join("data"))?;
            row.state = "retained".into();
            row.updated_at = now();
            row.error = None;
            Ok(())
        })
    }
    pub fn collect(&self, policy: &Policy, at: u64, manual: bool) -> Result<Vec<String>> {
        self.collect_protected(policy, at, manual, &BTreeSet::new())
    }
    pub fn collect_protected(
        &self,
        policy: &Policy,
        at: u64,
        manual: bool,
        owners: &BTreeSet<String>,
    ) -> Result<Vec<String>> {
        let rows = self.list()?;
        let busy_projects = rows
            .iter()
            .filter(|row| row.busy && row.kind == "run")
            .map(|row| row.project.as_str())
            .collect::<BTreeSet<_>>();
        let mut total = rows.iter().map(|row| row.bytes).sum::<u64>();
        let limit = policy.soft_limit_gib.saturating_mul(1024 * 1024 * 1024);
        let mut cleaned = Vec::new();
        for row in rows.iter().rev() {
            if row.busy
                || row.pinned
                || row.state == "candidate"
                || row.state == "reclaimed"
                || owners.contains(&row.owner)
                || row.kind == "cache" && busy_projects.contains(row.project.as_str())
            {
                continue;
            }
            let age = at.saturating_sub(row.updated_at);
            if row.kind == "run" && row.process_id.is_none() && self.get(&row.id)?.state == "active"
            {
                continue;
            }
            if row.state == "quarantined" {
                if age >= policy.recovery_days.max(1) * DAY {
                    let _gate = self.gate.lock();
                    let object = self.verify(&row.id)?;
                    let content = self.content_object(&row.id);
                    checked_path(&content)?;
                    let lease = lock_file(&object.join("lease"))?;
                    if lease.try_lock().is_err() {
                        continue;
                    }
                    let result = self.edit_locked(&row.id, |record| {
                        if record.pinned || record.state == "candidate" {
                            return Err("资源已被保护".into());
                        }
                        worktree_meta::purge(record, &self.recovery_directory(&row.id))?;
                        record.state = "reclaimed".into();
                        record.bytes = 0;
                        record.updated_at = at;
                        Ok(())
                    });
                    if result.is_ok() {
                        total = total.saturating_sub(row.bytes);
                        cleaned.push(row.id.clone());
                    } else if let Err(error) = result {
                        let _ = self.edit_locked(&row.id, |record| {
                            record.error = Some(error);
                            Ok(())
                        });
                    }
                }
                continue;
            }
            let days = if ["failed", "interrupted"].contains(&row.state.as_str()) {
                policy.failed_days
            } else {
                policy.keep_days
            };
            if manual && !["failed", "interrupted"].contains(&row.state.as_str())
                || age >= days.max(1) * DAY
                || row.kind == "cache" && total > limit
            {
                match self.quarantine(&row.id) {
                    Ok(()) => cleaned.push(row.id.clone()),
                    Err(error) => {
                        let _ = self.edit(&row.id, |record| {
                            record.error = Some(error);
                            Ok(())
                        });
                    }
                }
            }
        }
        Ok(cleaned)
    }
    pub fn write_text(&self, id: &str, relative: &str, content: &str) -> Result<PathBuf> {
        if content.len() > 16 * 1024 * 1024 {
            return Err("单个文本资源超过 16 MiB".into());
        }
        let object = self.verify(id)?;
        let lease = lock_file(&object.join("lease"))?;
        lease.lock_shared().map_err(|e| e.to_string())?;
        let path = self.path(id, relative)?;
        let row = self.get(id)?;
        if row.state == "quarantined" || row.kind == "trash" || row.kind == "log" && path.exists() {
            return Err("恢复材料和已保存的完整工具输出为只读".into());
        }
        private_dir(path.parent().ok_or("缺少父目录")?)?;
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
            && !content.starts_with('\u{feff}')
        {
            atomic(&path, format!("\u{feff}{content}").as_bytes())?;
        } else {
            atomic(&path, content.as_bytes())?;
        }
        self.edit(id, |row| {
            row.updated_at = now();
            Ok(())
        })?;
        Ok(path)
    }
    pub fn read_text(
        &self,
        id: &str,
        relative: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String> {
        let object = self.verify(id)?;
        let content = self.content_object(id);
        checked_path(&content)?;
        let lease = lock_file(&object.join("lease"))?;
        lease.lock_shared().map_err(|e| e.to_string())?;
        let path = self.path(id, relative)?;
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(|e| e.to_string())?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("请使用文件读取工具按范围读取大型资源".into());
        }
        let text = String::from_utf8(bytes).map_err(|_| "资源不是 UTF-8 文本".to_string())?;
        let text = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
        {
            text.trim_start_matches('\u{feff}')
        } else {
            &text
        };
        Ok(text.chars().skip(offset).take(limit.min(32_000)).collect())
    }
    pub fn files(&self, id: &str, relative: &str) -> Result<serde_json::Value> {
        let object = self.verify(id)?;
        let lease = lock_file(&object.join("lease"))?;
        lease.lock_shared().map_err(|e| e.to_string())?;
        let directory = if relative.is_empty() {
            self.path(id, "__entry__")?.parent().unwrap().to_path_buf()
        } else {
            self.path(id, relative)?
        };
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if checked_path(&entry.path()).is_err() {
                continue;
            }
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            entries.push(serde_json::json!({"name":name,"path":path,"kind":if meta.is_dir(){"directory"}else{"file"},"size":meta.len()}));
            if entries.len() >= 500 {
                truncated = true;
                break;
            }
        }
        entries.sort_by_key(|entry| entry["name"].as_str().unwrap_or_default().to_lowercase());
        Ok(serde_json::json!({"entries":entries,"truncated":truncated}))
    }
    pub fn owners(&self) -> Result<BTreeSet<String>> {
        Ok(self.list()?.into_iter().map(|row| row.owner).collect())
    }
}

#[cfg(test)]
mod tests;
