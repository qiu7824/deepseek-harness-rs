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
use std::io::{Read, Write};
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
}
pub(crate) struct Artifacts {
    root: PathBuf,
    indexes: parking_lot::Mutex<BTreeMap<String, Index>>,
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
    pub fn new(data: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: data.join("artifact-index"),
            indexes: parking_lot::Mutex::new(BTreeMap::new()),
        })
    }
    fn load(
        &self,
        owner: &str,
        root: &Path,
        indexes: &mut BTreeMap<String, Index>,
    ) -> Result<(), String> {
        if indexes.contains_key(owner) {
            return Ok(());
        }
        let file = self.root.join(format!("{}.json", digest(owner.as_bytes())));
        let index = if file.exists() {
            let bytes = fs::read(file).map_err(|e| e.to_string())?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err("产物记录过大".into());
            }
            serde_json::from_slice::<Index>(&bytes).map_err(|e| e.to_string())?
        } else {
            let (baseline, truncated) = scan(root)?;
            Index {
                root: root.to_string_lossy().into_owned(),
                baseline,
                truncated,
                ..Default::default()
            }
        };
        if Path::new(&index.root) != root {
            return Err("任务工作目录已变化，产物索引需要迁移".into());
        }
        persist_json(
            &self.root.join(format!("{}.json", digest(owner.as_bytes()))),
            &index,
        )?;
        indexes.insert(owner.into(), index);
        Ok(())
    }
    fn baseline(&self, owner: &str, root: &Path) -> Result<(), String> {
        let mut indexes = self.indexes.lock();
        self.load(owner, root, &mut indexes)
    }
    fn list(
        &self,
        owner: &str,
        root: &Path,
        events: &[dsh_session::SessionEvent],
    ) -> Result<Value, String> {
        let mut indexes = self.indexes.lock();
        self.load(owner, root, &mut indexes)?;
        let index = indexes.get_mut(owner).unwrap();
        let (current, truncated) = scan(root)?;
        index.truncated |= truncated;
        for (path, stamp) in &current {
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
            for path in index
                .baseline
                .keys()
                .filter(|path| !current.contains_key(*path))
            {
                index
                    .entries
                    .entry(path.clone())
                    .or_insert_with(|| Artifact {
                        path: path.clone(),
                        change: "deleted".into(),
                        source: "workspace".into(),
                        updated_at: dsh_workspace_resources::now(),
                    });
            }
        }
        for event in events.iter().filter(|event| event.type_ == "tool/result") {
            let Some(meta) = event.data.get("meta") else {
                continue;
            };
            let Some(path) = meta.get("path").and_then(Value::as_str) else {
                continue;
            };
            if meta.get("after").is_none() {
                continue;
            }
            let native = PathBuf::from(dsh_host_apiproxy::native_path_opener::display_native_path(
                path,
            ));
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
            index.entries.insert(
                relative.clone(),
                Artifact {
                    path: relative,
                    change: if meta.get("before").is_some_and(Value::is_null) {
                        "created"
                    } else {
                        "modified"
                    }
                    .into(),
                    source: "tool".into(),
                    updated_at: (event.time.max(0) as u64) / 1000,
                },
            );
        }
        for event in events
            .iter()
            .filter(|event| event.type_ == "deliverables/presented")
        {
            for file in event.data["files"].as_array().into_iter().flatten() {
                let Some(path) = file["path"].as_str() else {
                    continue;
                };
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
                index.entries.insert(
                    relative.clone(),
                    Artifact {
                        path: relative,
                        change: "presented".into(),
                        source: "delivery".into(),
                        updated_at: (event.time.max(0) as u64) / 1000,
                    },
                );
            }
        }
        let rows = index
            .entries
            .values()
            .map(|entry| {
                let mut value = serde_json::to_value(entry).unwrap();
                if let Some(stamp) = current.get(&entry.path) {
                    value["size"] = json!(stamp.size);
                    value["etag"] = json!(etag(stamp));
                } else {
                    value["change"] = json!("deleted");
                }
                value
            })
            .collect::<Vec<_>>();
        persist_json(
            &self.root.join(format!("{}.json", digest(owner.as_bytes()))),
            index,
        )?;
        Ok(json!({"entries":rows,"truncated":index.truncated}))
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
    fn file_action(
        &self,
        owner: &str,
        root: &Path,
        args: &Value,
        resources: &Resources,
    ) -> Result<Value, String> {
        let _transaction = self.indexes.lock();
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or("缺少操作")?;
        if action == "restore" {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("缺少资源 ID")?;
            let store = resources.assert_owner(owner, id)?;
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
            copy_new(&source, &target)?;
            store.set_origin(
                id,
                json!({"restored":true,"root":root,"path":origin["path"]}),
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
                fs::rename(&target, &destination).map_err(|e| e.to_string())?;
                Ok(json!({"renamed":true}))
            }
            "trash" => {
                let store = resources.current()?;
                let mut lease = store.allocate(
                    owner,
                    &root.to_string_lossy(),
                    "trash",
                    &relative.to_string_lossy(),
                )?;
                let id = lease.id().to_string();
                store.retain(&id, true, true)?;
                let backup = lease.path().join("payload");
                copy_new(&target, &backup)?;
                let hash = file_digest(&backup)?;
                if etag(&stamp(&target)?) != expected || file_digest(&target)? != hash {
                    return Err("复制期间文件已变化，原文件保留".into());
                }
                store.set_origin(&id,json!({"root":root,"path":relative.to_string_lossy().replace('\\',"/"),"sha256":hash}))?;
                fs::remove_file(&target).map_err(|e| e.to_string())?;
                lease.finish(true)?;
                store.retain(&id, false, false)?;
                Ok(json!({"id":id,"removed":true}))
            }
            _ => Err("未知产物操作".into()),
        }
    }
}

fn file_digest(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
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
fn copy_new(source: &Path, target: &Path) -> Result<(), String> {
    checked_path(source)?;
    checked_path(target)?;
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| e.to_string())?;
    if let Err(error) = std::io::copy(&mut input, &mut output)
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
    {
        drop(output);
        let _ = fs::remove_file(target);
        return Err(error.to_string());
    }
    Ok(())
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
        let result=tokio::task::spawn_blocking(move || -> Result<Value,String> {match operation.as_str(){
            "resources"=>Ok(json!({"entries":resources.list(args.get("sessionId").and_then(Value::as_str))?,"policy":resources.policy(),"warning":resources.location_error()})),
            "collect"=>Ok(json!({"collected":resources.collect(true)?})),
            "resource-files"=>{let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;resources.locate(id)?.files(id,args.get("path").and_then(Value::as_str).unwrap_or_default())},
            "resource-read"=>{let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;let path=args.get("path").and_then(Value::as_str).ok_or("缺少路径")?;Ok(json!({"text":resources.locate(id)?.read_text(id,path,args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,12000)?}))},
            _=>{
                let id=args.get("id").and_then(Value::as_str).ok_or("缺少资源 ID")?;
                let store=resources.locate(id)?;
                match args.get("action").and_then(Value::as_str){Some("pin")=>{let row=store.get(id)?;store.retain(id,args.get("pinned").and_then(Value::as_bool).unwrap_or(true),row.state=="candidate")?;},Some("release")=>store.retain(id,store.get(id)?.pinned,false)?,Some("trash")=>resources.quarantine(id)?,Some("restore")=>store.restore(id)?,_=>return Err("未知资源操作".into())};Ok(json!({"ok":true}))
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
        let events = sessions
            .get(&id)
            .map(|session| session.events())
            .unwrap_or_default();
        let owner = owner.to_string();
        return match tokio::task::spawn_blocking(move || artifacts.list(&owner, &root, &events))
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
