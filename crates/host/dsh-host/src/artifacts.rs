//! Session artifact projection and same-origin resource management.
use super::workspace_resources::Resources;
use axum::body::{Body, to_bytes};
use cordis::{Context, EventOptions, Listener, NextFn, downcast_arc};
use dsh_host_webserver::{
    WebHandlerError, WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer,
};
use dsh_session::{SessionStore, session_id};
use dsh_workspace::WorkspaceRegistry;
use dsh_workspace_resources::{checked_path, digest, persist_json};
use http::{Response, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Seek, Write};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Stamp {
    size: u64,
    modified: u128,
}
fn stamp(path: &Path) -> Result<Stamp, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("目标不是文件".into());
    }
    Ok(Stamp {
        size: meta.len(),
        modified: meta
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    })
}
fn etag(stamp: &Stamp) -> String {
    format!("{}:{}", stamp.size, stamp.modified)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    path: String,
    change: String,
    source: String,
    updated_at: u64,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    root: String,
    baseline: BTreeMap<String, Stamp>,
    entries: BTreeMap<String, Artifact>,
    truncated: bool,
    #[serde(default = "legacy_baseline_ready")]
    baseline_ready: bool,
    #[serde(default)]
    next_event_seq: u64,
    #[serde(skip)]
    observed: BTreeMap<String, Stamp>,
    #[serde(skip)]
    missing: std::collections::BTreeSet<String>,
    #[serde(skip)]
    unavailable: std::collections::BTreeSet<String>,
    #[serde(skip)]
    refresh_needed: bool,
    #[serde(skip)]
    last_saved: Option<std::time::Instant>,
    #[serde(skip)]
    baseline_gate: Arc<parking_lot::Mutex<()>>,
}
fn legacy_baseline_ready() -> bool {
    true
}
fn observe_file(index: &mut Index, root: &Path, relative: &str) {
    index.observed.remove(relative);
    index.missing.remove(relative);
    index.unavailable.remove(relative);
    let Some(safe) = super::web_preview::safe_relative(relative) else {
        return;
    };
    let path = root.join(safe);
    if checked_path(&path).is_err() {
        index.unavailable.insert(relative.into());
        return;
    }
    match fs::metadata(&path) {
        Ok(_) => {
            if let Ok(value) = stamp(&path) {
                index.observed.insert(relative.into(), value);
            } else {
                index.unavailable.insert(relative.into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            index.missing.insert(relative.into());
        }
        Err(_) => {
            index.unavailable.insert(relative.into());
        }
    }
}
pub(crate) struct Artifacts {
    root: PathBuf,
    indexes: parking_lot::Mutex<BTreeMap<String, Arc<parking_lot::Mutex<Index>>>>,
    file_operations: parking_lot::Mutex<()>,
}

fn scan(root: &Path) -> Result<(BTreeMap<String, Stamp>, bool), String> {
    let mut result = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        checked_path(&dir)?;
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if [
                ".git",
                "node_modules",
                "target",
                ".runtime",
                ".venv",
                "__pycache__",
                ".next",
                ".cache",
            ]
            .contains(&name.as_str())
            {
                continue;
            }
            if checked_path(&path).is_err() {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                dirs.push(path)
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                result.insert(relative, stamp(&path)?);
            }
            if result.len() + dirs.len() >= 20000 {
                return Ok((result, true));
            }
        }
    }
    Ok((result, false))
}

impl Artifacts {
    /// User-owned recovery remains possible after the original session was deleted.
    fn restore_original(
        &self,
        id: &str,
        location: Option<&str>,
        resources: &Resources,
    ) -> Result<Value, String> {
        let store = resources.locate_at(id, location)?;
        let row = store.get(id)?;
        if row.kind != "trash" {
            return Err("资源不是已移除产物".into());
        }
        let root = row
            .origin
            .as_ref()
            .and_then(|value| value["root"].as_str())
            .ok_or("恢复记录缺少工作区")?;
        let root = Path::new(root);
        if !root.is_absolute() {
            return Err("恢复工作区不是绝对路径".into());
        }
        checked_path(root)?;
        self.file_action(
            &row.owner,
            root,
            &json!({"action":"restore","id":id,"locationId":location}),
            resources,
        )
    }
    pub fn new(data: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: data.join("artifact-index"),
            indexes: parking_lot::Mutex::new(BTreeMap::new()),
            file_operations: parking_lot::Mutex::new(()),
        })
    }
    fn index(&self, owner: &str, root: &Path) -> Result<Arc<parking_lot::Mutex<Index>>, String> {
        let cached = { self.indexes.lock().get(owner).cloned() };
        if let Some(index) = cached {
            if Path::new(&index.lock().root) != root {
                return Err("任务工作目录已变化，产物索引需要迁移".into());
            }
            return Ok(index);
        }
        let file = self.root.join(format!("{}.json", digest(owner.as_bytes())));
        let index = if file.exists() {
            let bytes = fs::read(file).map_err(|e| e.to_string())?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err("产物记录过大".into());
            }
            serde_json::from_slice::<Index>(&bytes).map_err(|e| e.to_string())?
        } else {
            Index {
                root: root.to_string_lossy().into_owned(),
                ..Default::default()
            }
        };
        if Path::new(&index.root) != root {
            return Err("任务工作目录已变化，产物索引需要迁移".into());
        }
        let index = Arc::new(parking_lot::Mutex::new(index));
        let mut indexes = self.indexes.lock();
        // Persisted indexes can be reloaded; keep only idle entries in the bounded cache.
        if indexes.len() >= 32 {
            let idle = indexes
                .iter()
                .find(|(_, v)| Arc::strong_count(v) == 1)
                .map(|(k, _)| k.clone());
            if let Some(idle) = idle {
                indexes.remove(&idle);
            }
        }
        Ok(indexes.entry(owner.into()).or_insert(index).clone())
    }

    fn baseline(&self, owner: &str, root: &Path) -> Result<(), String> {
        let index = self.index(owner, root)?;
        let gate = {
            let state = index.lock();
            if state.baseline_ready {
                return Ok(());
            }
            state.baseline_gate.clone()
        };
        let _scan = gate.lock();
        if index.lock().baseline_ready {
            return Ok(());
        }
        let (files, truncated) = scan(root)?;
        let mut index = index.lock();
        if !index.baseline_ready {
            index.baseline = files;
            index.truncated = truncated;
            index.baseline_ready = true;
            persist_json(
                &self.root.join(format!("{}.json", digest(owner.as_bytes()))),
                &*index,
            )?;
        }
        Ok(())
    }

    fn list_mode(
        &self,
        owner: &str,
        root: &Path,
        session: Option<&dsh_session::Session>,
        refresh: bool,
    ) -> Result<Value, String> {
        let index = self.index(owner, root)?;
        // Slow traversal never holds the shared map or the per-session index lock.
        let scan_seq = session.map(|s| s.seq().get());
        let scanned = if refresh { Some(scan(root)?) } else { None };
        let mut index = index.lock();
        let mut changed = false;
        let mut advanced = false;
        if let Some((current, truncated)) = &scanned {
            index.refresh_needed = false;
            if !index.baseline_ready {
                index.baseline = current.clone();
                index.baseline_ready = true;
            } else {
                for (path, stamp) in current {
                    let change = match index.baseline.get(path) {
                        Some(before) if before == stamp => continue,
                        Some(_) => "modified",
                        None => "created",
                    };
                    index
                        .entries
                        .entry(path.clone())
                        .or_insert_with(|| Artifact {
                            path: path.clone(),
                            change: change.into(),
                            source: "workspace".into(),
                            updated_at: dsh_workspace_resources::now(),
                        });
                }
                if !truncated && !index.truncated {
                    let deleted: Vec<_> = index
                        .baseline
                        .keys()
                        .filter(|p| !current.contains_key(*p))
                        .cloned()
                        .collect();
                    for path in deleted {
                        index
                            .entries
                            .entry(path.clone())
                            .or_insert_with(|| Artifact {
                                path,
                                change: "deleted".into(),
                                source: "workspace".into(),
                                updated_at: dsh_workspace_resources::now(),
                            });
                    }
                }
            }
            index.truncated |= *truncated;
            index.observed = current.clone();
            index.missing.clear();
            index.unavailable.clear();
            let unobserved: Vec<_> = index
                .entries
                .keys()
                .filter(|p| !current.contains_key(*p))
                .cloned()
                .collect();
            for path in unobserved {
                observe_file(&mut index, root, &path);
            }
            changed = true;
        }
        if let Some(session) = session {
            let end = session.seq().get();
            if index.next_event_seq > end {
                index.next_event_seq = 0;
            }
            let start = index.next_event_seq;
            if start < end {
                session.visit_events(start, Some(end), |event| {
                    if event.type_ == "turn/end"
                        && (!refresh || event.seq.get() >= scan_seq.unwrap_or(0))
                    {
                        index.refresh_needed = true;
                    }
                    let candidates: Vec<(&str, &str, &str)> = match event.type_.as_str() {
                        "tool/result" => event
                            .data
                            .get("meta")
                            .filter(|m| m.get("after").is_some())
                            .and_then(|m| {
                                m["path"].as_str().map(|p| {
                                    (
                                        p,
                                        if m.get("before").is_some_and(Value::is_null) {
                                            "created"
                                        } else {
                                            "modified"
                                        },
                                        "tool",
                                    )
                                })
                            })
                            .into_iter()
                            .collect(),
                        "deliverables/presented" => event.data["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|f| {
                                f["path"].as_str().map(|p| (p, "presented", "delivery"))
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (path, change, source) in candidates {
                        let native = PathBuf::from(
                            dsh_host_apiproxy::native_path_opener::display_native_path(path),
                        );
                        let native = if native.is_absolute() {
                            native
                        } else {
                            root.join(native)
                        };
                        let absolute = fs::canonicalize(&native).unwrap_or(native);
                        let Ok(relative) = absolute.strip_prefix(root) else {
                            continue;
                        };
                        let relative = relative.to_string_lossy().replace('\\', "/");
                        if super::web_preview::safe_relative(&relative).is_none() {
                            continue;
                        }
                        observe_file(&mut index, root, &relative);
                        if source == "tool"
                            && index
                                .entries
                                .get(&relative)
                                .is_some_and(|e| e.source == "delivery")
                        {
                            continue;
                        }
                        index.entries.insert(
                            relative.clone(),
                            Artifact {
                                path: relative,
                                change: change.into(),
                                source: source.into(),
                                updated_at: (event.time.max(0) as u64) / 1000,
                            },
                        );
                        changed = true;
                    }
                    Ok(true)
                })?;
                index.next_event_seq = end;
                advanced = true;
            }
        }
        let mut rows = index
            .entries
            .values()
            .filter_map(|entry| {
                super::web_preview::safe_relative(&entry.path)?;
                let mut value = serde_json::to_value(entry).ok()?;
                if index.unavailable.contains(&entry.path) {
                    value["unavailable"] = json!(true);
                } else if let Some(current) = index.observed.get(&entry.path) {
                    value["size"] = json!(current.size);
                    value["etag"] = json!(etag(current));
                    if entry.change == "deleted" {
                        value["change"] = json!("restored");
                    }
                } else if index.missing.contains(&entry.path) {
                    value["change"] = json!("deleted");
                }
                Some(value)
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| {
            let priority = |value: &Value| match value["source"].as_str() {
                Some("delivery") => 0,
                Some("tool") => 1,
                _ => 2,
            };
            priority(a)
                .cmp(&priority(b))
                .then_with(|| b["updatedAt"].as_u64().cmp(&a["updatedAt"].as_u64()))
                .then_with(|| a["path"].as_str().cmp(&b["path"].as_str()))
        });
        if changed
            || advanced
                && index
                    .last_saved
                    .is_none_or(|saved| saved.elapsed() >= std::time::Duration::from_secs(30))
        {
            persist_json(
                &self.root.join(format!("{}.json", digest(owner.as_bytes()))),
                &*index,
            )?;
            index.last_saved = Some(std::time::Instant::now());
        }
        let result = json!({"entries":rows,"truncated":index.truncated,"workspaceScanned":refresh,"refreshNeeded":index.refresh_needed});
        drop(index);
        let mut indexes = self.indexes.lock();
        while indexes.len() > 32 {
            let idle = indexes
                .iter()
                .find(|(_, v)| Arc::strong_count(v) == 1)
                .map(|(k, _)| k.clone());
            if let Some(idle) = idle {
                indexes.remove(&idle);
            } else {
                break;
            }
        }
        Ok(result)
    }
    pub fn install_tracking(self: &Arc<Self>, ctx: &Context) -> Result<(), String> {
        let service = self.clone();
        let listener: Arc<Listener> = Arc::new(move |_: &Context, args| {
            let execution = downcast_arc::<Arc<dsh_tools::ToolExecution>>(&args[0]).unwrap();
            let next = downcast_arc::<NextFn>(&args[1]).unwrap().clone();
            let service = service.clone();
            Box::pin(async move {
                if ["pwsh", "bash", "write", "edit", "str_replace_editor"]
                    .contains(&execution.name.as_str())
                    || execution.name == "workspace_scratch"
                        && execution.arguments["action"] == "promote"
                {
                    if let Some(agent) = execution.agent.as_ref() {
                        if let Some(root) = agent.session().header().cwd.as_deref() {
                            if let Ok(root) = fs::canonicalize(root) {
                                let _ = service.baseline(agent.id().as_str(), &root);
                            }
                        }
                    }
                }
                Some(next.call().await)
            })
        });
        let dispose = futures::executor::block_on(ctx.on(
            "tools/execute",
            listener,
            EventOptions::default().prepend(true),
        ));
        let _ = ctx.effect(
            "artifact baseline tracking",
            Box::pin(async move { Some(dispose) }),
        );
        Ok(())
    }
    pub(crate) fn file_identity(root: &Path, relative: &str) -> Result<Value, String> {
        let relative = super::web_preview::safe_relative(relative).ok_or("文件路径无效")?;
        let path = root.join(relative);
        checked_path(&path)?;
        let stamp = stamp(&path)?;
        if stamp.size > 128 * 1024 * 1024 {
            return Err("文件操作上限为 128 MiB".into());
        }
        Ok(json!({"etag":etag(&stamp),"bytes":stamp.size,"sha256":file_digest(&path)?}))
    }

    fn file_action(
        &self,
        owner: &str,
        root: &Path,
        args: &Value,
        resources: &Resources,
    ) -> Result<Value, String> {
        self.file_action_tracked(owner, root, args, resources, &|| false, &|| Ok(()))
    }

    pub(crate) fn file_action_tracked(
        &self,
        owner: &str,
        root: &Path,
        args: &Value,
        resources: &Resources,
        signal: &dyn Fn() -> bool,
        mark_effects: &dyn Fn() -> Result<(), String>,
    ) -> Result<Value, String> {
        let _transaction = self.file_operations.lock();
        if signal() {
            return Err("文件操作已取消".into());
        }
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or("缺少操作")?;
        if action == "restore" {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("缺少资源 ID")?;
            let store = resources.locate_at(id, args.get("locationId").and_then(Value::as_str))?;
            if store.get(id)?.owner != owner {
                return Err("不能操作其他任务的资源".into());
            }
            let resource = store.get(id)?;
            if resource.kind != "trash" {
                return Err("资源不是已移除产物".into());
            }
            let origin = resource.origin.ok_or("缺少恢复记录")?;
            if origin.get("root").and_then(Value::as_str) != Some(root.to_string_lossy().as_ref()) {
                return Err("产物所属工作区已变化，不能覆盖其他目录".into());
            }
            let relative = origin
                .get("path")
                .and_then(Value::as_str)
                .and_then(super::web_preview::safe_relative)
                .ok_or("恢复路径无效")?;
            let target = root.join(relative);
            checked_path(&target)?;
            let source = store.path(id, "payload")?;
            if file_digest(&source)? != origin.get("sha256").and_then(Value::as_str).unwrap_or("") {
                return Err("恢复材料校验失败".into());
            }
            let expected = origin["sha256"].as_str().ok_or("恢复记录缺少摘要")?;
            if target.exists() {
                if (origin["restoreStarted"] == true || origin["restored"] == true)
                    && target.is_file()
                    && file_digest(&target)? == expected
                {
                    store.set_origin(id,json!({"restored":true,"root":root,"path":origin["path"],"sha256":expected}))?;
                    return Ok(json!({"restored":true,"alreadyPresent":true}));
                }
                return Err("目标已存在，不能覆盖恢复".into());
            }
            let mut prepared = origin.clone();
            prepared["restoreStarted"] = json!(true);
            store.set_origin(id, prepared)?;
            if let Some(parent) = target.parent() {
                checked_path(parent)?;
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                checked_path(parent)?;
            }
            restore_new(&source, &target, expected, signal, mark_effects)?;
            store.set_origin(
                id,
                json!({"restored":true,"root":root,"path":origin["path"],"sha256":expected}),
            )?;
            return Ok(json!({"restored":true}));
        }
        let relative = args
            .get("path")
            .and_then(Value::as_str)
            .and_then(super::web_preview::safe_relative)
            .ok_or("产物路径无效")?;
        let target = root.join(&relative);
        checked_path(&target)?;
        if !target.is_file() {
            return Err("产物已不存在".into());
        }
        let expected = args
            .get("etag")
            .and_then(Value::as_str)
            .ok_or("缺少文件版本，请刷新列表")?;
        if etag(&stamp(&target)?) != expected {
            return Err("文件已被其他操作修改，请刷新后重试".into());
        }
        if let Some(expected_hash) = args.get("sha256").and_then(Value::as_str) {
            if file_digest(&target)? != expected_hash {
                return Err("批准后文件内容已变化，原文件保留".into());
            }
        }
        match action {
            "rename" => {
                let new_relative = args
                    .get("newPath")
                    .and_then(Value::as_str)
                    .and_then(super::web_preview::safe_relative)
                    .ok_or("新路径无效")?;
                let destination = root.join(new_relative);
                checked_path(&destination)?;
                if destination.exists() {
                    return Err("目标已存在，不能覆盖".into());
                }
                if destination.parent().is_none_or(|parent| !parent.is_dir()) {
                    return Err("目标目录不存在".into());
                }
                if signal() {
                    return Err("文件操作已取消".into());
                }
                mark_effects()?;
                checked_path(&destination)?;
                require_current(
                    &target,
                    expected,
                    args.get("sha256").and_then(Value::as_str),
                )?;
                if signal() {
                    return Err("文件操作已取消".into());
                }
                rename_without_replacement(&target, &destination)?;
                Ok(json!({"renamed":true}))
            }
            "trash" => {
                let store = resources.current_for(&root.to_string_lossy())?;
                let mut lease = store.allocate(
                    owner,
                    &root.to_string_lossy(),
                    "trash",
                    &relative.to_string_lossy(),
                )?;
                let id = lease.id().to_string();
                store.retain(&id, true, true)?;
                let backup = lease.path().join("payload");
                copy_new_cancellable(&target, &backup, signal)?;
                let hash = file_digest(&backup)?;
                if etag(&stamp(&target)?) != expected || file_digest(&target)? != hash {
                    return Err("复制期间文件已变化，原文件保留".into());
                }
                store.set_origin(&id,json!({"root":root,"path":relative.to_string_lossy().replace('\\',"/"),"sha256":hash}))?;
                if signal() {
                    return Err("文件操作已取消，原文件保留".into());
                }
                mark_effects()?;
                checked_path(&backup)?;
                if file_digest(&backup)? != hash {
                    return Err("恢复材料已变化，原文件保留".into());
                }
                require_current(&target, expected, Some(&hash))?;
                if signal() {
                    return Err("文件操作已取消，原文件保留".into());
                }
                fs::remove_file(&target).map_err(|e| e.to_string())?;
                lease.finish(true)?;
                store.retain(&id, false, false)?;
                Ok(json!({"id":id,"removed":true,"recoverable":true}))
            }
            _ => Err("未知产物操作".into()),
        }
    }
}

fn require_current(path: &Path, expected: &str, expected_hash: Option<&str>) -> Result<(), String> {
    checked_path(path)?;
    if etag(&stamp(path)?) != expected
        || expected_hash
            .is_some_and(|expected| !file_digest(path).is_ok_and(|actual| actual == expected))
    {
        return Err("文件已变化，未执行操作；原文件保留".into());
    }
    Ok(())
}

fn file_digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    reader_digest(&mut file)
}
fn reader_digest(file: &mut impl Read) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

struct RestoreStaging {
    path: PathBuf,
    file: Option<fs::File>,
    active: bool,
}
impl Drop for RestoreStaging {
    fn drop(&mut self) {
        // Release the Windows exclusive handle before removing an unfinished copy.
        self.file.take();
        if self.active && checked_path(&self.path).is_ok() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Publish only a complete verified copy. An interrupted process can leave a
/// unique staging file, but cannot leave a partial file at the recovery path.
fn restore_new(
    source: &Path,
    target: &Path,
    expected_hash: &str,
    signal: &dyn Fn() -> bool,
    mark_effects: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checked_path(source)?;
    checked_path(target)?;
    if signal() {
        return Err("文件恢复已取消".into());
    }
    if target.exists() {
        return Err("目标已存在，不能覆盖恢复".into());
    }
    let parent = target.parent().ok_or("恢复目标缺少目录")?;
    let path = parent.join(format!(".dsh-restore-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(&path).map_err(|e| e.to_string())?;
    let mut staging = RestoreStaging {
        path,
        file: Some(file),
        active: true,
    };
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let output = staging.file.as_mut().unwrap();
    let mut buffer = [0u8; 65536];
    loop {
        if signal() {
            return Err("文件恢复已取消".into());
        }
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    output.sync_all().map_err(|e| e.to_string())?;
    output.rewind().map_err(|e| e.to_string())?;
    if reader_digest(output)? != expected_hash {
        return Err("恢复材料校验失败".into());
    }
    staging.file.take();
    if signal() {
        return Err("文件恢复已取消".into());
    }
    mark_effects()?;
    checked_path(&staging.path)?;
    if file_digest(&staging.path)? != expected_hash {
        return Err("恢复暂存内容已变化".into());
    }
    checked_path(target)?;
    if signal() {
        return Err("文件恢复已取消".into());
    }
    rename_without_replacement(&staging.path, target)?;
    staging.active = false;
    Ok(())
}
fn copy_new_cancellable(
    source: &Path,
    target: &Path,
    signal: &dyn Fn() -> bool,
) -> Result<(), String> {
    checked_path(source)?;
    checked_path(target)?;
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| e.to_string())?;
    let copied = (|| {
        let mut buffer = [0u8; 65536];
        loop {
            if signal() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "file operation cancelled",
                ));
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
        Ok(())
    })();
    if let Err(error) = copied
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
    {
        drop(output);
        let _ = fs::remove_file(target);
        return Err(error.to_string());
    }
    Ok(())
}

fn rename_without_replacement(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 8) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(source, target).map_err(|error| error.to_string())?;
        fs::remove_file(source).map_err(|error| format!("新名称已创建，原名称未移除：{error}"))
    }
}

fn response(status: StatusCode, value: Value) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(value.to_string()))
        .unwrap()
}
fn failure(message: impl Into<String>) -> WebResponse {
    response(StatusCode::BAD_REQUEST, json!({"message":message.into()}))
}

async fn handle(
    request: WebRequest,
    resources: Arc<Resources>,
    artifacts: Arc<Artifacts>,
    registry: Arc<WorkspaceRegistry>,
    sessions: Arc<SessionStore>,
    remote: bool,
) -> WebResponse {
    if !request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| super::allowed_web_authority(host, remote))
        || !super::trusted_web_request(&request, remote)
    {
        return response(StatusCode::FORBIDDEN, json!({"message":"请求来源不可信"}));
    }
    if request.method() != http::Method::POST {
        return response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"message":"只允许 POST"}),
        );
    }
    let operation = request
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let bytes = match to_bytes(Body::new(request.into_body()), 65536).await {
        Ok(bytes) => bytes,
        Err(_) => return failure("请求过大"),
    };
    let args: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return failure("请求格式无效"),
    };
    if operation == "workspace-settings" {
        let Some(path) = args["path"].as_str() else {
            return failure("缺少工作区路径");
        };
        if args.get("location").is_some_and(|value| !value.is_string()) {
            return failure("垃圾槽位置必须是字符串");
        }
        return match resources
            .workspace_location(path, args.get("location").and_then(Value::as_str))
            .await
        {
            Ok(value) => response(StatusCode::OK, value),
            Err(error) => failure(error),
        };
    }
    if operation == "resources"
        || operation == "collect"
        || operation == "resource-action"
        || operation == "resource-files"
        || operation == "resource-read"
    {
        let result=tokio::task::spawn_blocking(move || -> Result<Value,String> {let location=match args.get("locationId"){None|Some(Value::Null)=>None,Some(Value::String(value)) if !value.is_empty()=>Some(value.as_str()),_=>return Err("存储位置标识无效".into())};match operation.as_str(){
            "resources"=>{let owner=match args.get("sessionId"){None=>None,Some(Value::String(value)) if !value.is_empty()=>Some(value.as_str()),_=>return Err("任务范围无效".into())};Ok(json!({"entries":resources.list(owner)?,"policy":resources.policy(),"warning":resources.location_error()}))},
            "collect"=>{let owner=match args.get("sessionId"){None=>None,Some(Value::String(value)) if !value.is_empty()=>Some(value.as_str()),_=>return Err("清理任务范围无效".into())};Ok(json!({"collected":resources.collect_for(owner,true)?}))},
            "resource-files"=>{let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;resources.locate_at(id,location)?.files(id,args.get("path").and_then(Value::as_str).unwrap_or_default())},
            "resource-read"=>{let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;let path=args.get("path").and_then(Value::as_str).ok_or("缺少路径")?;Ok(json!({"text":resources.locate_at(id,location)?.read_text(id,path,args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,12000)?}))},
            _=>{
                let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;
                let store=resources.locate_at(id,location)?;
                match args.get("action").and_then(Value::as_str){Some("pin")=>{let row=store.get(id)?;store.retain(id,args.get("pinned").and_then(Value::as_bool).unwrap_or(true),row.state=="candidate")?;},Some("release")=>store.retain(id,store.get(id)?.pinned,false)?,Some("trash")=>resources.quarantine_at(id,location)?,Some("restore")=>store.restore(id)?,Some("restore-original")=>{artifacts.restore_original(id,location,&resources)?;},_=>return Err("未知资源操作".into())};Ok(json!({"ok":true}))
            }
        }}).await;
        return match result {
            Ok(Ok(value)) => response(StatusCode::OK, value),
            Ok(Err(error)) => failure(error),
            Err(error) => failure(error.to_string()),
        };
    }
    let Some(owner) = args.get("sessionId").and_then(Value::as_str) else {
        return failure("缺少任务 ID");
    };
    let id = session_id(owner);
    let (_, root) = match super::web_preview::workspace_root(&registry, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if operation == "list" {
        let session = sessions.get(&id);
        let owner = owner.to_string();
        let refresh = args.get("refresh").and_then(Value::as_bool).unwrap_or(true);
        return match tokio::task::spawn_blocking(move || {
            artifacts.list_mode(&owner, &root, session.as_ref(), refresh)
        })
        .await
        {
            Ok(Ok(value)) => response(StatusCode::OK, value),
            Ok(Err(error)) => failure(error),
            Err(error) => failure(error.to_string()),
        };
    }
    if operation == "file-action" {
        let owner = owner.to_string();
        return match tokio::task::spawn_blocking(move || {
            artifacts.file_action(&owner, &root, &args, &resources)
        })
        .await
        {
            Ok(Ok(value)) => response(StatusCode::OK, value),
            Ok(Err(error)) => failure(error),
            Err(error) => failure(error.to_string()),
        };
    }
    failure("未知产物操作")
}

pub(crate) fn register(
    ctx: &Context,
    web: &Arc<WebServer>,
    resources: Arc<Resources>,
    artifacts: Arc<Artifacts>,
    registry: Arc<WorkspaceRegistry>,
    sessions: Arc<SessionStore>,
    remote: bool,
) {
    let route = web.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-artifacts".into(),
        handler: Arc::new(move |request| {
            let resources = resources.clone();
            let artifacts = artifacts.clone();
            let registry = registry.clone();
            let sessions = sessions.clone();
            Box::pin(async move {
                Ok::<_, WebHandlerError>(
                    handle(request, resources, artifacts, registry, sessions, remote).await,
                )
            })
        }),
    });
    let _ = ctx.effect(
        "artifact management route",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                route();
                Box::pin(async {})
            }))
        }),
    );
}
#[cfg(test)]
#[path = "cleanup_recovery_tests.rs"]
mod cleanup_recovery_tests;
#[cfg(test)]
#[path = "artifact_index_tests.rs"]
mod index_tests;
#[cfg(test)]
#[path = "artifacts_restore_tests.rs"]
mod restore_tests;
