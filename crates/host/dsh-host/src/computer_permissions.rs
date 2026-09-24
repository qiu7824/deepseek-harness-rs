//! Durable, user-managed application grants. Only adapter-attested targets
//! enter the catalog; the settings API accepts opaque catalog IDs, not paths.
use cordis::Context;
use dsh_tool_computer_use_command::{
    AbortPredicate, AdapterError, COMPUTER_PERMISSION_SCOPES, ComputerPermissionLease,
    ComputerPermissionRequest, ComputerPermissionService, ComputerTargetIdentity,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grant {
    id: String,
    target_id: String,
    identity: ComputerTargetIdentity,
    scopes: Vec<String>,
    scope: String,
    owner_id: Option<String>,
    remaining: Option<u64>,
    created_at: u64,
    revoked: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    version: u32,
    revision: u64,
    host_id: String,
    grants: Vec<Grant>,
}
struct State {
    saved: Saved,
    storage_error: Option<String>,
    fingerprint: Option<String>,
    observed: BTreeMap<String, (ComputerTargetIdentity, String)>,
    generations: BTreeMap<String, Arc<AtomicU64>>,
}

pub(super) struct ComputerPermissions {
    ctx: Context,
    path: PathBuf,
    state: Arc<Mutex<State>>,
    open: Arc<AtomicBool>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn target_key(target: &ComputerTargetIdentity) -> String {
    let identity = json!([
        target.host_id,
        target.device_id,
        target.application_id,
        target.application_revision,
        target.origin
    ]);
    format!("app-{:x}", Sha256::digest(identity.to_string().as_bytes()))
}
fn error(code: &str, message: impl Into<String>) -> AdapterError {
    AdapterError::new(code, message)
}
fn empty_saved() -> Saved {
    Saved {
        version: 1,
        revision: 0,
        host_id: uuid::Uuid::new_v4().to_string(),
        grants: vec![],
    }
}
fn last_good(path: &std::path::Path) -> PathBuf {
    path.with_file_name("computer-permissions.last-good.json")
}
fn fingerprint(path: &std::path::Path) -> Result<Option<String>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 512 * 1024 {
        return Ok(Some(format!(
            "metadata:{}:{:?}:{}",
            meta.len(),
            meta.modified().ok(),
            meta.file_type().is_symlink()
        )));
    }
    Ok(Some(format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).map_err(|e| e.to_string())?)
    )))
}
fn validate(saved: &Saved) -> Result<(), String> {
    if saved.version != 1
        || saved.revision > 9_007_199_254_740_990
        || uuid::Uuid::parse_str(&saved.host_id).is_err()
        || saved.grants.len() > 512
    {
        return Err("应用授权配置版本、修订或数量无效".into());
    }
    let mut ids = std::collections::HashSet::new();
    for grant in &saved.grants {
        if uuid::Uuid::parse_str(&grant.id).is_err()
            || !ids.insert(&grant.id)
            || grant.target_id != target_key(&grant.identity)
            || grant.scopes.is_empty()
            || grant.scopes.len() > COMPUTER_PERMISSION_SCOPES.len()
            || grant
                .scopes
                .iter()
                .any(|s| !COMPUTER_PERMISSION_SCOPES.contains(&s.as_str()))
            || grant
                .scopes
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != grant.scopes.len()
            || !matches!(grant.scope.as_str(), "once" | "session" | "persistent")
            || (grant.scope == "persistent" && grant.owner_id.is_some())
            || (grant.scope != "persistent" && grant.owner_id.as_deref().is_none_or(str::is_empty))
            || (grant.scope == "once" && !matches!(grant.remaining, Some(0 | 1)))
            || (grant.scope != "once" && grant.remaining.is_some())
        {
            return Err("应用授权记录无效".into());
        }
    }
    Ok(())
}
fn load(path: &std::path::Path) -> Result<Saved, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 512 * 1024 {
        return Err("应用授权配置不是有效的有界普通文件".into());
    }
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "应用授权配置无法解析")?;
    validate(&saved)?;
    Ok(saved)
}
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("授权目录无效")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(
        ".computer-permissions-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e: std::io::Error| e.to_string())
}
fn save(path: &std::path::Path, saved: &Saved) -> Result<(), String> {
    validate(saved)?;
    let bytes = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    if bytes.len() > 512 * 1024 {
        return Err("应用授权记录超过大小限制".into());
    }
    write_atomic(path, &bytes)?;
    if let Err(error) = write_atomic(&last_good(path), &bytes) {
        eprintln!("computer permission backup unavailable: {error}");
    }
    Ok(())
}
fn verify_storage(state: &mut State, path: &std::path::Path) -> Result<(), String> {
    if let Some(error) = &state.storage_error {
        return Err(error.clone());
    }
    let current = match fingerprint(path) {
        Ok(current) => current,
        Err(error) => {
            for generation in state.generations.values() {
                generation.fetch_add(1, Ordering::SeqCst);
            }
            state.storage_error = Some(error.clone());
            return Err(error);
        }
    };
    if current != state.fingerprint {
        for generation in state.generations.values() {
            generation.fetch_add(1, Ordering::SeqCst);
        }
        state.fingerprint = current;
        state.storage_error = Some("授权配置已被外部修改，请重新读取后再操作".into());
        return Err(state.storage_error.clone().unwrap());
    }
    Ok(())
}
impl ComputerPermissions {
    pub(super) fn install(ctx: &Context, data_root: PathBuf) -> Result<Arc<Self>, String> {
        let path = data_root.join("computer-permissions.json");
        let (saved, storage_error) = if path.exists() {
            match load(&path) {
                Ok(saved) => (saved, None),
                Err(error) => (empty_saved(), Some(error)),
            }
        } else {
            (empty_saved(), None)
        };
        let source = fingerprint(&path).ok().flatten();
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            path,
            state: Arc::new(Mutex::new(State {
                saved,
                storage_error,
                fingerprint: source,
                observed: BTreeMap::new(),
                generations: BTreeMap::new(),
            })),
            open: Arc::new(AtomicBool::new(true)),
        });
        let open = service.open.clone();
        let _ = ctx.effect(
            "computer permissions lifetime",
            Box::pin(async move {
                Some(cordis::events::make_disposer(move || {
                    open.store(false, Ordering::SeqCst);
                    Box::pin(async {})
                }))
            }),
        );
        let erased: Arc<dyn ComputerPermissionService> = service.clone();
        ctx.register_service(erased);
        Ok(service)
    }
    pub(super) fn snapshot(&self) -> Value {
        let state = self.state.lock();
        json!({"storageError":state.storage_error,"canRestore":load(&last_good(&self.path)).is_ok(),"revision":state.saved.revision,"hostId":state.saved.host_id,"scopes":COMPUTER_PERMISSION_SCOPES,"targets":state.observed.iter().map(|(id,(identity,owner))|json!({"id":id,"identity":identity,"ownerSessionId":owner})).collect::<Vec<_>>(),"grants":state.saved.grants,"capabilities":{"nativeDesktop":cfg!(windows),"browser":true,"remoteDesktop":"device_scope_only"}})
    }
    fn mutate(&self, args: &Value) -> Result<Value, String> {
        if !self.open.load(Ordering::SeqCst) {
            return Err("应用授权服务已关闭".into());
        }
        let mut state = self.state.lock();
        if args["expectedRevision"].as_u64() != Some(state.saved.revision) {
            return Err("授权列表已变化，请刷新后再操作".into());
        }
        if args["action"] == "reload" {
            match load(&self.path) {
                Ok(saved) => {
                    state.saved = saved;
                    state.storage_error = None;
                }
                Err(error) => state.storage_error = Some(error),
            }
            state.fingerprint = fingerprint(&self.path).ok().flatten();
            state.observed.clear();
            for generation in state.generations.values() {
                generation.fetch_add(1, Ordering::SeqCst);
            }
            drop(state);
            return Ok(self.snapshot());
        }
        if args["action"] == "recover" {
            if state.storage_error.is_none() {
                return Err("当前授权配置无需恢复".into());
            }
            if fingerprint(&self.path)? != state.fingerprint {
                return Err("配置再次变化，请先重新读取".into());
            }
            let mut restored = match args["strategy"].as_str() {
                Some("last-good") => load(&last_good(&self.path))?,
                Some("reset") => empty_saved(),
                _ => return Err("恢复方式无效".into()),
            };
            restored.revision = state
                .saved
                .revision
                .max(restored.revision)
                .checked_add(1)
                .ok_or("授权修订溢出")?;
            validate(&restored)?;
            let backup = self.path.with_file_name(format!(
                "computer-permissions.invalid-{}.json",
                uuid::Uuid::new_v4()
            ));
            if std::fs::symlink_metadata(&self.path).is_ok() {
                std::fs::rename(&self.path, &backup).map_err(|e| e.to_string())?;
            }
            if let Err(error) = save(&self.path, &restored) {
                state.fingerprint = fingerprint(&self.path).ok().flatten();
                state.storage_error = Some(error.clone());
                return Err(format!(
                    "恢复未完成，原文件保留在 {}：{error}",
                    backup.display()
                ));
            }
            state.saved = restored;
            state.fingerprint = fingerprint(&self.path)?;
            state.storage_error = None;
            state.observed.clear();
            for generation in state.generations.values() {
                generation.fetch_add(1, Ordering::SeqCst);
            }
            drop(state);
            return Ok(self.snapshot());
        }
        verify_storage(&mut state, &self.path)?;
        let mut saved = state.saved.clone();
        let changed_key;
        match args["action"].as_str() {
            Some("grant") => {
                let target_id = args["targetId"].as_str().ok_or("缺少已验证的应用标识")?;
                let (identity, observed_owner) = state
                    .observed
                    .get(target_id)
                    .ok_or("应用尚未被控制器验证，请先定位目标应用")?;
                let scope = args["scope"]
                    .as_str()
                    .filter(|scope| matches!(*scope, "once" | "session" | "persistent"))
                    .ok_or("授权范围无效")?;
                if scope != "persistent"
                    && args["ownerSessionId"].as_str() != Some(observed_owner.as_str())
                {
                    return Err("目标所属任务已变化，请刷新后重新选择授权范围".into());
                }
                let scopes = args["scopes"]
                    .as_array()
                    .ok_or("缺少操作权限")?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .filter(|value| COMPUTER_PERMISSION_SCOPES.contains(value))
                            .map(str::to_string)
                            .ok_or("操作权限无效")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if scopes.is_empty() || scopes.len() > COMPUTER_PERMISSION_SCOPES.len() {
                    return Err("请选择至少一项操作权限".into());
                }
                if saved.grants.len() >= 512 {
                    saved
                        .grants
                        .retain(|grant| !grant.revoked && grant.remaining != Some(0));
                }
                if saved.grants.len() >= 512 {
                    return Err("授权数量已达上限，请撤销不用的规则".into());
                }
                saved.grants.push(Grant {
                    id: uuid::Uuid::new_v4().to_string(),
                    target_id: target_id.into(),
                    identity: identity.clone(),
                    scopes,
                    scope: scope.into(),
                    owner_id: if scope == "persistent" {
                        None
                    } else {
                        Some(observed_owner.clone())
                    },
                    remaining: if scope == "once" { Some(1) } else { None },
                    created_at: now(),
                    revoked: false,
                });
                changed_key = None;
            }
            Some("revoke") => {
                let id = args["grantId"].as_str().ok_or("缺少授权标识")?;
                let grant = saved
                    .grants
                    .iter_mut()
                    .find(|grant| grant.id == id)
                    .ok_or("授权不存在")?;
                grant.revoked = true;
                changed_key = Some(grant.id.clone());
            }
            _ => return Err("未知授权操作".into()),
        }
        saved.revision = saved.revision.saturating_add(1);
        save(&self.path, &saved)?;
        state.saved = saved;
        state.fingerprint = fingerprint(&self.path)?;
        if let Some(key) = changed_key {
            state
                .generations
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .fetch_add(1, Ordering::SeqCst);
        }
        drop(state);
        Ok(self.snapshot())
    }
    pub(super) fn register(
        self: &Arc<Self>,
        server: &Arc<dsh_host_webserver::WebServer>,
        allow_remote: bool,
    ) -> dsh_host_webserver::RouteDisposer {
        let service = self.clone();
        server.register(dsh_host_webserver::WebRoute {
            kind: dsh_host_webserver::WebRouteKind::Exact,
            path: "/__dsh-computer-permissions".into(),
            handler: Arc::new(move |request| {
                let service = service.clone();
                Box::pin(async move {
                    let trusted = crate::trusted_web_request(&request, allow_remote);
                    let post = request.method() == http::Method::POST
                        && request
                            .headers()
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value.starts_with("application/json"));
                    let result = if !trusted {
                        Err("禁止跨站访问".into())
                    } else if !post {
                        Err("需要 JSON POST 请求".into())
                    } else {
                        match axum::body::to_bytes(
                            axum::body::Body::new(request.into_body()),
                            16 * 1024,
                        )
                        .await
                        {
                            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                                Ok(args) => {
                                    if args["action"] == "list" {
                                        Ok(service.snapshot())
                                    } else {
                                        service.mutate(&args)
                                    }
                                }
                                Err(_) => Err("无效 JSON".into()),
                            },
                            Err(_) => Err("请求超过大小限制".into()),
                        }
                    };
                    let (status, value) = match result {
                        Ok(value) => (http::StatusCode::OK, value),
                        Err(error) => (
                            if trusted {
                                http::StatusCode::BAD_REQUEST
                            } else {
                                http::StatusCode::FORBIDDEN
                            },
                            json!({"error":error}),
                        ),
                    };
                    Ok(http::Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .header("cache-control", "no-store")
                        .body(axum::body::Body::from(value.to_string()))
                        .expect("computer permission response"))
                })
            }),
        })
    }
}
impl ComputerPermissionService for ComputerPermissions {
    fn authorize(
        &self,
        request: ComputerPermissionRequest,
    ) -> futures::future::BoxFuture<'static, Result<ComputerPermissionLease, AdapterError>> {
        let state = self.state.clone();
        let path = self.path.clone();
        let ctx = self.ctx.clone();
        let open = self.open.clone();
        Box::pin(async move {
            if request.signal.as_ref()() || !open.load(Ordering::SeqCst) {
                return Err(AdapterError::cancelled());
            }
            if request.scopes.is_empty()
                || request
                    .scopes
                    .iter()
                    .any(|s| !COMPUTER_PERMISSION_SCOPES.contains(&s.as_str()))
            {
                return Err(error(
                    "COMPUTER_USE_PERMISSION_REQUIRED",
                    "Application permission scope is invalid",
                ));
            }
            if [
                &request.target.host_id,
                &request.target.device_id,
                &request.target.application_id,
                &request.target.application_revision,
                &request.target.target_revision,
            ]
            .iter()
            .any(|value| value.is_empty() || value.len() > 16384)
            {
                return Err(error(
                    "COMPUTER_USE_IDENTITY_UNAVAILABLE",
                    "Application identity is incomplete or exceeds the metadata limit",
                ));
            }
            if request.owner_id.is_empty() {
                return Err(error(
                    "COMPUTER_USE_OWNER_REQUIRED",
                    "Application authorization requires an owning task",
                ));
            }
            let (key, generation, stamp, revision, granted) = {
                let mut state = state.lock();
                verify_storage(&mut state, &path)
                    .map_err(|message| error("COMPUTER_USE_PERMISSION_STORAGE", message))?;
                let mut identity = request.target.clone();
                identity.label = identity.label.chars().take(512).collect();
                identity.host_id = if identity.host_id == "local" {
                    state.saved.host_id.clone()
                } else {
                    format!("{}::{}", state.saved.host_id, identity.host_id)
                };
                let key = target_key(&identity);
                if !state.observed.contains_key(&key) && state.observed.len() >= 128 {
                    let oldest = state.observed.keys().next().cloned().unwrap();
                    state.observed.remove(&oldest);
                }
                state
                    .observed
                    .insert(key.clone(), (identity, request.owner_id.clone()));
                if state.generations.len() > 256 {
                    state
                        .generations
                        .retain(|_, generation| Arc::strong_count(generation) > 1);
                }
                let matched = state.saved.grants.iter().position(|grant| {
                    !grant.revoked
                        && grant.target_id == key
                        && grant.remaining != Some(0)
                        && grant
                            .owner_id
                            .as_ref()
                            .is_none_or(|owner| owner == &request.owner_id)
                        && request
                            .scopes
                            .iter()
                            .all(|scope| grant.scopes.contains(scope))
                });
                let generation_key = matched
                    .map(|index| state.saved.grants[index].id.clone())
                    .unwrap_or_else(|| format!("approval:{}:{key}", request.owner_id));
                let generation = state
                    .generations
                    .entry(generation_key)
                    .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                    .clone();
                let stamp = generation.load(Ordering::SeqCst);
                if let Some(index) = matched
                    && state.saved.grants[index].remaining == Some(1)
                {
                    let mut saved = state.saved.clone();
                    saved.grants[index].remaining = Some(0);
                    saved.revision += 1;
                    save(&path, &saved)
                        .map_err(|message| error("COMPUTER_USE_PERMISSION_STORAGE", message))?;
                    state.saved = saved;
                    state.fingerprint = fingerprint(&path)
                        .map_err(|message| error("COMPUTER_USE_PERMISSION_STORAGE", message))?;
                }
                (
                    key,
                    generation,
                    stamp,
                    state.saved.revision,
                    matched.is_some(),
                )
            };
            if !granted {
                let agents=ctx.get_typed::<Arc<dsh_agent::AgentRegistry>>("agents",false).ok_or_else(||error("COMPUTER_USE_PERMISSION_REQUIRED","Application permission is required; open application permissions in settings"))?;
                let owner = agents
                    .get(&dsh_session::session_id(&request.owner_id))
                    .ok_or_else(|| {
                        error(
                            "COMPUTER_USE_OWNER_REQUIRED",
                            "The owning task is no longer active",
                        )
                    })?;
                let approval = ctx
                    .get_typed::<Arc<dsh_user_approval::ApprovalService>>("approval", false)
                    .ok_or_else(|| {
                        error(
                            "COMPUTER_USE_PERMISSION_REQUIRED",
                            "No approval service is available",
                        )
                    })?;
                let outcome=approval.request(&dsh_user_approval::ApprovalRequest{agent:owner,tool_name:"computer_use".into(),call_id:None,reason:Some(format!("应用：{}\n应用标识：{}\n版本摘要：{}\n设备：{}\n来源：{}\n操作：{}\n本次所需权限：{}",request.target.label,request.target.application_id,request.target.application_revision,request.target.device_id,request.target.origin.as_deref().unwrap_or("桌面应用"),request.action,request.scopes.join(", "))),grant_key:Some(format!("computer:{key}:{}",request.scopes.join(","))),rememberable:false,signal:Some(request.signal.clone())}).await.map_err(|message|error("COMPUTER_USE_PERMISSION_REQUIRED",message))?;
                if !matches!(
                    outcome,
                    dsh_user_approval::ApprovalOutcome::AllowedOnce
                        | dsh_user_approval::ApprovalOutcome::AllowedAlways
                ) {
                    return Err(error(
                        "COMPUTER_USE_PERMISSION_DENIED",
                        "Application action was not authorized; no action was dispatched",
                    ));
                }
            }
            if generation.load(Ordering::SeqCst) != stamp || request.signal.as_ref()() {
                return Err(error(
                    "COMPUTER_USE_PERMISSION_REVOKED",
                    "Application authorization changed while waiting",
                ));
            }
            let valid: AbortPredicate = Arc::new(move || {
                open.load(Ordering::SeqCst) && generation.load(Ordering::SeqCst) == stamp
            });
            Ok(ComputerPermissionLease {
                target: request.target,
                revision,
                valid,
            })
        })
    }
}

#[cfg(test)]
#[path = "computer_permissions_tests.rs"]
mod tests;
