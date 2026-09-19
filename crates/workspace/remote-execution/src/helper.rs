//! Remote-side trust boundary. Configuration is created explicitly on the remote host.
use crate::protocol::*;
use base64::Engine;
use dsh_fs::{FileSystem, ResolveOptions};
use dsh_sandbox::{ConfinedSandboxMode, SandboxMode, SandboxPolicy, SandboxProvider};
use dsh_subprocess::{
    SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec, SubprocessStdinMode,
    SubprocessStdio,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrant {
    pub path: String,
    pub max_mode: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub host_id: String,
    pub workspaces: Vec<WorkspaceGrant>,
}

fn state_root() -> Result<PathBuf, String> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .map(|p| p.join(".deepseek-harness/remote-helper"))
        .ok_or("remote home directory unavailable".into())
}
async fn atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("record budget exceeded".into());
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
fn read<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() > MAX_FRAME as u64 {
        return Err("record budget exceeded".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
fn mode(value: &str) -> Result<SandboxMode, String> {
    match value {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
        _ => Err("invalid permission mode".into()),
    }
}
fn rank(value: SandboxMode) -> u8 {
    match value {
        SandboxMode::ReadOnly => 0,
        SandboxMode::WorkspaceWrite => 1,
        SandboxMode::DangerFullAccess => 2,
    }
}
fn candidate(name: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|p| p.is_absolute())
        .map(|p| p.join(format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

pub struct Helper {
    root: PathBuf,
    policy: Policy,
    fs: Arc<dsh_fs_local::LocalFileSystem>,
}
impl Helper {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let policy:Policy=read(&root.join("policy.json")).map_err(|e|format!("remote helper is not configured: run dsh-remote-helper --authorize <workspace> <read-only|workspace-write|danger-full-access> on the remote host; {e}"))?;
        if policy.version != PROTOCOL_VERSION || policy.workspaces.len() > 64 {
            return Err("unsupported helper policy version or workspace limit".into());
        }
        Ok(Self {
            root,
            policy,
            fs: dsh_fs_local::LocalFileSystem::build(dsh_fs_local::Config {
                cwd: None,
                diff_basis_max_bytes: None,
            })?,
        })
    }
    fn workspace(&self, path: &str) -> Result<(String, String), String> {
        if !Path::new(path).is_absolute() {
            return Err("remote workspace must be absolute".into());
        }
        let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !canonical.is_dir() {
            return Err("remote workspace is not a directory".into());
        }
        let grant = self
            .policy
            .workspaces
            .iter()
            .find(|g| std::fs::canonicalize(&g.path).ok().as_ref() == Some(&canonical))
            .ok_or("workspace has not been authorized on remote host")?;
        mode(&grant.max_mode)?;
        Ok((
            canonical.to_string_lossy().into_owned(),
            grant.max_mode.clone(),
        ))
    }
    pub fn handshake(&self, workspace: &str) -> Result<Handshake, String> {
        let (workspace, ceiling) = self.workspace(workspace)?;
        let shell_kind = if cfg!(windows) { "powershell" } else { "bash" }.to_string();
        let shell_path = if cfg!(windows) {
            candidate("pwsh").or_else(|| candidate("powershell"))
        } else {
            candidate("bash")
        };
        let python_path = candidate(if cfg!(windows) { "python" } else { "python3" });
        let tools = ["node", "git", "rustc", "rg", "ffmpeg"]
            .into_iter()
            .filter_map(|id| candidate(id).map(|p| (id.into(), p)))
            .collect::<BTreeMap<_, _>>();
        let identity = |p: &String| {
            let m = std::fs::metadata(p).ok();
            json!({"path":p,"size":m.as_ref().map(|m|m.len()),"modified":m.and_then(|m|m.modified().ok()).and_then(|s|s.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())})
        };
        let context_id = digest(
            &json!({"protocol":PROTOCOL_VERSION,"host":self.policy.host_id,"workspace":workspace,"ceiling":ceiling,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"shell":shell_path.as_ref().map(identity),"python":python_path.as_ref().map(identity),"tools":tools.iter().map(|(k,v)|(k,identity(v))).collect::<BTreeMap<_,_>>()}),
        );
        Ok(Handshake {
            protocol_version: PROTOCOL_VERSION,
            host_id: self.policy.host_id.clone(),
            backend_id: "ssh-helper".into(),
            context_id,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            workspace,
            permission_ceiling: ceiling,
            shell_path,
            shell_kind,
            python_path,
            tools,
            capabilities: vec![
                "files".into(),
                "structured-process".into(),
                "execution-query".into(),
                "confirmed-cancel".into(),
            ],
        })
    }
    fn authorized_mode(
        &self,
        handshake: &Handshake,
        requested: Option<&str>,
    ) -> Result<SandboxMode, String> {
        let mode = mode(requested.unwrap_or("read-only"))?;
        if rank(mode) > rank(self::mode(&handshake.permission_ceiling)?) {
            return Err("requested permissions exceed the remote host grant".into());
        }
        Ok(mode)
    }
    pub async fn request(&self, request: Request) -> Reply {
        let mut reply = Reply {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            host_id: self.policy.host_id.clone(),
            context_id: String::new(),
            backend_id: "ssh-helper".into(),
            ok: false,
            value: Value::Null,
            error: None,
        };
        let result = async {
            if request.protocol_version != PROTOCOL_VERSION {
                return Err("remote helper protocol version mismatch".into());
            }
            let handshake = self.handshake(&request.workspace)?;
            reply.context_id = handshake.context_id.clone();
            if request.action == "handshake" {
                return serde_json::to_value(handshake).map_err(|e| e.to_string());
            }
            if request.context_id.as_deref() != Some(handshake.context_id.as_str()) {
                return Err(
                    "remote context changed; reconnect and verify environment before executing"
                        .into(),
                );
            }
            let permission =
                self.authorized_mode(&handshake, request.permission_mode.as_deref())?;
            match request.action.as_str() {
                "file" => self.file(&handshake, &request.payload, permission).await,
                "execute" => {
                    self.execute(
                        &handshake,
                        request
                            .execution_id
                            .as_deref()
                            .ok_or("missing execution id")?,
                        request.payload,
                        permission,
                    )
                    .await
                }
                "query" | "cancel" => {
                    self.query(
                        &handshake,
                        request
                            .execution_id
                            .as_deref()
                            .ok_or("missing execution id")?,
                        &request.payload,
                        request.action == "cancel",
                    )
                    .await
                }
                _ => Err("unknown helper action".into()),
            }
        }
        .await;
        match result {
            Ok(value) => {
                reply.ok = true;
                reply.value = value;
            }
            Err(error) => reply.error = Some(error),
        }
        reply
    }

    async fn target(&self, handshake: &Handshake, path: &str) -> Result<dsh_fs::FsTarget, String> {
        if path.len() > 16384 || path.contains('\0') {
            return Err("invalid path".into());
        }
        let options = ResolveOptions {
            cwd: Some(handshake.workspace.clone()),
            signal: None,
        };
        let root = self
            .fs
            .resolve(&handshake.workspace, None)
            .await
            .map_err(|e| e.to_string())?;
        let target = self
            .fs
            .resolve(path, Some(&options))
            .await
            .map_err(|e| e.to_string())?;
        if !self.fs.contains(&root, &target) {
            return Err("FS_SANDBOX_DENIED: remote path escapes authorized workspace".into());
        }
        Ok(target)
    }
    async fn file(
        &self,
        handshake: &Handshake,
        args: &Value,
        permission: SandboxMode,
    ) -> Result<Value, String> {
        let op = args["op"].as_str().ok_or("missing file operation")?;
        let path = args["path"].as_str().unwrap_or(".");
        let target = self.target(handshake, path).await?;
        let version = |info: dsh_fs::FsInfo| json!({"version":info.version.as_str(),"kind":info.kind.as_str(),"size":info.size});
        match op {
            "resolve"=>Ok(json!({"path":self.fs.process_path(&target)})),
            "stat"=>self.fs.stat(&target,None).await.map(|v|v.map(version).unwrap_or(Value::Null)).map_err(|e|e.to_string()),
            "lstat"=>self.fs.lstat(path,Some(&dsh_fs::LstatOptions{cwd:Some(handshake.workspace.clone())}),None).await.map(|v|v.map(|i|json!({"version":i.version.as_str(),"kind":i.kind.as_str(),"size":i.size})).unwrap_or(Value::Null)).map_err(|e|e.to_string()),
            "read"=>{let max=args["maxBytes"].as_u64().unwrap_or(1024*1024).min(2*1024*1024);let bytes=self.fs.read_bytes(&target,None,max).await.map_err(|e|e.to_string())?;Ok(json!({"base64":base64::engine::general_purpose::STANDARD.encode(bytes)}))},
            "list"=>{let entries=self.fs.list_dir(&target,None).await.map_err(|e|e.to_string())?;if entries.len()>4096{return Err("directory entry budget exceeded".into());}Ok(json!({"entries":entries.into_iter().map(|e|json!({"name":e.name,"path":self.fs.process_path(&e.target),"kind":e.kind.as_str(),"version":e.version.map(|v|v.to_string()),"size":e.size})).collect::<Vec<_>>()}))},
            "write"|"edit"=>{
                if permission==SandboxMode::ReadOnly{return Err("FS_SANDBOX_DENIED: remote workspace is read-only".into());}
                if self.fs.stat(&target,None).await.map_err(|e|e.to_string())?.is_some_and(|info|info.size.unwrap_or(0)>256*1024) {return Err("FS_TOO_LARGE: remote editable file exceeds 256 KiB".into());}
                let lockpath=self.root.join("file-locks").join(digest(&json!(target.target_key.as_str())));
                tokio::fs::create_dir_all(lockpath.parent().unwrap()).await.map_err(|e|e.to_string())?;
                dsh_atomic_write::with_file_lock(&lockpath,async {
                    // Re-resolve after acquiring the cross-process mutation lock.
                    let target=self.target(handshake,path).await?;
                    if op=="write" {
                        let content=args["content"].as_str().ok_or("missing content")?;if content.len()>256*1024{return Err("file write budget exceeded".into());}
                        let guard=if args["createOnly"]==true{Some(dsh_fs::FsWriteIntent::CreateIfAbsent)}else{args["expectedVersion"].as_str().map(|v|dsh_fs::FsWriteIntent::ReplaceIfVersion{version:dsh_fs::fs_version(v)})};
                        let out=self.fs.write_text(&target,content,guard.as_ref(),None,None).await.map_err(|e|format!("{}: {e}",e.code.as_str()))?;
                        Ok(json!({"operation":if matches!(out.operation,dsh_fs::FsWriteOperation::Create){"create"}else{"update"},"version":out.version.as_str(),"before":out.before,"after":out.after}))
                    }else{
                        let edit=dsh_fs::FsEditRequest{old_string:args["oldString"].as_str().ok_or("missing old text")?.into(),new_string:args["newString"].as_str().ok_or("missing new text")?.into(),replace_all:args["replaceAll"].as_bool().unwrap_or(false)};
                        let guard=args["expectedVersion"].as_str().map(|v|dsh_fs::FsEditGuard{version:dsh_fs::fs_version(v)});
                        let out=self.fs.edit_text(&target,&edit,guard.as_ref(),None,None).await.map_err(|e|format!("{}: {e}",e.code.as_str()))?;
                        Ok(json!({"version":out.version.as_str(),"before":out.before,"after":out.after}))
                    }
                }).await.map_err(|e|e.to_string())?
            },
            _=>Err("unknown file operation".into()),
        }
    }

    fn execution_dir(&self, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid execution id")?;
        Ok(self.root.join("executions").join(id))
    }
    async fn execute(
        &self,
        handshake: &Handshake,
        id: &str,
        payload: Value,
        permission: SandboxMode,
    ) -> Result<Value, String> {
        let mut execution: Execution =
            serde_json::from_value(payload).map_err(|e| e.to_string())?;
        if execution.execution_id != id
            || execution.context_id != handshake.context_id
            || execution.workspace != handshake.workspace
            || execution.permission_mode != permission.as_str()
        {
            return Err("execution identity mismatch".into());
        }
        if execution.argv.is_empty()
            || execution.argv.len() > 1024
            || execution.argv.iter().map(String::len).sum::<usize>() > 256 * 1024
            || execution.timeout_ms == 0
            || execution.timeout_ms > 3_600_000
        {
            return Err("invalid execution limits".into());
        }
        if execution.env.iter().any(|(k, _)| {
            dsh_subprocess::sensitive_env_pattern().is_match(k)
                || k.starts_with("DSH_")
                || k == "SSH_AUTH_SOCK"
        }) {
            return Err("credential or harness environment forwarding is not supported".into());
        }
        execution.cwd = self
            .fs
            .process_path(&self.target(handshake, &execution.cwd).await?);
        let dir = self.execution_dir(id)?;
        tokio::fs::create_dir_all(dir.parent().unwrap())
            .await
            .map_err(|e| e.to_string())?;
        if !dir.exists()
            && std::fs::read_dir(dir.parent().unwrap())
                .map_err(|e| e.to_string())?
                .take(128)
                .count()
                >= 128
        {
            return Err("remote execution record budget reached; archive completed helper records on the remote host".into());
        }
        match tokio::fs::create_dir(&dir).await {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let original: Execution = read(&dir.join("request.json"))?;
                if serde_json::to_value(original).unwrap()
                    != serde_json::to_value(execution).unwrap()
                {
                    return Err("execution id already belongs to a different request".into());
                }
                return self.query(handshake, id, &json!({}), false).await;
            }
            Err(e) => return Err(e.to_string()),
        }
        atomic(&dir.join("request.json"), &execution).await?;
        atomic(
            &dir.join("state.json"),
            &ExecutionState {
                execution_id: id.into(),
                context_id: handshake.context_id.clone(),
                state: "submitted".into(),
                exit_code: None,
                signal: None,
                updated_at: now(),
                stdout_bytes: 0,
                stderr_bytes: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                error: None,
            },
        )
        .await?;
        let mut command =
            std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .args([
                "--worker",
                self.root.to_str().ok_or("helper path is not Unicode")?,
                id,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000008 | 0x00000200 | 0x08000000);
        }
        command.spawn().map_err(|e| {
            format!("worker dispatch failed; query original execution id before retrying: {e}")
        })?;
        self.query(handshake, id, &json!({}), false).await
    }

    async fn query(
        &self,
        handshake: &Handshake,
        id: &str,
        args: &Value,
        cancel: bool,
    ) -> Result<Value, String> {
        let dir = self.execution_dir(id)?;
        let mut state: ExecutionState = match read(&dir.join("state.json")) {
            Ok(s) => s,
            Err(_) => {
                return Ok(
                    json!({"executionId":id,"contextId":handshake.context_id,"state":"unknown","reason":"no authoritative execution record; command has not been replayed"}),
                );
            }
        };
        if state.context_id != handshake.context_id {
            return Err("execution belongs to a different remote environment".into());
        }
        if cancel && !state.terminal() {
            atomic(&dir.join("cancel.json"), &json!({"executionId":id})).await?;
        }
        if !state.terminal() && now().saturating_sub(state.updated_at) > 15 {
            state.state = "unknown".into();
            state.error =
                Some("worker heartbeat unavailable; command has not been replayed".into());
        }
        let output = |name: &str, offset: u64| -> Result<Value, String> {
            use std::io::{Read, Seek};
            let mut file = match std::fs::File::open(dir.join(name)) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(json!({"base64":"","nextOffset":0}));
                }
                Err(e) => return Err(e.to_string()),
            };
            let total = file.metadata().map_err(|e| e.to_string())?.len();
            let offset = offset.min(total);
            file.seek(std::io::SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.take(256 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            Ok(
                json!({"base64":base64::engine::general_purpose::STANDARD.encode(&bytes),"nextOffset":offset+bytes.len() as u64,"totalBytes":total}),
            )
        };
        let mut value = serde_json::to_value(&state).unwrap();
        value["stdout"] = output("stdout.bin", args["stdoutOffset"].as_u64().unwrap_or(0))?;
        value["stderr"] = output("stderr.bin", args["stderrOffset"].as_u64().unwrap_or(0))?;
        value["cancelAcknowledged"] = json!(cancel && state.state == "cancelled");
        Ok(value)
    }
}

async fn drain(
    mut input: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    path: PathBuf,
    count: Arc<AtomicU64>,
    truncated: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 16384];
    let mut retained = 0;
    loop {
        let n = input.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count.fetch_add(n as u64, Ordering::AcqRel);
        let keep = (MAX_LOG - retained).min(n as u64) as usize;
        if keep > 0 {
            file.write_all(&buffer[..keep])
                .await
                .map_err(|e| e.to_string())?;
            retained += keep as u64;
        }
        if keep < n {
            truncated.store(true, Ordering::Release);
        }
    }
    file.flush().await.map_err(|e| e.to_string())
}

pub async fn worker(root: PathBuf, id: &str) -> Result<(), String> {
    let helper = Helper::open(root)?;
    let dir = helper.execution_dir(id)?;
    // The durable claim prevents duplicate workers even if SSH repeats a transport request.
    let _claim = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dir.join("worker.claim"))
        .map_err(|_| "worker already claimed; query execution instead")?;
    let execution: Execution = read(&dir.join("request.json"))?;
    let handshake = helper.handshake(&execution.workspace)?;
    if execution.context_id != handshake.context_id {
        return Err("remote environment changed before worker dispatch".into());
    }
    let permission = helper.authorized_mode(&handshake, Some(&execution.permission_mode))?;
    let runtime = dsh_subprocess_local::LocalSubprocessRuntime::new();
    let sandbox = dsh_sandbox_local::LocalSandboxProvider::new(Default::default());
    let argv = if permission == SandboxMode::DangerFullAccess {
        execution.argv.clone()
    } else {
        sandbox
            .confine_with_startup(
                &execution.argv,
                &SandboxPolicy {
                    mode: if permission == SandboxMode::ReadOnly {
                        ConfinedSandboxMode::ReadOnly
                    } else {
                        ConfinedSandboxMode::WorkspaceWrite
                    },
                    workspace_root: handshake.workspace.clone(),
                    read_only_roots: Vec::new(),
                    session_id: Some(dsh_session::session_id(id)),
                },
            )
            .map_err(|e| e.to_string())?
            .argv
    };
    let child = runtime.spawn(SubprocessSpawnSpec {
        argv,
        cwd: execution.cwd.clone(),
        env: Some(
            execution
                .env
                .iter()
                .map(|(k, v)| (k.clone(), Some(v.clone())))
                .collect(),
        ),
        signal: None,
        grace_ms: 500,
        stdio: SubprocessStdio {
            stdin: execution
                .stdin
                .map(SubprocessStdinMode::Data)
                .unwrap_or(SubprocessStdinMode::Ignore),
            stdout: SubprocessOutputMode::Pipe,
            stderr: SubprocessOutputMode::Pipe,
        },
    })?;
    let counts = [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))];
    let trunc = [
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ];
    let out = tokio::spawn(drain(
        child.stdout().ok_or("stdout unavailable")?,
        dir.join("stdout.bin"),
        counts[0].clone(),
        trunc[0].clone(),
    ));
    let err = tokio::spawn(drain(
        child.stderr().ok_or("stderr unavailable")?,
        dir.join("stderr.bin"),
        counts[1].clone(),
        trunc[1].clone(),
    ));
    let mut state = ExecutionState {
        execution_id: id.into(),
        context_id: handshake.context_id,
        state: "running".into(),
        exit_code: None,
        signal: None,
        updated_at: now(),
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        error: None,
    };
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(execution.timeout_ms);
    let done = child.done();
    struct KillChild(Arc<dyn dsh_subprocess::SubprocessHandle>);
    impl Drop for KillChild {
        fn drop(&mut self) {
            self.0.terminate();
        }
    }
    let _kill = KillChild(child.clone());
    let mut ending: Option<&str> = None;
    tokio::pin!(done);
    loop {
        state.updated_at = now();
        state.stdout_bytes = counts[0].load(Ordering::Acquire);
        state.stderr_bytes = counts[1].load(Ordering::Acquire);
        atomic(&dir.join("state.json"), &state).await?;
        tokio::select! {
            outcome=&mut done=>{match outcome{Ok(outcome)=>{state.exit_code=outcome.exit_code;state.signal=outcome.signal;state.state=ending.unwrap_or(if state.exit_code==Some(0){"completed"}else{"failed"}).into();},Err(error)=>{state.state="unknown".into();state.error=Some(error);}}break;},
            _=tokio::time::sleep(std::time::Duration::from_millis(250))=>{
                if ending.is_none() && dir.join("cancel.json").is_file(){ending=Some("cancelled");child.terminate();}
                else if ending.is_none() && tokio::time::Instant::now()>=deadline{ending=Some("timed_out");child.terminate();}
            }
        }
    }
    for task in [out, err] {
        if let Err(error) = task.await.map_err(|e| e.to_string())? {
            state.state = "failed".into();
            state.error = Some(error);
        }
    }
    state.updated_at = now();
    state.stdout_bytes = counts[0].load(Ordering::Acquire);
    state.stderr_bytes = counts[1].load(Ordering::Acquire);
    state.stdout_truncated = trunc[0].load(Ordering::Acquire);
    state.stderr_truncated = trunc[1].load(Ordering::Acquire);
    atomic(&dir.join("state.json"), &state).await
}

pub async fn run_cli(args: Vec<String>) -> Result<i32, String> {
    #[cfg(windows)]
    {
        if args.first().map(String::as_str) == Some("__dsh-sandbox-windows") {
            return dsh_sandbox_local::run_windows_sandbox(args.into_iter().skip(1));
        }
        dsh_sandbox_local::register_embedded_windows_runner()?;
    }
    match args.first().map(String::as_str) {
        Some("--worker") if args.len() == 3 => {
            let root = PathBuf::from(&args[1]);
            let result = worker(root.clone(), &args[2]).await;
            if let Err(error) = &result {
                if let Ok(helper) = Helper::open(root) {
                    if let Ok(dir) = helper.execution_dir(&args[2]) {
                        if let Ok(mut state) = read::<ExecutionState>(&dir.join("state.json")) {
                            state.state = "unknown".into();
                            state.error = Some(error.clone());
                            state.updated_at = now();
                            let _ = atomic(&dir.join("state.json"), &state).await;
                        }
                    }
                }
            }
            result.map(|_| 0)
        }
        Some("--authorize") if args.len() == 3 => {
            mode(&args[2])?;
            let root = state_root()?;
            let path = std::fs::canonicalize(&args[1]).map_err(|e| e.to_string())?;
            if !path.is_dir() {
                return Err("workspace must be a directory".into());
            }
            let mut policy = if root.join("policy.json").exists() {
                read(&root.join("policy.json"))?
            } else {
                Policy {
                    version: PROTOCOL_VERSION,
                    host_id: uuid::Uuid::new_v4().to_string(),
                    workspaces: Vec::new(),
                }
            };
            let path = path.to_string_lossy().into_owned();
            policy.workspaces.retain(|w| w.path != path);
            policy.workspaces.push(WorkspaceGrant {
                path,
                max_mode: args[2].clone(),
            });
            atomic(&root.join("policy.json"), &policy).await?;
            Ok(0)
        }
        Some("--stdio") if args.len() == 1 => {
            // Blocking stdin is intentional: one bounded frame per SSH invocation, no child stdout on this channel.
            let bytes = tokio::task::spawn_blocking(|| {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take((MAX_FRAME + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            if bytes.len() > MAX_FRAME {
                return Err("request frame budget exceeded".into());
            }
            let request: Request = serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid protocol frame: {e}"))?;
            let helper = Helper::open(state_root()?)?;
            let reply = helper.request(request).await;
            let frame = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
            if frame.len() > MAX_FRAME {
                return Err("response frame budget exceeded".into());
            }
            use std::io::Write;
            std::io::stdout()
                .write_all(&frame)
                .map_err(|e| e.to_string())?;
            Ok(0)
        }
        _ => Err("usage: dsh-remote-helper --authorize <workspace> <mode> | --stdio".into()),
    }
}
