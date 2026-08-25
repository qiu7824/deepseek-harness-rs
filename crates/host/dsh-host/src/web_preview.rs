//! Session-authorized workspace preview routes for the Web GUI.
//!
//! Every operation is rooted by a durable Workspace → Session association.
//! The browser never supplies a filesystem root or an executable command.
//! Canonicalization happens before a path is accepted so `..` and symlink
//! escapes fail closed. Project execution uses a short-lived, one-shot
//! challenge bound to the Host-detected argv and canonical Workspace.

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use dsh_host_webserver::{
    RouteDisposer, WebHandlerError, WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer,
};
use dsh_sandbox::{ConfinedSandboxMode, SandboxEnforcement, SandboxPolicy, SandboxProvider};
use dsh_session::{SessionId, session_id};
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use dsh_workspace::WorkspaceRegistry;
use http::{Method, Response, StatusCode, header};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const ROUTE: &str = "/__dsh-preview";
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONTROL_BYTES: usize = 16 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 1_000;
const MAX_LOG_LINES: usize = 400;
const CHALLENGE_TTL_MS: u64 = 60_000;

const ANNOTATION_BRIDGE: &str = r#"<script data-dsh-preview-bridge>(function(){
const SOURCE='dsh-web-preview-rs';let enabled=false,box=null,last=null;
function ensure(){if(box)return box;box=document.createElement('div');box.style.cssText='position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #4f8cff;background:rgba(79,140,255,.12);display:none';document.documentElement.appendChild(box);return box}
function selector(el){if(!el||el.nodeType!==1)return'';if(el.id)return'#'+CSS.escape(el.id);const out=[];for(let n=el;n&&n.nodeType===1&&out.length<6;n=n.parentElement){let s=n.localName||'*';const cls=[...n.classList].filter(x=>!x.startsWith('dsh-')).slice(0,2);if(cls.length)s+='.'+cls.map(CSS.escape).join('.');if(n.parentElement){const same=[...n.parentElement.children].filter(x=>x.localName===n.localName);if(same.length>1)s+=':nth-of-type('+(same.indexOf(n)+1)+')'}out.unshift(s)}return out.join(' > ')}
function point(el){last=el;const r=el.getBoundingClientRect(),b=ensure();b.style.display='block';b.style.left=r.left+'px';b.style.top=r.top+'px';b.style.width=Math.max(0,r.width)+'px';b.style.height=Math.max(0,r.height)+'px'}
function move(e){if(!enabled)return;const el=e.target;if(el!==box&&el instanceof Element)point(el)}
function click(e){if(!enabled||!(e.target instanceof Element))return;e.preventDefault();e.stopImmediatePropagation();const el=e.target;point(el);parent.postMessage({source:SOURCE,type:'element-selected',selector:selector(el),text:(el.innerText||el.textContent||'').trim().slice(0,1600),html:el.outerHTML.slice(0,5000),url:location.href},'*')}
addEventListener('mousemove',move,true);addEventListener('click',click,true);addEventListener('message',e=>{if(e.source!==parent||!e.data||e.data.source!==SOURCE)return;if(e.data.type==='mark-mode'){enabled=!!e.data.enabled;document.documentElement.style.cursor=enabled?'crosshair':'';if(!enabled&&box)box.style.display='none'}},true);parent.postMessage({source:SOURCE,type:'bridge-ready'},'*');
})();</script>"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody<'a> {
    error: &'a str,
    message: String,
}

#[derive(Clone)]
struct ProjectSpec {
    kind: &'static str,
    name: &'static str,
    manifest: Option<&'static str>,
    argv: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectHint {
    kind: &'static str,
    name: &'static str,
    manifest: Option<&'static str>,
    command: Option<String>,
    runnable: bool,
}

impl From<ProjectSpec> for ProjectHint {
    fn from(spec: ProjectSpec) -> Self {
        let command = spec.argv.as_ref().map(|argv| render_argv(argv));
        Self {
            kind: spec.kind,
            name: spec.name,
            manifest: spec.manifest,
            runnable: spec.argv.is_some(),
            command,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaBody {
    session_id: String,
    workspace_title: String,
    capabilities: [&'static str; 7],
    max_text_bytes: u64,
    max_media_bytes: u64,
    max_upload_bytes: usize,
    site_token: String,
    project: ProjectHint,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryEntry {
    name: String,
    path: String,
    kind: &'static str,
    size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryBody {
    path: String,
    entries: Vec<DirectoryEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    session_id: String,
    #[serde(default)]
    challenge: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareBody {
    challenge: String,
    project: ProjectHint,
    command: String,
    expires_at: u64,
    warning: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectStatusBody {
    status: &'static str,
    kind: Option<String>,
    command: Option<String>,
    logs: Vec<String>,
    url: Option<String>,
    exit_code: Option<i32>,
    signal: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadBody {
    path: String,
    size: usize,
}

struct RunChallenge {
    session_id: String,
    root: PathBuf,
    kind: String,
    argv: Vec<String>,
    expires_at: u64,
}

#[derive(Default)]
struct ProjectRuntimeState {
    stdout_offset: u64,
    stderr_offset: u64,
    logs: VecDeque<String>,
    url: Option<String>,
    exit_code: Option<i32>,
    signal: Option<String>,
    error: Option<String>,
    settled: bool,
    stopping: bool,
}

struct ManagedProject {
    root: PathBuf,
    kind: String,
    command: String,
    handle: Arc<dyn SubprocessHandle>,
    runtime: Arc<Mutex<ProjectRuntimeState>>,
}

struct PreviewState {
    challenges: HashMap<String, RunChallenge>,
    projects: HashMap<String, Arc<ManagedProject>>,
}

struct PreviewService {
    registry: Arc<WorkspaceRegistry>,
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox: Arc<dyn SandboxProvider>,
    site_token: String,
    state: Mutex<PreviewState>,
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        for project in self.state.get_mut().projects.values() {
            project.handle.terminate();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn response(status: StatusCode, content_type: &str, body: impl Into<Body>) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body.into())
        .expect("preview response")
}

fn json_response(status: StatusCode, value: &impl Serialize) -> WebResponse {
    response(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(value).unwrap_or_else(|_| b"{\"error\":\"internal\"}".to_vec()),
    )
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> WebResponse {
    json_response(
        status,
        &ErrorBody {
            error: code,
            message: message.into(),
        },
    )
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16).ok_or(())?;
                let low = (bytes[index + 2] as char).to_digit(16).ok_or(())?;
                output.push((high * 16 + low) as u8);
                index += 3;
            }
            b'%' => return Err(()),
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| ())
}

fn query_value(query: Option<&str>, key: &str) -> Result<Option<String>, ()> {
    for pair in query.unwrap_or_default().split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(raw_key)? == key {
            return percent_decode(raw_value).map(Some);
        }
    }
    Ok(None)
}

fn denied_name(name: &str) -> bool {
    [".git", ".hg", ".svn", "node_modules", "target"]
        .iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked))
        || name.eq_ignore_ascii_case(".env")
        || name.to_ascii_lowercase().starts_with(".env.")
}

fn safe_relative(value: &str) -> Option<PathBuf> {
    if value.as_bytes().contains(&0) {
        return None;
    }
    let value = value.replace('\\', "/");
    let path = Path::new(&value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return (value.is_empty() || value == ".").then(PathBuf::new);
    }
    if path.components().any(|part| {
        let Component::Normal(name) = part else {
            return true;
        };
        denied_name(&name.to_string_lossy())
    }) {
        return None;
    }
    Some(path.to_path_buf())
}

fn python_executable() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

fn node_script(root: &Path) -> Option<&'static str> {
    let path = root.join("package.json");
    if std::fs::metadata(&path).ok()?.len() > 1024 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let scripts = value.get("scripts")?.as_object()?;
    if scripts.get("dev").is_some_and(serde_json::Value::is_string) {
        Some("dev")
    } else if scripts
        .get("start")
        .is_some_and(serde_json::Value::is_string)
    {
        Some("start")
    } else {
        None
    }
}

fn detect_project(root: &Path) -> ProjectSpec {
    if root.join("package.json").is_file() {
        let argv = node_script(root)
            .map(|script| vec!["npm".to_string(), "run".to_string(), script.to_string()]);
        return ProjectSpec {
            kind: "node",
            name: "Node 项目",
            manifest: Some("package.json"),
            argv,
        };
    }
    if root.join("Cargo.toml").is_file() {
        return ProjectSpec {
            kind: "rust",
            name: "Rust 项目",
            manifest: Some("Cargo.toml"),
            argv: Some(vec!["cargo".to_string(), "run".to_string()]),
        };
    }
    if root.join("go.mod").is_file() {
        return ProjectSpec {
            kind: "go",
            name: "Go 项目",
            manifest: Some("go.mod"),
            argv: Some(vec!["go".to_string(), "run".to_string(), ".".to_string()]),
        };
    }
    if root.join("manage.py").is_file() {
        return ProjectSpec {
            kind: "python",
            name: "Django 项目",
            manifest: Some("manage.py"),
            argv: Some(vec![
                python_executable().to_string(),
                "manage.py".to_string(),
                "runserver".to_string(),
                "127.0.0.1:8081".to_string(),
            ]),
        };
    }
    if root.join("app.py").is_file() {
        return ProjectSpec {
            kind: "python",
            name: "Python 项目",
            manifest: Some("app.py"),
            argv: Some(vec![python_executable().to_string(), "app.py".to_string()]),
        };
    }
    if root.join("pyproject.toml").is_file() || root.join("requirements.txt").is_file() {
        return ProjectSpec {
            kind: "python",
            name: "Python 静态服务",
            manifest: root
                .join("pyproject.toml")
                .is_file()
                .then_some("pyproject.toml")
                .or(Some("requirements.txt")),
            argv: Some(vec![
                python_executable().to_string(),
                "-m".to_string(),
                "http.server".to_string(),
                "0".to_string(),
                "--bind".to_string(),
                "127.0.0.1".to_string(),
            ]),
        };
    }
    ProjectSpec {
        kind: "static",
        name: "静态工作区",
        manifest: None,
        argv: None,
    }
}

fn render_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|part| {
            if part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-._/:".contains(ch))
            {
                part.clone()
            } else {
                format!("{:?}", part)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn workspace_for_session(
    registry: &WorkspaceRegistry,
    id: &SessionId,
) -> Result<Option<dsh_workspace::Workspace>, String> {
    Ok(registry.list()?.into_iter().find(|workspace| {
        workspace
            .session_ids()
            .iter()
            .any(|candidate| candidate == id)
    }))
}

async fn workspace_root(
    registry: &WorkspaceRegistry,
    session: &SessionId,
) -> Result<(dsh_workspace::Workspace, PathBuf), WebResponse> {
    let workspace = workspace_for_session(registry, session)
        .map_err(|message| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "workspace-failed",
                message,
            )
        })?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "session-not-found",
                "Session 没有关联的工作区",
            )
        })?;
    let root = tokio::fs::canonicalize(workspace.path())
        .await
        .map_err(|_| {
            error(
                StatusCode::NOT_FOUND,
                "workspace-not-found",
                "工作区目录不存在",
            )
        })?;
    Ok((workspace, root))
}

async fn authorized_path(
    registry: &WorkspaceRegistry,
    session: &SessionId,
    relative: &str,
) -> Result<(dsh_workspace::Workspace, PathBuf, PathBuf), WebResponse> {
    let relative = safe_relative(relative).ok_or_else(|| {
        error(
            StatusCode::BAD_REQUEST,
            "unsafe-path",
            "预览路径不安全或位于受保护目录",
        )
    })?;
    let (workspace, root) = workspace_root(registry, session).await?;
    let target = tokio::fs::canonicalize(root.join(&relative))
        .await
        .map_err(|_| error(StatusCode::NOT_FOUND, "file-not-found", "文件或目录不存在"))?;
    if !target.starts_with(&root) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "path-escape",
            "路径越出了工作区",
        ));
    }
    Ok((workspace, root, target))
}

fn execution_path(path: &Path) -> String {
    let rendered = path.to_string_lossy();
    #[cfg(windows)]
    {
        return rendered
            .strip_prefix(r"\\?\")
            .unwrap_or(&rendered)
            .to_string();
    }
    #[cfg(not(windows))]
    rendered.into_owned()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn inject_bridge(mut html: String) -> String {
    if html.contains("data-dsh-preview-bridge") {
        return html;
    }
    if let Some(index) = html.to_ascii_lowercase().rfind("</body>") {
        html.insert_str(index, ANNOTATION_BRIDGE);
    } else {
        html.push_str(ANNOTATION_BRIDGE);
    }
    html
}

fn local_preview_url(text: &str) -> Option<String> {
    for marker in [
        "http://127.0.0.1:",
        "http://localhost:",
        "https://127.0.0.1:",
        "https://localhost:",
    ] {
        let Some(start) = text.find(marker) else {
            continue;
        };
        let candidate: String = text[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || ":/.-_[]?=&%".contains(*ch))
            .collect();
        let authority = candidate.split('/').nth(2)?;
        let port = authority.rsplit_once(':')?.1.parse::<u16>().ok()?;
        if port != 0 {
            return Some(candidate.trim_end_matches(['.', ',', ';']).to_string());
        }
    }
    None
}

fn append_logs(project: &ManagedProject) {
    let collected = project.handle.collected();
    let mut state = project.runtime.lock();
    let mut chunks = Vec::new();
    if let Some(reader) = collected.stdout {
        let read = reader.read_from(state.stdout_offset);
        state.stdout_offset = read.next_offset;
        if !read.text.is_empty() {
            chunks.push(read.text);
        }
    }
    if let Some(reader) = collected.stderr {
        let read = reader.read_from(state.stderr_offset);
        state.stderr_offset = read.next_offset;
        if !read.text.is_empty() {
            chunks.push(read.text);
        }
    }
    for chunk in chunks {
        if state.url.is_none() {
            state.url = local_preview_url(&chunk);
        }
        for line in chunk.lines().filter(|line| !line.trim().is_empty()) {
            state.logs.push_back(line.chars().take(2_000).collect());
        }
    }
    while state.logs.len() > MAX_LOG_LINES {
        state.logs.pop_front();
    }
}

fn project_status(project: Option<&Arc<ManagedProject>>) -> ProjectStatusBody {
    let Some(project) = project else {
        return ProjectStatusBody {
            status: "idle",
            kind: None,
            command: None,
            logs: Vec::new(),
            url: None,
            exit_code: None,
            signal: None,
            error: None,
        };
    };
    append_logs(project);
    let state = project.runtime.lock();
    ProjectStatusBody {
        status: if state.stopping && !state.settled {
            "stopping"
        } else if !state.settled {
            "running"
        } else if state.error.is_some() || state.exit_code.is_some_and(|code| code != 0) {
            "failed"
        } else {
            "completed"
        },
        kind: Some(project.kind.clone()),
        command: Some(project.command.clone()),
        logs: state.logs.iter().cloned().collect(),
        url: state.url.clone(),
        exit_code: state.exit_code,
        signal: state.signal.clone(),
        error: state.error.clone(),
    }
}

fn sanitize_file_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || "\\/:*?\"<>|".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    safe = safe.trim().trim_matches('.').chars().take(120).collect();
    if safe.is_empty() {
        "file".to_string()
    } else {
        safe
    }
}

impl PreviewService {
    fn new(
        registry: Arc<WorkspaceRegistry>,
        subprocess: Arc<dyn SubprocessRuntime>,
        sandbox: Arc<dyn SandboxProvider>,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            subprocess,
            sandbox,
            site_token: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(PreviewState {
                challenges: HashMap::new(),
                projects: HashMap::new(),
            }),
        })
    }

    async fn parse_control(request: WebRequest) -> Result<ControlRequest, WebResponse> {
        if request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_CONTROL_BYTES)
        {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body-too-large",
                "控制请求体过大",
            ));
        }
        let bytes = to_bytes(Body::new(request.into_body()), MAX_CONTROL_BYTES)
            .await
            .map_err(|_| {
                error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body-too-large",
                    "控制请求体过大",
                )
            })?;
        serde_json::from_slice(&bytes).map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid-json",
                "控制请求不是合法 JSON",
            )
        })
    }

    async fn prepare_project(&self, control: ControlRequest) -> WebResponse {
        let session = session_id(control.session_id.clone());
        let (_, root) = match workspace_root(&self.registry, &session).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let spec = detect_project(&root);
        let Some(argv) = spec.argv.clone() else {
            return error(
                StatusCode::CONFLICT,
                "project-not-runnable",
                "静态工作区无需启动项目，可直接选择 HTML 文件预览",
            );
        };
        let challenge = uuid::Uuid::new_v4().to_string();
        let expires_at = now_ms().saturating_add(CHALLENGE_TTL_MS);
        let command = render_argv(&argv);
        let mut state = self.state.lock();
        state.challenges.retain(|_, pending| {
            pending.expires_at >= now_ms() && pending.session_id != control.session_id
        });
        state.challenges.insert(
            challenge.clone(),
            RunChallenge {
                session_id: control.session_id,
                root,
                kind: spec.kind.to_string(),
                argv,
                expires_at,
            },
        );
        json_response(
            StatusCode::OK,
            &PrepareBody {
                challenge,
                project: spec.into(),
                command,
                expires_at,
                warning: "此命令来自Host项目探测，但项目脚本本身可以执行代码。仅在信任当前工作区时确认运行。",
            },
        )
    }

    async fn start_project(&self, control: ControlRequest) -> WebResponse {
        let Some(challenge_id) = control.challenge else {
            return error(
                StatusCode::BAD_REQUEST,
                "challenge-required",
                "缺少一次性启动确认",
            );
        };
        let pending = {
            let mut state = self.state.lock();
            state.challenges.remove(&challenge_id)
        };
        let Some(pending) = pending else {
            return error(
                StatusCode::CONFLICT,
                "challenge-invalid",
                "启动确认不存在、已使用或已过期",
            );
        };
        if pending.session_id != control.session_id || pending.expires_at < now_ms() {
            return error(
                StatusCode::CONFLICT,
                "challenge-invalid",
                "启动确认与当前Session不匹配或已过期",
            );
        }
        let session = session_id(control.session_id.clone());
        let (_, root) = match workspace_root(&self.registry, &session).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        if root != pending.root {
            return error(
                StatusCode::CONFLICT,
                "workspace-changed",
                "Workspace 已变化，请重新确认启动命令",
            );
        }
        if self
            .state
            .lock()
            .projects
            .get(&control.session_id)
            .is_some_and(|project| !project.runtime.lock().settled)
        {
            return error(StatusCode::CONFLICT, "project-running", "项目已经在运行");
        }
        let executable = match self
            .subprocess
            .resolve_executable(&pending.argv[0], None, None)
            .await
        {
            Ok(value) => value,
            Err(message) => {
                return error(StatusCode::NOT_FOUND, "executable-not-found", message);
            }
        };
        let mut argv = pending.argv;
        argv[0] = executable;
        let display_command = render_argv(&argv);
        let process_root = execution_path(&root);
        let confined = match self.sandbox.confine(
            &argv,
            &SandboxPolicy {
                mode: ConfinedSandboxMode::WorkspaceWrite,
                workspace_root: process_root.clone(),
                session_id: Some(session.clone()),
            },
        ) {
            Ok(value) if value.enforcement == SandboxEnforcement::Full => value,
            Ok(_) => {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "sandbox-incomplete",
                    "当前平台不能完整隔离项目进程，已拒绝启动",
                );
            }
            Err(message) => {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "sandbox-unavailable",
                    message.to_string(),
                );
            }
        };
        let handle = match self.subprocess.spawn(SubprocessSpawnSpec {
            argv: confined.argv,
            cwd: process_root,
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 2 * 1024 * 1024,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 2 * 1024 * 1024,
                    spill: None,
                }),
            },
            grace_ms: 8_000,
            signal: None,
            env: Some(vec![
                ("NO_COLOR".to_string(), Some("1".to_string())),
                ("BROWSER".to_string(), Some("none".to_string())),
            ]),
        }) {
            Ok(value) => value,
            Err(message) => {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "spawn-failed", message);
            }
        };
        let runtime = Arc::new(Mutex::new(ProjectRuntimeState::default()));
        let managed = Arc::new(ManagedProject {
            root,
            kind: pending.kind,
            command: display_command,
            handle: handle.clone(),
            runtime: runtime.clone(),
        });
        self.state
            .lock()
            .projects
            .insert(control.session_id, managed.clone());
        tokio::spawn(async move {
            match handle.done().await {
                Ok(outcome) => {
                    let mut state = runtime.lock();
                    state.exit_code = outcome.exit_code;
                    state.signal = outcome.signal;
                    state.settled = true;
                }
                Err(message) => {
                    let mut state = runtime.lock();
                    state.error = Some(message);
                    state.settled = true;
                }
            }
        });
        json_response(StatusCode::OK, &project_status(Some(&managed)))
    }

    async fn stop_project(&self, control: ControlRequest) -> WebResponse {
        let project = self.state.lock().projects.get(&control.session_id).cloned();
        if let Some(project) = &project {
            let session = session_id(control.session_id);
            if let Ok((_, root)) = workspace_root(&self.registry, &session).await
                && root != project.root
            {
                return error(
                    StatusCode::CONFLICT,
                    "workspace-changed",
                    "运行项目不再属于当前Workspace",
                );
            }
            project.runtime.lock().stopping = true;
            project.handle.terminate();
        }
        json_response(StatusCode::OK, &project_status(project.as_ref()))
    }

    async fn upload(&self, request: WebRequest) -> WebResponse {
        let query = request.uri().query();
        let session = match query_value(query, "sessionId") {
            Ok(Some(value)) if !value.is_empty() && value.len() <= 200 => session_id(value),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "session-required",
                    "缺少有效的 sessionId",
                );
            }
        };
        let name = match query_value(query, "name") {
            Ok(Some(value)) => sanitize_file_name(&value),
            _ => "file".to_string(),
        };
        if request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_UPLOAD_BYTES)
        {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "file-too-large",
                "上传文件超过64 MiB",
            );
        }
        let (_, root) = match workspace_root(&self.registry, &session).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let bytes = match to_bytes(Body::new(request.into_body()), MAX_UPLOAD_BYTES).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "file-too-large",
                    "上传文件超过64 MiB",
                );
            }
        };
        let upload_root = root.join(".dsh-drops");
        if let Err(message) = tokio::fs::create_dir_all(&upload_root).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload-failed",
                message.to_string(),
            );
        }
        let canonical_upload = match tokio::fs::canonicalize(&upload_root).await {
            Ok(value) if value.starts_with(&root) => value,
            _ => {
                return error(
                    StatusCode::FORBIDDEN,
                    "upload-path-escape",
                    "上传目录越出了Workspace",
                );
            }
        };
        let file_name = format!("{}-{name}", uuid::Uuid::new_v4());
        let target = canonical_upload.join(&file_name);
        if let Err(message) = tokio::fs::write(&target, &bytes).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload-failed",
                message.to_string(),
            );
        }
        json_response(
            StatusCode::OK,
            &UploadBody {
                path: format!(".dsh-drops/{file_name}"),
                size: bytes.len(),
            },
        )
    }

    async fn serve_site(&self, request: WebRequest, tail: &str) -> WebResponse {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method-not-allowed",
                "站点预览只允许 GET 和 HEAD",
            );
        }
        let mut parts = tail.split('/');
        let token = parts.next().and_then(|value| percent_decode(value).ok());
        if token.as_deref() != Some(self.site_token.as_str()) {
            return error(
                StatusCode::FORBIDDEN,
                "site-token-invalid",
                "站点预览令牌无效",
            );
        }
        let sid = match parts.next().and_then(|value| percent_decode(value).ok()) {
            Some(value) if !value.is_empty() => session_id(value),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "session-required",
                    "站点预览缺少Session",
                );
            }
        };
        let relative = match parts
            .map(percent_decode)
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join("/"))
        {
            Ok(value) => value,
            Err(()) => return error(StatusCode::BAD_REQUEST, "invalid-path", "站点路径编码无效"),
        };
        let (_, root, mut target) = match authorized_path(&self.registry, &sid, &relative).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        if target.is_dir() {
            target = match tokio::fs::canonicalize(target.join("index.html")).await {
                Ok(value) if value.starts_with(&root) => value,
                _ => {
                    return error(
                        StatusCode::NOT_FOUND,
                        "index-not-found",
                        "目录中没有 index.html",
                    );
                }
            };
        }
        self.serve_file(request.method(), &target, true).await
    }

    async fn serve_file(&self, method: &Method, target: &Path, site: bool) -> WebResponse {
        let metadata = match tokio::fs::metadata(target).await {
            Ok(value) if value.is_file() => value,
            _ => return error(StatusCode::BAD_REQUEST, "not-file", "目标不是文件"),
        };
        let mime = mime_guess::from_path(target).first_or_octet_stream();
        let text_like = mime.essence_str().starts_with("text/")
            || matches!(
                mime.essence_str(),
                "application/json"
                    | "application/javascript"
                    | "application/xml"
                    | "application/yaml"
                    | "image/svg+xml"
            );
        let limit = if text_like {
            MAX_TEXT_BYTES
        } else {
            MAX_MEDIA_BYTES
        };
        if metadata.len() > limit {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "file-too-large",
                format!("文件超过预览上限 {limit} bytes"),
            );
        }
        let mut bytes = if method == Method::HEAD {
            Vec::new()
        } else {
            match tokio::fs::read(target).await {
                Ok(value) => value,
                Err(_) => {
                    return error(StatusCode::FORBIDDEN, "file-unreadable", "文件不可读取");
                }
            }
        };
        if site && mime.essence_str() == "text/html" && method != Method::HEAD {
            let Ok(html) = String::from_utf8(bytes) else {
                return error(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "html-not-utf8",
                    "HTML不是UTF-8编码",
                );
            };
            bytes = inject_bridge(html).into_bytes();
        }
        let mut builder = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                "cross-origin-resource-policy",
                if site { "cross-origin" } else { "same-origin" },
            )
            .header("referrer-policy", "no-referrer");
        if site {
            builder = builder.header("access-control-allow-origin", "*");
        }
        if site && mime.essence_str() == "text/html" {
            builder = builder.header(
                "content-security-policy",
                "sandbox allow-scripts; default-src data: blob: http://127.0.0.1:* http://localhost:*; img-src data: blob: http://127.0.0.1:* http://localhost:*; media-src data: blob: http://127.0.0.1:* http://localhost:*; style-src 'unsafe-inline' http://127.0.0.1:* http://localhost:*; script-src 'unsafe-inline' 'unsafe-eval' blob: http://127.0.0.1:* http://localhost:*; connect-src ws://127.0.0.1:* ws://localhost:* http://127.0.0.1:* http://localhost:*; font-src data: http://127.0.0.1:* http://localhost:*; form-action 'none'; base-uri 'none'",
            );
        } else if matches!(mime.essence_str(), "text/html" | "image/svg+xml") {
            builder = builder.header(
                "content-security-policy",
                "sandbox; default-src 'none'; img-src 'self' data: blob:; style-src 'unsafe-inline'; font-src 'self' data:",
            );
        }
        builder
            .header(
                header::CONTENT_LENGTH,
                if method == Method::HEAD {
                    metadata.len()
                } else {
                    bytes.len() as u64
                },
            )
            .body(Body::from(bytes))
            .expect("preview file response")
    }

    async fn handle(self: Arc<Self>, request: WebRequest) -> WebResponse {
        let tail = request
            .uri()
            .path()
            .strip_prefix(ROUTE)
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string();
        let site_request = tail.strip_prefix("site/").is_some_and(|site_tail| {
            site_tail
                .split('/')
                .next()
                .and_then(|value| percent_decode(value).ok())
                .as_deref()
                == Some(self.site_token.as_str())
        });
        let loopback_host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .is_some_and(super::is_loopback_authority);
        if !loopback_host || (!site_request && !super::trusted_web_request(&request)) {
            return error(StatusCode::FORBIDDEN, "forbidden", "预览请求来源不可信");
        }
        if let Some(site_tail) = tail.strip_prefix("site/").map(str::to_string) {
            return self.serve_site(request, &site_tail).await;
        }
        let operation = tail.trim_matches('/');
        if request.method() == Method::POST {
            return match operation {
                "project-prepare" => match Self::parse_control(request).await {
                    Ok(control) => self.prepare_project(control).await,
                    Err(response) => response,
                },
                "project-start" => match Self::parse_control(request).await {
                    Ok(control) => self.start_project(control).await,
                    Err(response) => response,
                },
                "project-stop" => match Self::parse_control(request).await {
                    Ok(control) => self.stop_project(control).await,
                    Err(response) => response,
                },
                "upload" => self.upload(request).await,
                _ => error(StatusCode::NOT_FOUND, "route-not-found", "未知预览操作"),
            };
        }
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method-not-allowed",
                "只允许 GET、HEAD 和受限 POST",
            );
        }
        let query = request.uri().query();
        let session = match query_value(query, "sessionId") {
            Ok(Some(value)) if !value.is_empty() && value.len() <= 200 => session_id(value),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "session-required",
                    "缺少有效的 sessionId",
                );
            }
        };
        let relative = match query_value(query, "path") {
            Ok(value) => value.unwrap_or_default(),
            Err(()) => return error(StatusCode::BAD_REQUEST, "invalid-query", "查询参数编码无效"),
        };
        if operation == "meta" {
            let (workspace, root) = match workspace_root(&self.registry, &session).await {
                Ok(value) => value,
                Err(response) => return response,
            };
            return json_response(
                StatusCode::OK,
                &MetaBody {
                    session_id: session.as_str().to_string(),
                    workspace_title: workspace.title(),
                    capabilities: [
                        "list",
                        "file",
                        "site",
                        "upload",
                        "project-detect",
                        "project-run-confirmed",
                        "element-annotate",
                    ],
                    max_text_bytes: MAX_TEXT_BYTES,
                    max_media_bytes: MAX_MEDIA_BYTES,
                    max_upload_bytes: MAX_UPLOAD_BYTES,
                    site_token: self.site_token.clone(),
                    project: detect_project(&root).into(),
                },
            );
        }
        if operation == "project-status" {
            let root = match workspace_root(&self.registry, &session).await {
                Ok((_, root)) => root,
                Err(response) => return response,
            };
            let project = self
                .state
                .lock()
                .projects
                .get(session.as_str())
                .filter(|project| project.root == root)
                .cloned();
            return json_response(StatusCode::OK, &project_status(project.as_ref()));
        }
        let (_, root, target) = match authorized_path(&self.registry, &session, &relative).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        if operation == "list" {
            if !target.is_dir() {
                return error(StatusCode::BAD_REQUEST, "not-directory", "目标不是目录");
            }
            let mut reader = match tokio::fs::read_dir(&target).await {
                Ok(value) => value,
                Err(_) => {
                    return error(
                        StatusCode::FORBIDDEN,
                        "directory-unreadable",
                        "目录不可读取",
                    );
                }
            };
            let mut entries = Vec::new();
            let mut truncated = false;
            while let Ok(Some(entry)) = reader.next_entry().await {
                if entries.len() >= MAX_DIRECTORY_ENTRIES {
                    truncated = true;
                    break;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if denied_name(&name) {
                    continue;
                }
                let Ok(kind) = entry.file_type().await else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                let Ok(meta) = entry.metadata().await else {
                    continue;
                };
                entries.push(DirectoryEntry {
                    name,
                    path: relative_display(&root, &entry.path()),
                    kind: if kind.is_dir() {
                        "directory"
                    } else if kind.is_file() {
                        "file"
                    } else {
                        "other"
                    },
                    size: kind.is_file().then_some(meta.len()),
                });
            }
            entries.sort_by(|a, b| {
                (a.kind != "directory", a.name.to_ascii_lowercase())
                    .cmp(&(b.kind != "directory", b.name.to_ascii_lowercase()))
            });
            return json_response(
                StatusCode::OK,
                &DirectoryBody {
                    path: relative_display(&root, &target),
                    entries,
                    truncated,
                },
            );
        }
        if operation == "file" {
            return self.serve_file(request.method(), &target, false).await;
        }
        error(StatusCode::NOT_FOUND, "route-not-found", "未知预览操作")
    }
}

pub fn register(
    web_server: &Arc<WebServer>,
    registry: Arc<WorkspaceRegistry>,
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox: Arc<dyn SandboxProvider>,
) -> RouteDisposer {
    let service = PreviewService::new(registry, subprocess, sandbox);
    web_server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: ROUTE.to_string(),
        handler: Arc::new(move |request| {
            let service = Arc::clone(&service);
            Box::pin(async move { Ok::<_, WebHandlerError>(service.handle(request).await) })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_fail_closed() {
        assert_eq!(
            safe_relative("src/main.rs"),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(safe_relative("."), Some(PathBuf::new()));
        assert_eq!(safe_relative("../secret"), None);
        assert_eq!(safe_relative("C:/secret"), None);
        assert_eq!(safe_relative(".git/config"), None);
        assert_eq!(safe_relative("foo/.env.local"), None);
        assert_eq!(safe_relative("node_modules/pkg/index.js"), None);
    }

    #[test]
    fn query_decoding_is_strict() {
        assert_eq!(
            query_value(Some("path=docs%2Fguide.md"), "path"),
            Ok(Some("docs/guide.md".to_string()))
        );
        assert_eq!(query_value(Some("path=%GG"), "path"), Err(()));
        assert_eq!(query_value(Some("path=%"), "path"), Err(()));
    }

    #[test]
    fn project_detection_returns_only_fixed_argv() {
        let root =
            std::env::temp_dir().join(format!("dsh-preview-project-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create fixture");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='fixture'\n")
            .expect("write manifest");
        let spec = detect_project(&root);
        assert_eq!(spec.kind, "rust");
        assert_eq!(spec.manifest, Some("Cargo.toml"));
        assert_eq!(
            spec.argv,
            Some(vec!["cargo".to_string(), "run".to_string()])
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn annotation_bridge_is_injected_once() {
        let once = inject_bridge("<html><body>Hello</body></html>".to_string());
        assert_eq!(once.matches("data-dsh-preview-bridge").count(), 1);
        let twice = inject_bridge(once);
        assert_eq!(twice.matches("data-dsh-preview-bridge").count(), 1);
    }

    #[test]
    fn local_url_detection_rejects_remote_hosts() {
        assert_eq!(
            local_preview_url("ready at http://localhost:5173/"),
            Some("http://localhost:5173/".to_string())
        );
        assert_eq!(local_preview_url("https://example.com:443/"), None);
    }

    #[test]
    fn upload_names_are_flat_and_bounded() {
        assert_eq!(sanitize_file_name("../../a?.md"), "_.._a_.md");
        assert!(sanitize_file_name(&"x".repeat(500)).chars().count() <= 120);
    }

    #[test]
    fn execution_paths_drop_windows_verbatim_prefix() {
        let rendered = execution_path(Path::new(r"\\?\D:\workspace"));
        if cfg!(windows) {
            assert_eq!(rendered, r"D:\workspace");
        }
    }
}
