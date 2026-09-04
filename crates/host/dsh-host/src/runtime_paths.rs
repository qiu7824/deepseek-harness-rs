//! Restart-bound storage relocation with verified copies and retained source data.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const LOCK: &str = ".runtime.lock";
const REDIRECT: &str = ".dsh-home-redirect.json";
const ACTIVE: &str = ".runtime-paths.json";
const FAILURE: &str = ".runtime-migration-failed.json";

#[path = "runtime_paths_migration.rs"]
mod migration;

#[derive(Default)]
struct Admission {
    restarting: bool,
    requests: usize,
}

struct AdmissionRollback<'a>(&'a RuntimePaths, bool);
impl Drop for AdmissionRollback<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.admission.lock().restarting = false;
        }
    }
}

pub struct ApiRequestGuard(Arc<RuntimePaths>);
impl Drop for ApiRequestGuard {
    fn drop(&mut self) {
        let mut admission = self.0.admission.lock();
        admission.requests = admission.requests.saturating_sub(1);
        self.0.requests_changed.notify_waiters();
    }
}

pub struct RuntimePaths {
    pub paths: BTreeMap<String, PathBuf>,
    lock: parking_lot::Mutex<Option<Vec<File>>>,
    restart: tokio::sync::Notify,
    restart_supported: AtomicBool,
    instance_id: String,
    migration_error: Option<String>,
    restart_error: parking_lot::Mutex<Option<String>>,
    admission: parking_lot::Mutex<Admission>,
    requests_changed: tokio::sync::Notify,
}

fn read_json(path: &Path) -> Result<Value, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn canonical_target(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("目录必须为不含 .. 的绝对路径".into());
    }
    let mut ancestor = path.to_path_buf();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(ancestor.file_name().ok_or("目录无效")?.to_os_string());
        ancestor = ancestor.parent().ok_or("目录无效")?.to_path_buf();
    }
    let mut result = fs::canonicalize(&ancestor).map_err(|e| e.to_string())?;
    if !result.is_dir() {
        return Err("目录的父级不是文件夹".into());
    }
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    if result.parent().is_none() {
        return Err("不能使用磁盘根目录".into());
    }
    Ok(result)
}

pub fn resolve_redirect(root: &Path) -> Result<PathBuf, String> {
    let mut root = canonical_target(root)?;
    let mut visited = std::collections::HashSet::new();
    for _ in 0..16 {
        if !visited.insert(root.clone()) {
            return Err("数据目录重定向形成循环".into());
        }
        let marker = read_json(&root.join(REDIRECT))?;
        let Some(target) = marker.get("target").and_then(Value::as_str) else {
            return Ok(root);
        };
        let next = canonical_target(Path::new(target))?;
        if !next.join("settings.json").is_file() {
            return Err("迁移后的数据目录不可用，原数据仍保留".into());
        }
        root = next;
    }
    Err("数据目录重定向层数过多".into())
}

fn hash(path: &Path) -> Result<Vec<u8>, String> {
    let mut input = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    loop {
        let n = input.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&block[..n]);
    }
    Ok(hash.finalize().to_vec())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if [LOCK, REDIRECT]
            .iter()
            .any(|name| entry.file_name() == *name)
        {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!("迁移目录含符号链接，请先处理：{}", from.display()));
        }
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
            OpenOptions::new()
                .write(true)
                .open(&to)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            if hash(&from)? != hash(&to)? {
                return Err(format!("文件校验失败：{}", from.display()));
            }
        } else {
            return Err(format!("迁移目录含特殊文件：{}", from.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
fn copy_to_empty(source: &Path, target: &Path) -> Result<(), String> {
    if source == target {
        return Ok(());
    }
    if source.starts_with(target) || target.starts_with(source) {
        return Err("源目录与目标目录不能互相包含".into());
    }
    if target.exists()
        && fs::read_dir(target)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("目标目录必须为空：{}", target.display()));
    }
    let parent = target.parent().ok_or("目标目录无父级")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stage = parent.join(format!(".dsh-migration-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    if source.exists() {
        copy_tree(source, &stage)?;
    }
    if target.exists() {
        fs::remove_dir(target).map_err(|e| e.to_string())?;
    }
    fs::rename(&stage, target).map_err(|e| e.to_string())
}

fn defaults(root: &Path) -> BTreeMap<String, PathBuf> {
    [
        ("dataDirectory", root.to_path_buf()),
        ("cacheDirectory", root.join("cache")),
        ("environmentDirectory", root.join("environments")),
        ("testDirectory", root.join("test-runs")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

pub fn validate(
    value: &dsh_schemastery::Data,
    active: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let value = value.to_json().ok_or("目录设置必须为对象")?;
    for name in active.keys() {
        if value.get(name).and_then(Value::as_str).is_none() {
            return Err(format!("缺少 {name}"));
        }
    }
    migration::validate_targets(&migration::desired_paths(&value, active)?, active)
}

impl RuntimePaths {
    pub fn prepare(root: &Path) -> Result<Arc<Self>, String> {
        let root = resolve_redirect(root)?;
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut lock = migration::acquire_lock(&root)?;
        let mut settings = read_json(&root.join("settings.json"))?;
        let mut active = defaults(&root);
        let previous = read_json(&root.join(ACTIVE))?;
        for name in ["cacheDirectory", "environmentDirectory", "testDirectory"] {
            if let Some(path) = previous.get(name).and_then(Value::as_str) {
                active.insert(name.into(), canonical_target(Path::new(path))?);
            }
        }
        let attempt =
            migration::desired_paths(&settings["storage-paths"], &active).and_then(|paths| {
                migration::migrate(&active, &paths, &settings).map(|lock| (paths, lock))
            });
        let (paths, mut migration_error) = match attempt {
            Ok((paths, new_lock)) => {
                if let Some(new_lock) = new_lock {
                    lock = new_lock;
                }
                (paths, None)
            }
            Err(error) => {
                let _ = write_json(
                    &root.join(FAILURE),
                    &json!({"requested":settings.get("storage-paths"),"error":error}),
                );
                settings["storage-paths"] = json!(active);
                write_json(&root.join("settings.json"), &settings)
                    .map_err(|restore| format!("{error}；恢复原目录设置失败：{restore}"))?;
                for path in active.values() {
                    fs::create_dir_all(path).map_err(|e| e.to_string())?;
                }
                write_json(&root.join(ACTIVE), &json!(active))?;
                (active, Some(error))
            }
        };
        // Preserve a legacy derived search cache using a verified temporary file.
        // Failure does not prevent opening the retained session data.
        let legacy_search = paths["dataDirectory"].join("search.db");
        let search = paths["cacheDirectory"].join("search.db");
        if legacy_search.is_file() && !search.exists() {
            let temporary = search.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let copied = (|| -> Result<(), String> {
                fs::copy(&legacy_search, &temporary).map_err(|e| e.to_string())?;
                if hash(&legacy_search)? != hash(&temporary)? {
                    return Err("搜索缓存迁移校验失败".into());
                }
                fs::rename(&temporary, &search).map_err(|e| e.to_string())
            })();
            if let Err(error) = copied {
                let _ = fs::remove_file(&temporary);
                migration_error.get_or_insert(error);
            }
        }
        Ok(Arc::new(Self {
            paths,
            lock: parking_lot::Mutex::new(Some(lock)),
            restart: tokio::sync::Notify::new(),
            restart_supported: AtomicBool::new(false),
            instance_id: uuid::Uuid::new_v4().to_string(),
            migration_error,
            restart_error: parking_lot::Mutex::new(None),
            admission: parking_lot::Mutex::new(Admission::default()),
            requests_changed: tokio::sync::Notify::new(),
        }))
    }

    pub fn begin_api_request(self: &Arc<Self>) -> Option<ApiRequestGuard> {
        let mut admission = self.admission.lock();
        if admission.restarting {
            return None;
        }
        admission.requests += 1;
        Some(ApiRequestGuard(self.clone()))
    }

    async fn quiesce(&self) -> Result<(), String> {
        {
            let mut admission = self.admission.lock();
            if admission.restarting {
                return Err("服务已在重启中".into());
            }
            admission.restarting = true;
        }
        let mut rollback = AdmissionRollback(self, true);
        *self.restart_error.lock() = None;
        let drain = async {
            loop {
                let changed = self.requests_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.admission.lock().requests == 0 {
                    break;
                }
                changed.await;
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(5), drain)
            .await
            .is_err()
        {
            return Err("仍有请求正在处理，请稍后重试".into());
        }
        rollback.1 = false;
        Ok(())
    }

    pub fn release(&self) {
        self.lock.lock().take();
    }
    pub fn enable_restart(&self) {
        self.restart_supported.store(true, Ordering::Release);
    }
    pub async fn wait_restart(&self) {
        self.restart.notified().await;
    }
    pub fn node_command(&self) -> String {
        let name = if cfg!(windows) { "node.exe" } else { "node" };
        [
            self.paths["environmentDirectory"].join(name),
            self.paths["environmentDirectory"].join("bin").join(name),
        ]
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "node".into())
    }

    pub fn register(
        self: &Arc<Self>,
        server: &Arc<WebServer>,
        agents: Arc<dsh_agent::AgentRegistry>,
        allow_remote: bool,
    ) -> RouteDisposer {
        let runtime = self.clone();
        server.register(WebRoute { kind: WebRouteKind::Exact, path: "/__dsh-runtime".into(), handler: Arc::new(move |request| {
            let runtime = runtime.clone(); let agents = agents.clone();
            Box::pin(async move {
                let mut status = http::StatusCode::OK;
                let payload = if !super::trusted_web_request(&request, allow_remote) {
                    status = http::StatusCode::FORBIDDEN; json!({"error":"禁止跨站访问"})
                } else if request.method() == http::Method::GET {
                    json!({"paths":runtime.paths,"nodeCommand":runtime.node_command(),"restartSupported":runtime.restart_supported.load(Ordering::Acquire),"instanceId":runtime.instance_id,"migrationError":runtime.migration_error,"restarting":runtime.admission.lock().restarting,"restartError":runtime.restart_error.lock().clone()})
                } else if request.method() == http::Method::POST && request.headers().get("content-type").and_then(|h| h.to_str().ok()).is_some_and(|s| s.starts_with("application/json")) {
                    let bytes = axum::body::to_bytes(axum::body::Body::new(request.into_body()), 4096).await.unwrap_or_default();
                    let action: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    if action.get("action").and_then(Value::as_str) != Some("restart") {
                        status = http::StatusCode::BAD_REQUEST; json!({"error":"无效操作"})
                    } else if !runtime.restart_supported.load(Ordering::Acquire) {
                        status = http::StatusCode::CONFLICT; json!({"error":"当前入口不支持自动重启，请使用启动器重启"})
                    } else if let Err(error) = runtime.quiesce().await {
                        status = http::StatusCode::CONFLICT; json!({"error":error})
                    } else if agents.list().iter().any(|a| a.status() != dsh_agent::AgentStatus::Idle || a.inbox().has_pending()) {
                        runtime.admission.lock().restarting = false;
                        status = http::StatusCode::CONFLICT; json!({"error":"请等待运行中的任务完成后重启"})
                    } else {
                        let restart = runtime.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                            if agents.list().iter().any(|a| a.status() != dsh_agent::AgentStatus::Idle || a.inbox().has_pending()) {
                                *restart.restart_error.lock() = Some("有任务开始运行，已取消重启；请等待任务完成后重试".into());
                                restart.admission.lock().restarting = false;
                            } else { restart.restart.notify_one(); }
                        });
                        json!({"restarting":true,"instanceId":runtime.instance_id})
                    }
                } else { status = http::StatusCode::METHOD_NOT_ALLOWED; json!({"error":"不支持的请求"}) };
                Ok(http::Response::builder().status(status).header("content-type","application/json").header("cache-control","no-store")
                    .body(axum::body::Body::from(payload.to_string())).expect("runtime response"))
            })
        }) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("dsh-migrate-{}", uuid::Uuid::new_v4()))
    }
    #[test]
    fn migration_preserves_original_and_redirects_next_boot() {
        let source = temp();
        let target = temp();
        fs::create_dir_all(source.join("sessions")).unwrap();
        fs::write(source.join("sessions/test.jsonl"), "durable session\n").unwrap();
        write_json(
            &source.join("settings.json"),
            &json!({"storage-paths":{"dataDirectory":target}}),
        )
        .unwrap();
        let active = RuntimePaths::prepare(&source).unwrap();
        assert!(
            active.migration_error.is_none(),
            "{:?}",
            active.migration_error
        );
        assert_eq!(
            hash(&source.join("sessions/test.jsonl")).unwrap(),
            hash(&target.join("sessions/test.jsonl")).unwrap()
        );
        assert_eq!(
            active.paths["dataDirectory"],
            fs::canonicalize(&target).unwrap()
        );
        assert!(RuntimePaths::prepare(&source).is_err());
        active.release();
        let reopened = RuntimePaths::prepare(&source).unwrap();
        reopened.release();
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
    #[test]
    fn refuses_populated_and_nested_targets_without_overwriting() {
        let source = temp();
        let target = temp();
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep"), "existing").unwrap();
        assert!(copy_to_empty(&source, &target).is_err());
        assert!(copy_to_empty(&source, &source.join("nested")).is_err());
        assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "existing");
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
    #[test]
    fn environment_relocation_is_applied_and_survives_restart() {
        let source = temp();
        let target = temp();
        let active = RuntimePaths::prepare(&source).unwrap();
        active.release();
        fs::write(source.join("environments/package"), "environment").unwrap();
        write_json(
            &source.join("settings.json"),
            &json!({"storage-paths":{"environmentDirectory":target}}),
        )
        .unwrap();
        let next = RuntimePaths::prepare(&source).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("package")).unwrap(),
            "environment"
        );
        next.release();
        RuntimePaths::prepare(&source).unwrap().release();
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
    #[test]
    fn failed_multi_directory_plan_keeps_the_original_home_bootable() {
        let source = temp();
        let target = temp();
        let cache_target = temp();
        let occupied = temp();
        let original = RuntimePaths::prepare(&source).unwrap();
        let original_paths = original.paths.clone();
        original.release();
        fs::write(source.join("cache/cache-entry"), "cached").unwrap();
        fs::write(source.join("environments/runtime"), "runtime").unwrap();
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("keep"), "unrelated").unwrap();
        write_json(&source.join("settings.json"), &json!({"other":{"preserve":true},"storage-paths":{"dataDirectory":target,"cacheDirectory":cache_target,"environmentDirectory":occupied}})).unwrap();
        let recovered = RuntimePaths::prepare(&source).unwrap();
        assert_eq!(recovered.paths, original_paths);
        assert!(recovered.migration_error.is_some());
        assert!(!source.join(REDIRECT).exists());
        assert!(!target.exists());
        assert!(!cache_target.exists());
        assert_eq!(
            fs::read_to_string(occupied.join("keep")).unwrap(),
            "unrelated"
        );
        assert!(
            RuntimePaths::prepare(&source).is_err(),
            "recovery retains the original home lock"
        );
        let settings = read_json(&source.join("settings.json")).unwrap();
        assert_eq!(settings["storage-paths"], json!(original_paths));
        assert_eq!(settings["other"]["preserve"], true);
        recovered.release();
        let reopened = RuntimePaths::prepare(&source).unwrap();
        assert!(reopened.migration_error.is_none());
        reopened.release();
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(occupied).unwrap();
    }
    #[test]
    fn accepts_standard_data_children_and_rejects_cross_purpose_nesting() {
        let source = temp();
        let runtime = RuntimePaths::prepare(&source).unwrap();
        let active = runtime.paths.clone();
        assert!(migration::validate_targets(&active, &active).is_ok());
        let mut invalid = active.clone();
        invalid.insert(
            "cacheDirectory".into(),
            active["environmentDirectory"].join("nested"),
        );
        assert!(migration::validate_targets(&invalid, &active).is_err());
        invalid.insert("cacheDirectory".into(), active["dataDirectory"].clone());
        assert!(migration::validate_targets(&invalid, &active).is_err());
        runtime.release();
        fs::remove_dir_all(source).unwrap();
    }
    #[tokio::test]
    async fn restarting_drains_accepted_requests_and_rejects_new_requests() {
        let source = temp();
        let runtime = RuntimePaths::prepare(&source).unwrap();
        let accepted = runtime.begin_api_request().unwrap();
        let waiting = runtime.clone();
        let drain = tokio::spawn(async move { waiting.quiesce().await });
        tokio::task::yield_now().await;
        assert!(runtime.begin_api_request().is_none());
        assert!(!drain.is_finished());
        drop(accepted);
        drain.await.unwrap().unwrap();
        assert!(runtime.begin_api_request().is_none());
        runtime.release();
        fs::remove_dir_all(source).unwrap();
    }
    #[tokio::test]
    async fn cancelling_a_restart_request_reopens_admission() {
        let source = temp();
        let runtime = RuntimePaths::prepare(&source).unwrap();
        let accepted = runtime.begin_api_request().unwrap();
        let waiting = runtime.clone();
        let drain = tokio::spawn(async move { waiting.quiesce().await });
        tokio::task::yield_now().await;
        assert!(runtime.begin_api_request().is_none());
        drain.abort();
        assert!(drain.await.is_err());
        assert!(runtime.begin_api_request().is_some());
        drop(accepted);
        runtime.release();
        fs::remove_dir_all(source).unwrap();
    }
}
