//! Host-owned preferences and execution-scoped capability diagnostics.
//!
//! Profile data never grants permissions and never executes project configuration.
//! Each tool resolves one immutable snapshot; permission retries retain that snapshot.
use cordis::Context;
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use dsh_sandbox::{
    ConfinedSandboxMode, SandboxExecutionPolicy, SandboxMode, SandboxPolicy, SandboxProvider,
};
use dsh_shell::{ExecutionProfileResolver, ResolvedExecutionProfile};
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    environment_capabilities::{EnvironmentCapabilities, IDS},
    runtime_paths::RuntimePaths,
};

const MAX_FILE: u64 = 128 * 1024;
const MAX_PROFILES: usize = 256;
const MAX_CHECKS: usize = 128;
const MODULES: [&str; 9] = [
    "PIL",
    "pypdf",
    "fitz",
    "openpyxl",
    "docx",
    "pptx",
    "reportlab",
    "cv2",
    "numpy",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ShellKind {
    #[default]
    Powershell,
    Bash,
    Zsh,
}
impl ShellKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Powershell => "powershell",
            Self::Bash => "bash",
            Self::Zsh => "zsh",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Preferences {
    #[serde(default)]
    pub shell_kind: Option<ShellKind>,
    #[serde(default)]
    pub shell_path: Option<String>,
    #[serde(default)]
    pub python_path: Option<String>,
    #[serde(default)]
    pub wps_path: Option<String>,
    #[serde(default)]
    pub toolchain_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub use_project_python: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            shell_kind: None,
            shell_path: None,
            python_path: None,
            wps_path: None,
            toolchain_paths: BTreeMap::new(),
            use_project_python: false,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    revision: u64,
    preferences: Preferences,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileFile {
    version: u32,
    revision: u64,
    profiles: BTreeMap<String, Profile>,
}
impl Default for ProfileFile {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    checked_at: u64,
    value: Value,
}
#[derive(Default, Serialize, Deserialize)]
struct CheckFile {
    version: u32,
    checks: BTreeMap<String, Check>,
}

/// Preferences are deliberately stored outside project directories: repository files cannot
/// select code to execute, overwrite user choices, or alter a permission mode.
pub(super) struct ExecutionProfiles {
    own: std::sync::Weak<Self>,
    ctx: Context,
    runtime: Arc<dyn SubprocessRuntime>,
    paths: Arc<RuntimePaths>,
    host: Arc<EnvironmentCapabilities>,
    profile_path: PathBuf,
    cache_path: PathBuf,
    state: Mutex<ProfileFile>,
    checks: Mutex<BTreeMap<String, Check>>,
    gates: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    save_gate: tokio::sync::Mutex<()>,
    cache_gate: tokio::sync::Mutex<()>,
    generation: AtomicU64,
    flights: Mutex<BTreeMap<String, Arc<ValidationFlight>>>,
}

struct ValidationFlight {
    result: Shared<BoxFuture<'static, Result<Value, String>>>,
    waiters: AtomicUsize,
    cancelled: Arc<AtomicBool>,
}
struct ValidationWaiter(Arc<ValidationFlight>);
#[derive(Clone)]
struct SelectedProbe {
    path: Option<String>,
    policy: SandboxExecutionPolicy,
    context_id: String,
    kind: ShellKind,
    profile_revision: u64,
}
impl Drop for ValidationWaiter {
    fn drop(&mut self) {
        if self.0.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.cancelled.store(true, Ordering::Release);
        }
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
fn read_bounded<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    (std::fs::metadata(path).ok()?.len() <= MAX_FILE).then_some(())?;
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
async fn persist(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("环境配置或缓存超过 128 KiB 上限".into());
    }
    dsh_atomic_write::write_file_atomic(
        path,
        &bytes,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|e| e.to_string())
}
fn absolute_directory(cwd: &str) -> Result<String, String> {
    let path = std::fs::canonicalize(cwd).map_err(|e| format!("工作区不可用：{e}"))?;
    if !path.is_dir() {
        return Err("工作区必须是目录".into());
    }
    Ok(path.to_string_lossy().into_owned())
}
fn file_stamp(path: &str) -> Value {
    std::fs::metadata(path).ok().map(|m| json!({"path":path,"length":m.len(),"modified":m.modified().ok().and_then(|s|s.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())})).unwrap_or_else(|| json!({"path":path,"missing":true}))
}
fn locate(names: &[&str]) -> Option<String> {
    // Relative PATH entries could execute repository-controlled files and are not trusted probes.
    for name in names {
        for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .take(256)
            .filter(|p| p.is_absolute())
        {
            let path = dir.join(name);
            if path.is_file() && !is_app_execution_alias(&path) {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
}
fn is_app_execution_alias(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    normalized
        .rsplit_once('/')
        .is_some_and(|(parent, _)| parent.ends_with("/microsoft/windowsapps"))
}
fn ensure_file(path: String, capability: &str) -> Result<String, String> {
    if is_app_execution_alias(Path::new(&path)) {
        return Err(format!(
            "[APP_EXECUTION_ALIAS_UNSUPPORTED] 已选择的 {capability} 是 Windows 应用执行别名：{path}；请选择实际安装目录中的可执行文件，当前选择不会被静默替换"
        ));
    }
    if !Path::new(&path).is_absolute() || !Path::new(&path).is_file() {
        return Err(format!(
            "已选择的 {capability} 不可用：{path}；保留当前选择，请检查环境或修改设置"
        ));
    }
    Ok(path)
}
fn validate_preferences(value: &Preferences) -> Result<(), String> {
    for path in [&value.shell_path, &value.python_path, &value.wps_path]
        .into_iter()
        .flatten()
        .chain(value.toolchain_paths.values())
    {
        if !Path::new(path).is_absolute() || path.len() > 4096 || path.contains(['\0', '\n', '\r'])
        {
            return Err("程序必须使用绝对路径，不支持命令字符串或初始化脚本".into());
        }
    }
    if value
        .toolchain_paths
        .keys()
        .any(|k| !matches!(k.as_str(), "node" | "rustc" | "cargo" | "git" | "rg" | "ffmpeg"))
    {
        return Err("未知工具链名称".into());
    }
    Ok(())
}

impl ExecutionProfiles {
    pub(super) fn install(
        ctx: &Context,
        runtime: Arc<dyn SubprocessRuntime>,
        paths: Arc<RuntimePaths>,
        host: Arc<EnvironmentCapabilities>,
    ) -> Arc<Self> {
        let profile_path = paths.paths["dataDirectory"].join("execution-profiles-v1.json");
        let cache_path = paths.paths["cacheDirectory"].join("execution-capabilities-v1.json");
        let state = read_bounded::<ProfileFile>(&profile_path)
            .filter(|s| {
                s.version == 1
                    && s.profiles.len() <= MAX_PROFILES
                    && s.profiles
                        .values()
                        .all(|p| validate_preferences(&p.preferences).is_ok())
            })
            .unwrap_or_default();
        let checks = read_bounded::<CheckFile>(&cache_path)
            .filter(|s| s.version == 1 && s.checks.len() <= MAX_CHECKS)
            .map(|s| s.checks)
            .unwrap_or_default();
        let service = Arc::new_cyclic(|own| Self {
            own: own.clone(),
            ctx: ctx.clone(),
            runtime,
            paths,
            host,
            profile_path,
            cache_path,
            state: Mutex::new(state),
            checks: Mutex::new(checks),
            gates: Mutex::new(BTreeMap::new()),
            save_gate: tokio::sync::Mutex::new(()),
            cache_gate: tokio::sync::Mutex::new(()),
            generation: AtomicU64::new(0),
            flights: Mutex::new(BTreeMap::new()),
        });
        ctx.register_service(service.clone() as Arc<dyn ExecutionProfileResolver>);
        service
    }

    fn profile(&self, session: Option<&str>, cwd: &str) -> (String, Profile) {
        let state = self.state.lock();
        let project = format!("project:{}", hash(&json!(cwd)));
        for key in session
            .map(|s| format!("session:{s}"))
            .into_iter()
            .chain([project, "global".into()])
        {
            if let Some(profile) = state.profiles.get(&key) {
                return (key, profile.clone());
            }
        }
        ("platform".into(), Profile::default())
    }

    fn shell(&self, prefs: &Preferences) -> Result<(ShellKind, Option<String>), String> {
        let kind = prefs.shell_kind.clone().unwrap_or(if cfg!(windows) {
            ShellKind::Powershell
        } else {
            ShellKind::Bash
        });
        if let Some(path) = &prefs.shell_path {
            return Ok((kind, Some(ensure_file(path.clone(), "Shell")?)));
        }
        let found = match kind {
            ShellKind::Powershell => dsh_shell::powershell::locate_powershell(),
            ShellKind::Bash => locate(if cfg!(windows) {
                &["bash.exe"]
            } else {
                &["bash"]
            }),
            ShellKind::Zsh => locate(if cfg!(windows) {
                &["zsh.exe"]
            } else {
                &["zsh"]
            }),
        };
        Ok((kind, found))
    }

    fn python(&self, prefs: &Preferences, cwd: &str) -> Result<Option<String>, String> {
        if let Some(path) = &prefs.python_path {
            return ensure_file(path.clone(), "Python").map(Some);
        }
        if prefs.use_project_python {
            let path = Path::new(cwd).join(if cfg!(windows) {
                ".venv/Scripts/python.exe"
            } else {
                ".venv/bin/python"
            });
            // Opting into this explicit, trusted project path must not silently fall back.
            return ensure_file(path.to_string_lossy().into_owned(), "项目 Python").map(Some);
        }
        if let Some(path) = std::env::var("DSH_PYTHON_COMMAND")
            .ok()
            .filter(|p| !p.trim().is_empty())
        {
            return ensure_file(path, "Python").map(Some);
        }
        Ok(self
            .paths
            .python_command()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| Path::new(p).is_file())
            .or_else(|| {
                locate(if cfg!(windows) {
                    &["python.exe", "python3.exe"]
                } else {
                    &["python3", "python"]
                })
            }))
    }

    pub(super) fn snapshot(&self, session: Option<&str>, cwd: &str) -> Result<Value, String> {
        let cwd = absolute_directory(cwd)?;
        let (source, profile) = self.profile(session, &cwd);
        let resolved = self.resolve(session, &cwd);
        let policy = self.policy(session)?;
        let runtime = match resolved {
            Ok(r) => {
                json!({"contextId":r.context_id,"shellPath":r.shell_path,"shellKind":r.shell_kind,"pythonPath":r.python_path,"toolchainPaths":r.toolchain_paths,"status":"located","launchStatus":"unknown"})
            }
            Err(error) => json!({"status":"error","error":error}),
        };
        let current_context = runtime["contextId"].clone();
        let policy_fingerprint = hash(&json!(format!("{policy:?}")));
        let candidates: Vec<_> = [ShellKind::Powershell, ShellKind::Bash, ShellKind::Zsh]
            .into_iter()
            .filter_map(|kind| {
                self.shell(&Preferences {
                    shell_kind: Some(kind.clone()),
                    ..Default::default()
                })
                .ok()
                .and_then(|(_, path)| {
                    path.map(|path| json!({"kind":kind.as_str(),"path":path,"status":"located"}))
                })
            })
            .collect();
        Ok(
            json!({"version":1,"revision":self.state.lock().revision,"profileRevision":profile.revision,"source":source,"preferences":profile.preferences,"effective":runtime,"permissionMode":policy.mode.as_str(),"permissionManagedSeparately":true,"workspace":cwd,"backendId":"local","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"shellCandidates":candidates,"existingTerminalsRequireRestart":true,"cache":self.checks.lock().values().filter(|c|fresh(c)).map(|c|c.value.clone()).filter(|c|c["sessionId"].as_str()==session && c["workspace"].as_str()==Some(cwd.as_str()) && c["contextId"]==current_context && c["policyFingerprint"]==policy_fingerprint).collect::<Vec<_>>() }),
        )
    }

    fn policy(&self, session: Option<&str>) -> Result<SandboxExecutionPolicy, String> {
        let policy = self
            .ctx
            .get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
            .ok_or("权限服务不可用")?;
        let session = match session {
            Some(id) => Some(Arc::new(
                self.ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .and_then(|s| s.get(&dsh_session::session_id(id)))
                    .ok_or("会话不存在")?,
            )),
            None => None,
        };
        Ok(policy.resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
            session,
            mode: None,
        }))
    }

    async fn save(
        &self,
        scope: &str,
        session: Option<&str>,
        cwd: &str,
        expected: u64,
        preferences: Option<Preferences>,
    ) -> Result<Value, String> {
        let cwd = absolute_directory(cwd)?;
        if let Some(p) = &preferences {
            validate_preferences(p)?;
        }
        let key = match scope {
            "global" => "global".into(),
            "project" => format!("project:{}", hash(&json!(cwd))),
            "session" => {
                self.policy(session)?;
                format!("session:{}", session.ok_or("未选择会话")?)
            }
            _ => return Err("仅支持全局、项目或会话范围".into()),
        };
        let _guard = self.save_gate.lock().await;
        let mut next = self.state.lock().clone();
        if next.revision != expected {
            return Err("配置已被其他窗口修改，请刷新后重试".into());
        }
        next.revision = next.revision.checked_add(1).ok_or("配置版本溢出")?;
        if let Some(preferences) = preferences {
            next.profiles.insert(
                key,
                Profile {
                    revision: next.revision,
                    preferences,
                },
            );
        } else {
            next.profiles.remove(&key);
        }
        if next.profiles.len() > MAX_PROFILES {
            return Err("环境配置数量达到上限，请移除不用的覆盖配置".into());
        }
        persist(&self.profile_path, &next).await?;
        *self.state.lock() = next;
        if let Some(skills) = self
            .ctx
            .get_typed::<Arc<dsh_skill::SkillRegistry>>("skills", false)
        {
            skills.refresh();
        }
        // A new revision cannot reuse old execution facts; in-flight operations retain snapshots.
        self.snapshot(session, &cwd)
    }

    pub(super) fn selected_path(
        &self,
        id: &str,
        session: Option<&str>,
        cwd: &str,
    ) -> Result<Option<String>, String> {
        let (_, profile) = self.profile(session, cwd);
        let prefs = profile.preferences;
        match id {
            "shell" | "pwsh" => self.shell(&prefs).map(|(_, p)| p),
            "python" => self.python(&prefs, cwd),
            "wps" => prefs
                .wps_path
                .map(|p| ensure_file(p, "WPS"))
                .transpose()
                .map(|p| p.or_else(crate::environment_capabilities::find_wps)),
            name => prefs
                .toolchain_paths
                .get(name)
                .map(|p| ensure_file(p.clone(), name))
                .transpose()
                .map(|p| {
                    p.or_else(|| {
                        locate(&[&format!(
                            "{name}{}",
                            if cfg!(windows) { ".exe" } else { "" }
                        )])
                    })
                }),
        }
    }

    async fn inspect(
        self: &Arc<Self>,
        id: &str,
        level: &str,
        module: Option<&str>,
        session: Option<&str>,
        cwd: &str,
        refresh: bool,
        signal: dsh_tools::AbortPredicate,
    ) -> Result<Value, String> {
        let cwd = absolute_directory(cwd)?;
        let (_, profile) = self.profile(session, &cwd);
        let (kind, _) = self.shell(&profile.preferences)?;
        let selected = SelectedProbe {
            path: self.selected_path(id, session, &cwd)?,
            policy: self.policy(session)?,
            context_id: self.resolve(session, &cwd)?.context_id,
            kind,
            profile_revision: profile.revision,
        };
        self.inspect_selected(id, level, module, session, &cwd, refresh, signal, selected)
            .await
    }

    async fn inspect_selected(
        self: &Arc<Self>,
        id: &str,
        level: &str,
        module: Option<&str>,
        session: Option<&str>,
        cwd: &str,
        refresh: bool,
        signal: dsh_tools::AbortPredicate,
        selected: SelectedProbe,
    ) -> Result<Value, String> {
        if signal() {
            return Err("环境检查已取消".into());
        }
        let key = hash(
            &json!({"id":id,"level":level,"module":module,"session":session,"cwd":cwd,"refresh":refresh,"policy":format!("{:?}",selected.policy),"context":selected.context_id,"path":selected.path,"generation":self.generation.load(Ordering::Acquire),"environment":self.host.environment()}),
        );
        let (flight, coalesced) = {
            let mut flights = self.flights.lock();
            flights.retain(|_, f| !f.cancelled.load(Ordering::Acquire));
            let flight = flights
                .entry(key.clone())
                .or_insert_with(|| {
                    let service = self.clone();
                    let id = id.to_string();
                    let level = level.to_string();
                    let module = module.map(str::to_string);
                    let session = session.map(str::to_string);
                    let cwd = cwd.to_string();
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let flag = cancelled.clone();
                    let result = async move {
                        service
                            .inspect_inner(
                                &id,
                                &level,
                                module.as_deref(),
                                session.as_deref(),
                                &cwd,
                                refresh,
                                Arc::new(move || flag.load(Ordering::Acquire)),
                                selected,
                            )
                            .await
                    }
                    .boxed()
                    .shared();
                    Arc::new(ValidationFlight {
                        result,
                        waiters: AtomicUsize::new(0),
                        cancelled,
                    })
                })
                .clone();
            let coalesced = flight.waiters.fetch_add(1, Ordering::AcqRel) > 0;
            (flight, coalesced)
        };
        let waiter = ValidationWaiter(flight.clone());
        let mut result = tokio::select! {result=flight.result.clone()=>result,_=cancelled(signal)=>Err("环境检查已取消".into())};
        if flight.result.peek().is_some() || flight.waiters.load(Ordering::Acquire) == 1 {
            let mut flights = self.flights.lock();
            if flights
                .get(&key)
                .is_some_and(|active| Arc::ptr_eq(active, &flight))
            {
                flights.remove(&key);
            }
        }
        drop(waiter);
        if coalesced {
            if let Ok(value) = &mut result {
                value["cacheHit"] = json!(true);
                value["coalesced"] = json!(true);
            }
        }
        result
    }

    async fn inspect_inner(
        &self,
        id: &str,
        level: &str,
        module: Option<&str>,
        session: Option<&str>,
        cwd: &str,
        refresh: bool,
        signal: dsh_tools::AbortPredicate,
        selected: SelectedProbe,
    ) -> Result<Value, String> {
        let generation = self.generation.load(Ordering::Acquire);
        if id != "shell" && !IDS.contains(&id) {
            return Err("未知能力".into());
        }
        if !matches!(level, "locate" | "launch" | "dependency" | "feature") {
            return Err("未知检查级别".into());
        }
        if matches!(level, "dependency" | "feature")
            && (id != "python" || !module.is_some_and(|m| MODULES.contains(&m)))
        {
            return Err("依赖检查仅支持固定 Python 模块目录".into());
        }
        if level == "feature" && module != Some("cv2") {
            return Err("该能力没有注册功能探针".into());
        }
        if id == "wps" && level != "locate" {
            return Err("WPS 启动和转换验证必须使用 Office 自动化工具".into());
        }
        let cwd = absolute_directory(cwd)?;
        let path = selected.path;
        let policy = selected.policy;
        let key = hash(
            &json!({"host":self.host.environment(),"context":selected.context_id,"backend":"local","session":session,"cwd":cwd,"policy":format!("{policy:?}"),"id":id,"level":level,"module":module,"file":path.as_deref().map(file_stamp),"venv":file_stamp(&Path::new(&cwd).join(".venv/pyvenv.cfg").to_string_lossy())}),
        );
        let gate = {
            let mut gates = self.gates.lock();
            if gates.len() > MAX_CHECKS * 2 {
                gates.retain(|_, gate| Arc::strong_count(gate) > 1);
            }
            gates
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let previous = self
            .checks
            .lock()
            .get(&key)
            .map(|c| c.value["checkId"].clone());
        let _guard = tokio::select! { guard = gate.lock() => guard, _ = cancelled(signal.clone()) => return Err("环境检查已取消".into()) };
        if let Some(cached) = self
            .checks
            .lock()
            .get(&key)
            .filter(|c| fresh(c) && (!refresh || Some(c.value["checkId"].clone()) != previous))
            .cloned()
        {
            let mut value = cached.value;
            value["cacheHit"] = json!(true);
            return Ok(value);
        }
        if signal() {
            return Err("环境检查已取消".into());
        }
        let mut value = json!({"id":id,"level":level,"module":module,"path":path,"executionWorld":"selected_environment","backendId":"local","environmentFingerprint":key,"contextId":selected.context_id,"profileRevision":selected.profile_revision,"permissionMode":policy.mode.as_str(),"moduleScope":"selected isolated interpreter; workspace and user-site imports excluded","status":"missing","cacheHit":false,"checkedAt":timestamp(),"checkId":uuid::Uuid::new_v4().to_string(),"workspace":cwd,"sessionId":session});
        value["policyFingerprint"] = json!(hash(&json!(format!("{policy:?}"))));
        if let Some(path) = path {
            if level == "locate" {
                value["status"] = json!("located");
            } else {
                let args = probe_args(id, selected.kind, level, module)?;
                match self
                    .run_probe(&path, args, &cwd, &policy, signal.clone())
                    .await
                {
                    Ok(output) => {
                        value["status"] = json!("ready");
                        value["output"] = json!(output);
                    }
                    Err((status, error)) => {
                        value["status"] = json!(status);
                        value["error"] = json!(error.chars().take(512).collect::<String>());
                    }
                }
            }
        }
        if signal() {
            return Err("环境检查已取消".into());
        }
        let _cache = self.cache_gate.lock().await;
        if generation != self.generation.load(Ordering::Acquire) {
            value["invalidatedReason"] = json!("cache_refreshed_during_probe");
            return Ok(value);
        }
        {
            let mut checks = self.checks.lock();
            if checks.len() >= MAX_CHECKS {
                if let Some(oldest) = checks
                    .iter()
                    .min_by_key(|(_, c)| c.checked_at)
                    .map(|(k, _)| k.clone())
                {
                    checks.remove(&oldest);
                }
            }
            checks.insert(
                key,
                Check {
                    checked_at: timestamp(),
                    value: value.clone(),
                },
            );
        }
        let checks = self.checks.lock().clone();
        if persist(&self.cache_path, &CheckFile { version: 1, checks })
            .await
            .is_err()
        {
            value["cacheWarning"] = json!("持久缓存不可用，当前进程仍复用检查结果");
        }
        Ok(value)
    }

    async fn run_probe(
        &self,
        path: &str,
        args: Vec<String>,
        cwd: &str,
        policy: &SandboxExecutionPolicy,
        signal: dsh_tools::AbortPredicate,
    ) -> Result<String, (&'static str, String)> {
        let argv: Vec<_> = std::iter::once(path.to_string()).chain(args).collect();
        let (argv, enforcement) = if policy.mode == SandboxMode::DangerFullAccess {
            (argv, None)
        } else {
            let sandbox = self
                .ctx
                .get_typed::<Arc<dyn SandboxProvider>>("sandbox", false)
                .ok_or(("unknown", "沙箱服务不可用".into()))?;
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(120), sandbox.prepare(policy)) => {
                    result.map_err(|_| ("setup_timeout", "[SANDBOX_SETUP_TIMEOUT] runtime preparation timed out; probe not dispatched".into()))?
                        .map_err(|error| ("setup_failed", error))?;
                }
                _ = cancelled(signal.clone()) => return Err(("cancelled", "环境准备已取消；探测未启动".into())),
            }
            let confined = sandbox
                .confine_with_startup(
                    &argv,
                    &SandboxPolicy {
                        mode: if policy.mode == SandboxMode::ReadOnly {
                            ConfinedSandboxMode::ReadOnly
                        } else {
                            ConfinedSandboxMode::WorkspaceWrite
                        },
                        workspace_root: policy.workspace_root.clone(),
                        read_only_roots: policy.read_only_roots.clone(),
                        session_id: policy.session_id.clone(),
                    },
                )
                .map_err(|e| ("permission_denied", e.to_string()))?;
            (confined.argv.clone(), Some(confined))
        };
        let child = self
            .runtime
            .spawn(SubprocessSpawnSpec {
                argv,
                cwd: cwd.into(),
                env: Some(vec![
                    ("PYTHONSTARTUP".into(), None),
                    ("NODE_OPTIONS".into(), None),
                    ("PYTHONUTF8".into(), Some("1".into())),
                ]),
                signal: Some(signal.clone()),
                grace_ms: 200,
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Ignore,
                    stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 4096,
                        spill: None,
                    }),
                    stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 2048,
                        spill: None,
                    }),
                },
            })
            .map_err(|e| ("error", e))?;
        let _kill = ChildGuard(child.clone());
        if let Some(startup) = enforcement
            .as_ref()
            .and_then(|confined| confined.startup.as_ref())
        {
            let ready = async {
                loop {
                    if startup
                        .is_ready()
                        .map_err(|error| ("setup_failed", error))?
                    {
                        return Ok(());
                    }
                    tokio::select! {
                        result = child.done() => {
                            // The ready event and exit may become observable together.
                            if startup.is_ready().map_err(|error| ("setup_failed", error))? { return Ok(()); }
                            let detail = child.collected().stderr.map(|reader| reader.read_from(0).text).unwrap_or_default();
                            return Err(("setup_failed", format!("[SANDBOX_SETUP_FAILED] runner exited before readiness ({result:?}): {detail}")));
                        }
                        _ = cancelled(signal.clone()) => return Err(("cancelled", "环境启动已取消".into())),
                        _ = tokio::time::sleep(Duration::from_millis(15)) => {},
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(120), ready).await
                .map_err(|_| ("setup_timeout", format!("[SANDBOX_SETUP_TIMEOUT] phase={}; runner readiness not confirmed; inspect startup evidence before retrying", startup.phase())))??;
        }
        let outcome = tokio::select! {
            output = tokio::time::timeout(Duration::from_secs(5),child.done()) => output.map_err(|_|("timed_out","[ENVIRONMENT_PROBE_TIMEOUT] 程序已启动，探测运行超过五秒".into()))?.map_err(|e|("error",e))?,
            _ = cancelled(signal) => return Err(("unknown","环境检查已取消".into())),
        };
        let collected = child.collected();
        let out = collected.stdout.map(|r| r.read_from(0));
        let err = collected.stderr.map(|r| r.read_from(0));
        if outcome.exit_code != Some(0) {
            let stderr = err.as_ref().map(|r| r.text.as_str()).unwrap_or_default();
            let denied = enforcement.as_ref().is_some_and(|c| {
                dsh_shell::ShellSandboxInfo::observe(policy.mode, c, outcome.exit_code, stderr)
                    .denied
            });
            return Err((
                if denied { "permission_denied" } else { "error" },
                format!("exit={:?}; {}{}", outcome.exit_code, stderr,
                    dsh_shell::application_diagnostics(outcome.exit_code, stderr).iter()
                        .map(|entry| format!("\n[diagnostic: {}; suspected application stderr]\n[recovery: {}]", entry.category, entry.recovery()))
                        .collect::<String>()),
            ));
        }
        let out = out.ok_or(("error", "检查没有输出".into()))?;
        if out.lossy {
            return Err(("error", "检查输出超过限额".into()));
        }
        if out.text.trim().is_empty() {
            return Err(("error", "检查返回空输出".into()));
        }
        Ok(out.text.trim().chars().take(1024).collect())
    }
}

fn fresh(check: &Check) -> bool {
    let ttl = if matches!(check.value["status"].as_str(), Some("ready" | "located")) {
        300
    } else {
        60
    };
    let now = timestamp();
    now >= check.checked_at && now - check.checked_at < ttl
}
struct ChildGuard(Arc<dyn SubprocessHandle>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.0.terminate();
    }
}
async fn cancelled(signal: dsh_tools::AbortPredicate) {
    loop {
        if signal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn probe_args(
    id: &str,
    shell: ShellKind,
    level: &str,
    module: Option<&str>,
) -> Result<Vec<String>, String> {
    let args = match id {
        "shell" | "pwsh" if shell == ShellKind::Powershell => vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "$ProgressPreference='SilentlyContinue'; $PSVersionTable.PSVersion.ToString(); 'LanguageMode=' + $ExecutionContext.SessionState.LanguageMode; Get-ExecutionPolicy -List | ForEach-Object { 'ExecutionPolicy.' + $_.Scope + '=' + $_.ExecutionPolicy }".into(),
        ],
        "shell" | "pwsh" => vec!["--version".into()],
        "python" => {
            let code = if level == "dependency" {
                format!(
                    "import importlib; m=importlib.import_module({:?}); print(getattr(m,'__version__','imported'))",
                    module.ok_or("模块缺失")?
                )
            } else if level == "feature" {
                "import cv2,numpy as n; a=n.zeros((2,2,3),n.uint8); ok,b=cv2.imencode('.png',a); assert ok; c=cv2.imdecode(b,cv2.IMREAD_COLOR); assert c is not None and c.shape==(2,2,3); print('image-codec-ready')".into()
            } else {
                "import sys; print(sys.version.split()[0])".into()
            };
            vec!["-I".into(), "-B".into(), "-c".into(), code]
        }
        "ffmpeg" => vec!["-version".into()],
        _ => vec!["--version".into()],
    };
    Ok(args)
}

impl ExecutionProfileResolver for ExecutionProfiles {
    fn validate(
        &self,
        request: dsh_shell::ExecutionValidationRequest,
    ) -> BoxFuture<'static, Result<(), String>> {
        let own = self.own.clone();
        Box::pin(async move {
            let service = own.upgrade().ok_or("环境服务已关闭")?;
            let policy = request.sandbox_policy.ok_or("缺少执行权限快照")?;
            let kind = match request.shell_kind.as_deref() {
                Some("bash") => ShellKind::Bash,
                Some("zsh") => ShellKind::Zsh,
                _ => ShellKind::Powershell,
            };
            let selected = SelectedProbe {
                path: request.executable,
                policy,
                context_id: request.execution_context_id.ok_or("缺少执行环境快照")?,
                kind,
                profile_revision: 0,
            };
            let value = service
                .inspect_selected(
                    &request.capability,
                    "launch",
                    None,
                    request.session_id.as_deref(),
                    &request.workdir,
                    false,
                    request.signal.unwrap_or_else(|| Arc::new(|| false)),
                    selected,
                )
                .await?;
            if value["status"] == "ready" {
                Ok(())
            } else {
                Err(format!(
                    "[ENVIRONMENT_VALIDATION_FAILED] {}: {} {}",
                    request.capability,
                    value["status"].as_str().unwrap_or("unknown"),
                    value["error"]
                        .as_str()
                        .unwrap_or("所选程序不可用；请检查运行环境设置")
                ))
            }
        })
    }

    fn report_failure(&self, context_id: &str, capability: &str) {
        self.checks.lock().retain(|_, check| {
            !(check.value["contextId"] == context_id && check.value["id"] == capability)
        });
        // Existing unrelated facts remain valid; fence any late result from the failed operation.
        self.generation.fetch_add(1, Ordering::AcqRel);
        let own = self.own.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(service) = own.upgrade() {
                    let _guard = service.cache_gate.lock().await;
                    let checks = service.checks.lock().clone();
                    let _ = persist(&service.cache_path, &CheckFile { version: 1, checks }).await;
                }
            });
        }
    }

    fn resolve(
        &self,
        session_id: Option<&str>,
        workdir: &str,
    ) -> Result<ResolvedExecutionProfile, String> {
        let cwd = absolute_directory(workdir)?;
        let (_, profile) = self.profile(session_id, &cwd);
        let (kind, shell_path) = self.shell(&profile.preferences)?;
        let python_path = self.python(&profile.preferences, &cwd)?;
        let mut toolchain_paths = profile.preferences.toolchain_paths.clone();
        for (name, path) in &mut toolchain_paths {
            *path = ensure_file(path.clone(), name)?;
        }
        for name in ["node", "rustc", "cargo", "git", "rg", "ffmpeg"] {
            if !toolchain_paths.contains_key(name) {
                if let Some(path) = self.selected_path(name, session_id, &cwd)? {
                    toolchain_paths.insert(name.into(), path);
                }
            }
        }
        let context_id = hash(
            &json!({"host":self.host.environment(),"profile":profile,"cwd":cwd,"shell":shell_path.as_deref().map(file_stamp),"python":python_path.as_deref().map(file_stamp),"toolchains":toolchain_paths.iter().map(|(name,path)| (name,file_stamp(path))).collect::<BTreeMap<_,_>>()}),
        );
        Ok(ResolvedExecutionProfile {
            context_id,
            shell_path,
            shell_kind: kind.as_str().into(),
            python_path,
            toolchain_paths,
        })
    }
}

impl ExecutionProfiles {
    /// Registers a bounded, fixed-capability diagnostic tool; no arbitrary scripts are accepted.
    pub(super) fn install_tools(
        self: &Arc<Self>,
        ctx: &Context,
        tools: &Arc<ToolRuntime>,
        prompt: &Arc<dsh_system_prompt::SystemPrompt>,
    ) -> Result<(), String> {
        let service = self.clone();
        tools.register(ctx, ToolDefinition {
            name: "environment_validate".into(), description: "Validate selected shell, interpreter or a fixed capability inside the current execution policy. This is separate from environment_probe host facts. Request only needed capabilities, reuse cached facts; use refresh after a relevant failure. Python dependency imports use the selected isolated interpreter; cv2 feature checks an in-memory codec. WPS supports locate only. Never changes permissions or installs software.".into(),
            parameters: json!({"type":"object","properties":{"name":{"type":"string","enum":["shell","python","node","git","rg","pwsh","ffmpeg","wps","rustc","cargo"]},"level":{"type":"string","enum":["locate","launch","dependency","feature"]},"module":{"type":"string","enum":MODULES},"refresh":{"type":"boolean"}},"required":["name"],"additionalProperties":false}),
            output: ToolOutputDefinition { schema: json!({"type":"object"}), render: Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text {text:value.to_string()}])), presentation_meta: None }, timeout_ms: Some(15000), is_concurrency_safe: Some(Arc::new(|_|true)),
            execute: Arc::new(move |args,exec| { let service=service.clone(); let args=args.clone(); let signal=exec.signal.lock().clone(); let agent=exec.agent.clone(); Box::pin(async move {
                let session=agent.as_ref().map(|a|a.session().header().id.as_str());
                let cwd=agent.as_ref().and_then(|a|a.session().header().cwd.as_deref()).map(str::to_string).unwrap_or_else(||std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned());
                service.inspect(args["name"].as_str().unwrap_or_default(),args["level"].as_str().unwrap_or("launch"),args["module"].as_str(),session,&cwd,args["refresh"].as_bool().unwrap_or(false),signal).await.map_err(ToolBodyError::plain)
            }) }), finalize_content: None, present_call: None, present_result: None,
        })?;
        let service = self.clone();
        let weak = Arc::downgrade(tools);
        prompt.context(ctx,dsh_system_prompt::PromptContext { name:"environment:execution-profile".into(),order:82.0,text:dsh_system_prompt::PromptText::Provider(Arc::new(move |assembly| {
            if !weak.upgrade().is_some_and(|t|t.get("environment_validate",assembly.scope.as_ref()).is_some()) {return String::new();}
            let session=assembly.field_str("sessionId");
            let cwd=session.and_then(|id|service.ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false).and_then(|s|s.get(&dsh_session::session_id(id)))).and_then(|s|s.header().cwd.clone()).unwrap_or_else(||std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned());
            match service.snapshot(session,&cwd) {Ok(snapshot)=>format!("Selected execution environment (location is not proof of runtime access; use environment_validate once when needed): {}",snapshot).chars().take(4096).collect(),Err(e)=>format!("Selected execution environment unavailable: {e}")}
        })) });
        Ok(())
    }

    pub(super) fn register(
        self: &Arc<Self>,
        server: &Arc<WebServer>,
        allow_remote: bool,
    ) -> RouteDisposer {
        let service = self.clone();
        server.register(WebRoute {
            kind: WebRouteKind::Exact,
            path: "/__dsh-environment".into(),
            handler: Arc::new(move |request| {
                let service = service.clone();
                Box::pin(async move {
                    let trusted = crate::trusted_web_request(&request, allow_remote);
                    let is_post = request.method() == http::Method::POST
                        && request
                            .headers()
                            .get("content-type")
                            .and_then(|h| h.to_str().ok())
                            .is_some_and(|s| s.starts_with("application/json"));
                    let result = if !trusted {
                        Err("禁止跨站访问".into())
                    } else if !is_post {
                        Err("需要 JSON POST 请求".into())
                    } else {
                        match axum::body::to_bytes(
                            axum::body::Body::new(request.into_body()),
                            16 * 1024,
                        )
                        .await
                        {
                            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                                Ok(args) => service.handle(args).await,
                                Err(_) => Err("无效 JSON".into()),
                            },
                            Err(_) => Err("请求超过大小限制".into()),
                        }
                    };
                    let (status, payload) = match result {
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
                        .body(axum::body::Body::from(payload.to_string()))
                        .expect("environment response"))
                })
            }),
        })
    }

    async fn handle(self: &Arc<Self>, args: Value) -> Result<Value, String> {
        let session = args["sessionId"].as_str().filter(|s| !s.is_empty());
        // Settings requests can arrive after automatic idle retirement. Use
        // the same admission/resume boundary as control RPCs, and release the
        // lease after the response instead of retaining a generation forever.
        let _lease = match (
            session,
            self.ctx
                .get_typed::<Arc<dsh_host_apiproxy::ApiProxyService>>("apiProxy", false),
        ) {
            (Some(id), Some(api)) => Some(api.resolve_control_agent(id).await?),
            _ => None,
        };
        let default_cwd = session
            .and_then(|id| {
                self.ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .and_then(|s| s.get(&dsh_session::session_id(id)))
            })
            .and_then(|s| s.header().cwd.clone())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let cwd = args["cwd"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_cwd);
        match args["action"].as_str().unwrap_or("describe") {
            "describe" => self.snapshot(session, cwd),
            "save" | "reset" => {
                self.save(
                    args["scope"].as_str().unwrap_or("global"),
                    session,
                    cwd,
                    args["expectedRevision"].as_u64().ok_or("缺少配置版本")?,
                    if args["action"] == "reset" {
                        None
                    } else {
                        Some(
                            serde_json::from_value(args["preferences"].clone())
                                .map_err(|e| e.to_string())?,
                        )
                    },
                )
                .await
            }
            "probe" => {
                self.inspect(
                    args["name"].as_str().unwrap_or("shell"),
                    args["level"].as_str().unwrap_or("launch"),
                    args["module"].as_str(),
                    session,
                    cwd,
                    args["refresh"].as_bool().unwrap_or(false),
                    Arc::new(|| false),
                )
                .await
            }
            "probeHost" => self
                .host
                .inspect(
                    args["name"].as_str().unwrap_or("python"),
                    args["refresh"].as_bool().unwrap_or(false),
                    Arc::new(|| false),
                )
                .await
                .map_err(|e| format!("{e:?}")),
            "clearCache" => {
                let _guard = self.cache_gate.lock().await;
                persist(
                    &self.cache_path,
                    &CheckFile {
                        version: 1,
                        checks: BTreeMap::new(),
                    },
                )
                .await?;
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.checks.lock().clear();
                self.snapshot(session, cwd)
            }
            _ => Err("未知环境操作".into()),
        }
    }
}

#[cfg(test)]
#[path = "execution_profiles_tests.rs"]
mod tests;
