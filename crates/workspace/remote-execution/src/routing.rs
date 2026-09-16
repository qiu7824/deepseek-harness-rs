use crate::{
    protocol::*,
    transport::{RemoteConnection, RemoteRuntime},
};
use base64::Engine;
use dsh_fs::*;
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_shell::*;
use futures::{FutureExt, future::BoxFuture};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

fn fs_error(error: String) -> FsError {
    let code = if error.contains("FS_SANDBOX_DENIED") || error.contains("remote host grant") {
        FsErrorCode::FsSandboxDenied
    } else if error.contains("FS_STALE_VERSION") {
        FsErrorCode::FsStaleVersion
    } else if error.contains("FS_NOT_OBSERVED") {
        FsErrorCode::FsNotObserved
    } else if error.contains("FS_AMBIGUOUS_EDIT") {
        FsErrorCode::FsAmbiguousEdit
    } else if error.contains("FS_EDIT_NOT_FOUND") {
        FsErrorCode::FsEditNotFound
    } else {
        FsErrorCode::FsIoError
    };
    FsError::new(error, code)
}
fn target(id: &str, path: &str) -> FsTarget {
    let uri = format!("dsh-remote://{id}/{path}");
    FsTarget {
        target_key: fs_target_key(uri.clone()),
        display_path: uri,
    }
}
fn info(value: Value) -> Result<Option<FsInfo>, FsError> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(FsInfo {
        version: fs_version(
            value["version"]
                .as_str()
                .ok_or_else(|| fs_error("invalid remote metadata".into()))?,
        ),
        kind: kind(value["kind"].as_str()),
        size: value["size"].as_u64(),
    }))
}
fn kind(value: Option<&str>) -> FsInfoType {
    match value {
        Some("file") => FsInfoType::File,
        Some("directory") => FsInfoType::Directory,
        _ => FsInfoType::Other,
    }
}
fn native_path(row: &RemoteConnection, cwd_tail: &str, path: &str) -> String {
    let absolute = path.starts_with('/')
        || (row.handshake.os == "windows" && path.as_bytes().get(1) == Some(&b':'));
    if absolute {
        return path.into();
    }
    let cwd = if cwd_tail.is_empty() {
        row.handshake.workspace.as_str()
    } else {
        cwd_tail
    };
    format!("{}/{}", cwd.trim_end_matches(['/', '\\']), path)
}
pub struct RemoteFileSystem {
    local: Arc<dyn FileSystem>,
    remote: Arc<RemoteRuntime>,
}
impl RemoteFileSystem {
    fn remote_target(&self, target: &FsTarget) -> Option<(String, String)> {
        split_uri(target.target_key.as_str())
    }
    async fn call(
        &self,
        target: &FsTarget,
        mut args: Value,
        policy: Option<&SandboxExecutionPolicy>,
        signal: Option<AbortPredicate>,
    ) -> Result<Value, FsError> {
        let (id, path) = self
            .remote_target(target)
            .ok_or_else(|| fs_error("not a remote target".into()))?;
        if signal.as_ref().is_some_and(|s| s()) {
            return Err(FsError::new(
                "remote file operation cancelled",
                FsErrorCode::FsAborted,
            ));
        }
        args["path"] = json!(path);
        if let Some(policy) = policy {
            let (owner, _) = split_uri(&policy.workspace_root).ok_or_else(|| {
                fs_error(
                    "FS_SANDBOX_DENIED: local permission policy cannot authorize remote writes"
                        .into(),
                )
            })?;
            if owner != id {
                return Err(fs_error(
                    "FS_SANDBOX_DENIED: remote workspace identity mismatch".into(),
                ));
            }
        }
        self.remote
            .call(
                &id,
                "file",
                args,
                policy.map(|p| p.mode.as_str()).unwrap_or("read-only"),
                None,
                signal,
            )
            .await
            .map_err(fs_error)
    }
}
#[async_trait::async_trait]
impl FileSystem for RemoteFileSystem {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(SandboxMode::WorkspaceWrite)
    }
    async fn resolve(
        &self,
        path: &str,
        opts: Option<&ResolveOptions>,
    ) -> Result<FsTarget, FsError> {
        let remote = opts
            .and_then(|o| o.cwd.as_deref())
            .filter(|c| c.starts_with("dsh-remote://"));
        if let Some(cwd) = remote {
            let (row, tail) = self.remote.by_uri(cwd).map_err(fs_error)?;
            let path = if let Some((id, path)) = split_uri(path) {
                if id != row.connection.id {
                    return Err(fs_error("remote workspace crossing is not allowed".into()));
                }
                path
            } else {
                native_path(&row, &tail, path)
            };
            let value = self
                .remote
                .call(
                    &row.connection.id,
                    "file",
                    json!({"op":"resolve","path":path}),
                    "read-only",
                    None,
                    opts.and_then(|o| o.signal.clone()),
                )
                .await
                .map_err(fs_error)?;
            return Ok(target(
                &row.connection.id,
                value["path"]
                    .as_str()
                    .ok_or_else(|| fs_error("invalid remote path result".into()))?,
            ));
        }
        if path.starts_with("dsh-remote://") {
            let (id, native) =
                split_uri(path).ok_or_else(|| fs_error("invalid remote URI".into()))?;
            let root = format!("dsh-remote://{id}/");
            return self
                .resolve(
                    &native,
                    Some(&ResolveOptions {
                        cwd: Some(root),
                        signal: opts.and_then(|o| o.signal.clone()),
                    }),
                )
                .await;
        }
        self.local.resolve(path, opts).await
    }
    fn process_path(&self, target: &FsTarget) -> String {
        self.remote_target(target)
            .map(|(_, p)| p)
            .unwrap_or_else(|| self.local.process_path(target))
    }
    fn file_url(&self, target: &FsTarget) -> String {
        if self.remote_target(target).is_some() {
            target.display_path.clone()
        } else {
            self.local.file_url(target)
        }
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        match (self.remote_target(parent), self.remote_target(child)) {
            (Some((a, p)), Some((b, c))) => {
                a == b
                    && (p == c
                        || c.starts_with(&format!("{}/", p.trim_end_matches('/')))
                        || c.starts_with(&format!("{}\\", p.trim_end_matches('\\'))))
            }
            (None, None) => self.local.contains(parent, child),
            _ => false,
        }
    }
    async fn stat(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsInfo>, FsError> {
        if self.remote_target(target).is_none() {
            return self.local.stat(target, signal).await;
        }
        info(
            self.call(target, json!({"op":"stat"}), None, signal)
                .await?,
        )
    }
    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        if !path.starts_with("dsh-remote://")
            && !opts
                .and_then(|o| o.cwd.as_deref())
                .is_some_and(|c| c.starts_with("dsh-remote://"))
        {
            return self.local.lstat(path, opts, signal).await;
        }
        let target = self
            .resolve(
                path,
                Some(&ResolveOptions {
                    cwd: opts.and_then(|o| o.cwd.clone()),
                    signal: signal.clone(),
                }),
            )
            .await?;
        let value = self
            .call(&target, json!({"op":"lstat"}), None, signal)
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(FsPathInfo {
            version: fs_version(value["version"].as_str().unwrap_or_default()),
            kind: match value["kind"].as_str() {
                Some("file") => FsPathInfoType::File,
                Some("directory") => FsPathInfoType::Directory,
                Some("symlink") => FsPathInfoType::Symlink,
                _ => FsPathInfoType::Other,
            },
            size: value["size"].as_u64(),
        }))
    }
    async fn read_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<String, FsError> {
        if self.remote_target(target).is_none() {
            return self.local.read_text(target, signal).await;
        }
        let bytes = self.read_bytes(target, signal, 1024 * 1024).await?;
        if bytes.contains(&0) {
            return Err(FsError::new(
                "remote file is not text",
                FsErrorCode::FsNotText,
            ));
        }
        String::from_utf8(bytes)
            .map(|s| s.trim_start_matches('\u{feff}').replace("\r\n", "\n"))
            .map_err(|_| FsError::new("remote file is not UTF-8", FsErrorCode::FsNotText))
    }
    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
        if self.remote_target(target).is_none() {
            return self.local.stream_text(target, signal).await;
        }
        let text = self.read_text(target, signal).await?;
        Ok(Box::pin(futures::stream::once(async move { Ok(text) })))
    }
    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        if self.remote_target(target).is_none() {
            return self.local.read_bytes(target, signal, max_bytes).await;
        }
        let value = self
            .call(
                target,
                json!({"op":"read","maxBytes":max_bytes.min(2*1024*1024)}),
                None,
                signal,
            )
            .await?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                value["base64"]
                    .as_str()
                    .ok_or_else(|| fs_error("remote bytes missing".into()))?,
            )
            .map_err(|_| fs_error("invalid remote bytes".into()))?;
        if bytes.len() as u64 > max_bytes {
            return Err(FsError::new(
                "remote file exceeds byte limit",
                FsErrorCode::FsTooLarge,
            ));
        }
        Ok(bytes)
    }
    async fn list_dir(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError> {
        let Some((id, _)) = self.remote_target(target) else {
            return self.local.list_dir(target, signal).await;
        };
        let value = self
            .call(target, json!({"op":"list"}), None, signal)
            .await?;
        value["entries"]
            .as_array()
            .ok_or_else(|| fs_error("invalid remote listing".into()))?
            .iter()
            .map(|v| {
                Ok(FsDirEntry {
                    name: v["name"]
                        .as_str()
                        .ok_or_else(|| fs_error("invalid remote name".into()))?
                        .into(),
                    kind: kind(v["kind"].as_str()),
                    target: self::target(
                        &id,
                        v["path"]
                            .as_str()
                            .ok_or_else(|| fs_error("invalid remote path".into()))?,
                    ),
                    version: v["version"].as_str().map(fs_version),
                    size: v["size"].as_u64(),
                })
            })
            .collect()
    }
    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        if self.remote_target(target).is_none() {
            return self
                .local
                .write_text(target, content, expected, signal, policy)
                .await;
        }
        let value=self.call(target,json!({"op":"write","content":content,"createOnly":matches!(expected,Some(FsWriteIntent::CreateIfAbsent)),"expectedVersion":match expected{Some(FsWriteIntent::ReplaceIfVersion{version})=>Some(version.as_str()),_=>None}}),policy,signal).await?;
        Ok(FsWriteOutcome {
            operation: if value["operation"] == "create" {
                FsWriteOperation::Create
            } else {
                FsWriteOperation::Update
            },
            version: fs_version(value["version"].as_str().unwrap_or_default()),
            before: value["before"].as_str().map(str::to_string),
            after: value["after"].as_str().unwrap_or_default().into(),
        })
    }
    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        if self.remote_target(target).is_none() {
            return self
                .local
                .edit_text(target, edit, expected, signal, policy)
                .await;
        }
        let value=self.call(target,json!({"op":"edit","oldString":edit.old_string,"newString":edit.new_string,"replaceAll":edit.replace_all,"expectedVersion":expected.map(|g|g.version.as_str())}),policy,signal).await?;
        Ok(FsEditOutcome {
            version: fs_version(value["version"].as_str().unwrap_or_default()),
            before: value["before"].as_str().unwrap_or_default().into(),
            after: value["after"].as_str().unwrap_or_default().into(),
        })
    }
}

struct RemoteProfiles {
    local: Arc<dyn ExecutionProfileResolver>,
    remote: Arc<RemoteRuntime>,
}
impl ExecutionProfileResolver for RemoteProfiles {
    fn resolve(
        &self,
        session: Option<&str>,
        cwd: &str,
    ) -> Result<ResolvedExecutionProfile, String> {
        if !cwd.starts_with("dsh-remote://") {
            return self.local.resolve(session, cwd);
        }
        let (row, _) = self.remote.by_uri(cwd)?;
        Ok(ResolvedExecutionProfile {
            context_id: row.handshake.context_id,
            shell_path: row.handshake.shell_path,
            shell_kind: row.handshake.shell_kind,
            python_path: row.handshake.python_path,
            toolchain_paths: row.handshake.tools,
        })
    }
    fn validate(
        &self,
        request: ExecutionValidationRequest,
    ) -> BoxFuture<'static, Result<(), String>> {
        if !request.workdir.starts_with("dsh-remote://") {
            return self.local.validate(request);
        }
        let remote = self.remote.clone();
        Box::pin(async move {
            let (row, _) = remote.by_uri(&request.workdir)?;
            if request.execution_context_id.as_deref() != Some(row.handshake.context_id.as_str()) {
                return Err("remote environment snapshot changed".into());
            }
            let refreshed = remote.connect(row.connection).await?;
            if request.execution_context_id.as_deref()
                != Some(refreshed.handshake.context_id.as_str())
            {
                return Err("remote environment changed; resolve a fresh execution context".into());
            }
            Ok(())
        })
    }
    fn report_failure(&self, context_id: &str, capability: &str) {
        self.local.report_failure(context_id, capability)
    }
}

pub struct RemoteShell {
    local: Arc<dyn ShellExecutor>,
    remote: Arc<RemoteRuntime>,
}
impl RemoteShell {
    fn request(&self, spec: &ShellExecSpec) -> Result<(String, Execution), String> {
        let (row, tail) = self.remote.by_uri(&spec.workdir)?;
        let policy = spec
            .sandbox_policy
            .as_ref()
            .ok_or("remote execution requires a permission snapshot")?;
        let (owner, _) = split_uri(&policy.workspace_root)
            .ok_or("local policy cannot authorize remote execution")?;
        if owner != row.connection.id {
            return Err("remote permission workspace mismatch".into());
        }
        if spec.execution_context_id.as_deref() != Some(row.handshake.context_id.as_str()) {
            return Err("remote execution context mismatch".into());
        }
        let argv=spec.native_argv.clone().ok_or("remote execution requires execute_native or execute_script; a PowerShell string is not automatically translated")?;
        Ok((
            row.connection.id,
            Execution {
                execution_id: String::new(),
                context_id: row.handshake.context_id,
                workspace: row.handshake.workspace.clone(),
                permission_mode: policy.mode.as_str().into(),
                argv,
                cwd: if tail.is_empty() {
                    row.handshake.workspace
                } else {
                    tail
                },
                timeout_ms: spec.timeout_ms.min(3_600_000),
                env: spec.env.clone().unwrap_or_default(),
                stdin: spec.stdin.clone(),
            },
        ))
    }
}
fn decode(value: &Value, key: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(value[key]["base64"].as_str().unwrap_or_default())
        .map_err(|_| "invalid remote output encoding".into())
}
fn append_bounded(buffer: &mut Vec<u8>, bytes: &[u8], limit: usize) -> bool {
    let overflow = buffer.len().saturating_add(bytes.len()) > limit;
    buffer.extend_from_slice(bytes);
    if buffer.len() > limit {
        buffer.drain(..buffer.len() - limit);
    }
    overflow
}
async fn collect(
    remote: Arc<RemoteRuntime>,
    id: String,
    execution: Execution,
    signal: Option<ShellAbort>,
    limit: u64,
) -> Result<ShellRunResult, String> {
    let (execution_id, mut value) = remote.submit(&id, execution.clone()).await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let (mut out_offset, mut err_offset) = (0, 0);
    let (mut out_truncated, mut err_truncated) = (false, false);
    let mut cancelling = false;
    loop {
        out_truncated |= append_bounded(
            &mut stdout,
            &decode(&value, "stdout")?,
            limit.min(MAX_LOG) as usize,
        );
        err_truncated |= append_bounded(
            &mut stderr,
            &decode(&value, "stderr")?,
            limit.min(MAX_LOG) as usize,
        );
        out_offset = value["stdout"]["nextOffset"].as_u64().unwrap_or(out_offset);
        err_offset = value["stderr"]["nextOffset"].as_u64().unwrap_or(err_offset);
        let state = value["state"].as_str().unwrap_or("unknown");
        let fully_read = out_offset >= value["stdout"]["totalBytes"].as_u64().unwrap_or(0)
            && err_offset >= value["stderr"]["totalBytes"].as_u64().unwrap_or(0);
        if matches!(state, "completed" | "failed" | "cancelled" | "timed_out") && fully_read {
            let meta = format!(
                "\n[remote hostId={} backendId=ssh-helper contextId={} executionId={} state={}]\n",
                value["hostId"].as_str().unwrap_or_default(),
                execution.context_id,
                execution_id,
                state
            );
            stderr.extend_from_slice(meta.as_bytes());
            return Ok(ShellRunResult {
                execution_context_id: Some(execution.context_id.clone()),
                executable: execution.argv.first().cloned().unwrap_or_default(),
                stdout_total_bytes: value["stdout"]["totalBytes"].as_u64().unwrap_or(stdout.len() as u64),
                stderr_total_bytes: value["stderr"]["totalBytes"].as_u64().unwrap_or(stderr.len() as u64),
                exit_code: value["exitCode"].as_i64().map(|n| n as i32),
                signal: value["signal"].as_str().map(str::to_string),
                timed_out: state == "timed_out",
                aborted: state == "cancelled",
                timeout_ms: execution.timeout_ms,
                stdout: CollectedOutput {
                    text: String::from_utf8_lossy(&stdout).into_owned(),
                    truncated: out_truncated || value["stdoutTruncated"] == true,
                    spill_path: None,
                },
                stderr: CollectedOutput {
                    text: String::from_utf8_lossy(&stderr).into_owned(),
                    truncated: err_truncated || value["stderrTruncated"] == true,
                    spill_path: None,
                },
                sandbox: None,
            });
        }
        if state == "unknown" {
            return Err(format!(
                "[REMOTE_UNKNOWN] executionId={execution_id}; remote state cannot be confirmed; query original execution without replay"
            ));
        }
        if signal.as_ref().is_some_and(|s| s()) {
            cancelling = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        value=remote.query(&id,&execution_id,&execution.permission_mode,out_offset,err_offset,cancelling).await.map_err(|error|format!("[REMOTE_UNKNOWN] executionId={execution_id}; cancellation/completion unconfirmed; query without replay: {error}"))?;
    }
}
struct RemoteProcess {
    state: Arc<Mutex<Option<Result<ShellRunResult, String>>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    done: futures::future::Shared<BoxFuture<'static, ()>>,
    read: Mutex<bool>,
}
impl ShellProcess for RemoteProcess {
    fn status(&self) -> ShellProcessStatus {
        match self.state.lock().as_ref() {
            None => ShellProcessStatus::Running,
            Some(Ok(result)) if result.aborted => ShellProcessStatus::Killed,
            Some(_) => ShellProcessStatus::Completed,
        }
    }
    fn exit_code(&self) -> Option<i32> {
        self.state
            .lock()
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|r| r.exit_code)
    }
    fn signal(&self) -> Option<String> {
        self.state
            .lock()
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|r| r.signal.clone())
    }
    fn done(&self) -> BoxFuture<'static, ()> {
        self.done.clone().boxed()
    }
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }
    fn read_output(&self) -> ShellProcessRead {
        let state = self.state.lock();
        let mut read = self.read.lock();
        let (delta, lossy) = if *read {
            (String::new(), false)
        } else {
            match state.as_ref() {
                None => (String::new(), false),
                Some(Ok(result)) => {
                    *read = true;
                    (
                        format!("{}\n[stderr]\n{}", result.stdout.text, result.stderr.text),
                        result.stdout.truncated || result.stderr.truncated,
                    )
                }
                Some(Err(error)) => {
                    *read = true;
                    (error.clone(), false)
                }
            }
        };
        ShellProcessRead {
            delta,
            lossy,
            stdout_spill_path: None,
            stderr_spill_path: None,
        }
    }
    fn kill(&self) -> bool {
        if self.state.lock().is_some() {
            false
        } else {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Release);
            true
        }
    }
}
impl ShellExecutor for RemoteShell {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        self.local.sandbox_mode()
    }
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        self.local.resolve(request)
    }
    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        if !spec.workdir.starts_with("dsh-remote://") {
            return self.local.run(spec);
        }
        let request = self.request(&spec);
        let remote = self.remote.clone();
        Box::pin(async move {
            let (id, execution) = request?;
            collect(remote, id, execution, spec.signal, spec.stdout_max_bytes).await
        })
    }
    fn start(&self, spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        if !spec.workdir.starts_with("dsh-remote://") {
            return self.local.start(spec);
        }
        let state = Arc::new(Mutex::new(None));
        let stored = state.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancel.clone();
        let request = self.request(&spec);
        let remote = self.remote.clone();
        let done = async move {
            let result = match request {
                Ok((id, execution)) => {
                    collect(
                        remote,
                        id,
                        execution,
                        Some(Arc::new(move || {
                            flag.load(std::sync::atomic::Ordering::Acquire)
                                || spec.signal.as_ref().is_some_and(|s| s())
                        })),
                        spec.stdout_max_bytes,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            *stored.lock() = Some(result);
        }
        .boxed()
        .shared();
        tokio::spawn(done.clone());
        Arc::new(RemoteProcess {
            state,
            cancel,
            done,
            read: Mutex::new(false),
        })
    }
}

pub fn install_routes(ctx: &cordis::Context, remote: Arc<RemoteRuntime>) -> Result<(), String> {
    let fs = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .ok_or("local filesystem missing")?
        .as_ref()
        .clone();
    let shell = ctx
        .get_typed::<Arc<dyn ShellExecutor>>("shell", false)
        .ok_or("local shell missing")?
        .as_ref()
        .clone();
    let profiles = ctx
        .get_typed::<Arc<dyn ExecutionProfileResolver>>("executionProfiles", false)
        .ok_or("local execution profiles missing")?
        .as_ref()
        .clone();
    ctx.set(
        "fs",
        cordis::arc(Arc::new(RemoteFileSystem {
            local: fs,
            remote: remote.clone(),
        }) as Arc<dyn FileSystem>),
    )?;
    ctx.set(
        "shell",
        cordis::arc(Arc::new(RemoteShell {
            local: shell,
            remote: remote.clone(),
        }) as Arc<dyn ShellExecutor>),
    )?;
    ctx.set(
        "executionProfiles",
        cordis::arc(Arc::new(RemoteProfiles {
            local: profiles,
            remote: remote.clone(),
        }) as Arc<dyn ExecutionProfileResolver>),
    )?;
    ctx.register_service(remote as Arc<dyn dsh_workspace::WorkspaceExecutionProvider>);
    Ok(())
}

impl dsh_workspace::WorkspaceExecutionProvider for RemoteRuntime {
    fn verify(&self, uri: String) -> BoxFuture<'static, Result<String, String>> {
        let parsed = self.by_uri(&uri);
        Box::pin(async move {
            let (row, tail) = parsed?;
            if !tail.is_empty() {
                return Err("workspace registration requires the remote root URI".into());
            }
            Ok(row.connection.uri())
        })
    }
}
