use cordis::{Context, make_disposer};
use dsh_app_boot::plugin_profile::{Profile, read_runtime};
use dsh_host_apiproxy::{
    fetch::handler::AbortSignal,
    proxy::{ApiProxyService, PluginClientReady, PluginOperationControl},
};
use dsh_subprocess::{
    SubprocessCollect, SubprocessOutputMode, SubprocessRunGuard, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use futures::{FutureExt, future::BoxFuture};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn terminal(phase: &str) -> bool {
    matches!(
        phase,
        "succeeded" | "failed" | "cancelled" | "interrupted" | "recovery-required"
    )
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    version: u8,
    pub operation_id: String,
    pub action: String,
    pub spec: String,
    pub phase: String,
    pub log: String,
    pub error: Option<String>,
    pub restart_required: bool,
    pub effects: String,
    started_at: u64,
    finished_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
}
struct Operation {
    state: Mutex<Status>,
    cancelled: AtomicBool,
    done: tokio::sync::watch::Sender<bool>,
    signal: AbortSignal,
    client_required: bool,
    client: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}
impl Operation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.signal.abort();
    }
    fn record(&self, message: &str) {
        let mut state = self.state.lock();
        state.log.push_str(message);
        state.log.push('\n');
        if state.log.len() > 8192 {
            let mut cut = state.log.len() - 8192;
            while !state.log.is_char_boundary(cut) {
                cut += 1;
            }
            state.log.drain(..cut);
        }
        if message.contains("恢复") {
            state.phase = "rolling-back".into();
        } else if message == "提交插件配置" {
            state.phase = "committing".into();
        }
    }
}
struct WaitGuard {
    operation: Arc<Operation>,
    finished: bool,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.operation.cancel();
        }
    }
}
type Runner = Arc<dyn Fn(Arc<Operation>) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
struct State {
    disposing: bool,
    active: Option<Arc<Operation>>,
    history: VecDeque<Arc<Operation>>,
}
pub(super) struct Manager {
    directory: PathBuf,
    profile: PathBuf,
    state: Mutex<State>,
    runner: Runner,
}

fn read_status(path: &Path) -> Option<Status> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(32769)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 32768 {
        return None;
    }
    let status: Status = serde_json::from_slice(&bytes).ok()?;
    (status.version == 1 && uuid::Uuid::parse_str(&status.operation_id).is_ok()).then_some(status)
}
fn committed(profile: &Path, id: &str) -> bool {
    let mut bytes = Vec::new();
    let Ok(file) = std::fs::File::open(profile.join(".dsh-plugin-last-operation.json")) else {
        return false;
    };
    if file.take(16385).read_to_end(&mut bytes).is_err() || bytes.len() > 16384 {
        return false;
    }
    serde_json::from_slice::<Value>(&bytes)
        .is_ok_and(|receipt| receipt["operationId"] == id && receipt["committed"] == true)
}
impl Manager {
    pub(super) fn install(
        ctx: &Context,
        home: PathBuf,
        profile: String,
        runtime: Arc<dyn SubprocessRuntime>,
        api: Arc<ApiProxyService>,
    ) -> Arc<Self> {
        let runner_home = home.clone();
        let runner_profile = profile.clone();
        let runner: Runner = Arc::new(move |operation| {
            let runtime = runtime.clone();
            let home = runner_home.clone();
            let profile = runner_profile.clone();
            let api = api.clone();
            Box::pin(async move {
                let status = operation.state.lock().clone();
                if matches!(status.action.as_str(), "enable" | "disable") {
                    let progress_operation = operation.clone();
                    let ready = if operation.client_required {
                        let waiting = operation.clone();
                        Some(Arc::new(move || {
                            let operation = waiting.clone();
                            Box::pin(async move {
                                if operation.signal.aborted() {
                                    return Err("插件启停已取消".into());
                                }
                                operation.state.lock().phase = "awaiting-client".into();
                                operation.record("等待浏览器完成插件切换");
                                let mut client = operation.client.subscribe();
                                tokio::select! {
                                    result=client.wait_for(|result|result.is_some())=>result.map_err(|_|"浏览器确认通道关闭".to_string())?.clone().ok_or("浏览器确认缺失".to_string())?,
                                    _=operation.signal.cancelled()=>Err("插件启停已取消".into()),
                                    _=tokio::time::sleep(Duration::from_secs(45))=>Err("浏览器未完成插件切换，恢复原配置".into()),
                                }
                            }) as BoxFuture<'static, Result<(), String>>
                        }) as PluginClientReady)
                    } else {
                        None
                    };
                    let result = api
                        .apply_plugin_enablement(
                            status.spec,
                            status.action == "enable",
                            operation.signal.clone(),
                            Some(status.operation_id),
                            Arc::new(move |message| progress_operation.record(message)),
                            ready,
                        )
                        .await?;
                    operation.state.lock().result =
                        Some(serde_json::to_value(result).map_err(|error| error.to_string())?);
                    return Ok(());
                }
                let mut argv = vec![
                    std::env::current_exe()
                        .map_err(|error| error.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                    "plugin".into(),
                    "--profile".into(),
                    profile,
                    status.action,
                ];
                if !status.spec.is_empty() {
                    argv.push(status.spec);
                }
                let cancel = operation.clone();
                let output = SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 8192,
                    spill: None,
                });
                let child = runtime.spawn(SubprocessSpawnSpec {
                    argv,
                    cwd: home.to_string_lossy().into_owned(),
                    stdio: SubprocessStdio {
                        stdin: SubprocessStdinMode::Ignore,
                        stdout: output.clone(),
                        stderr: output,
                    },
                    grace_ms: 2000,
                    signal: Some(Arc::new(move || cancel.cancelled.load(Ordering::Acquire))),
                    env: Some(vec![
                        ("DSH_HOME".into(), Some(home.to_string_lossy().into_owned())),
                        ("DSH_PLUGIN_OPERATION_ID".into(), Some(status.operation_id)),
                        ("GIT_TERMINAL_PROMPT".into(), Some("0".into())),
                    ]),
                })?;
                let mut guard = SubprocessRunGuard::new(child.clone());
                let deadline = Instant::now() + Duration::from_secs(120);
                let mut timed_out = false;
                let done = child.done();
                tokio::pin!(done);
                let result = loop {
                    let captured = child.collected();
                    operation.state.lock().log = format!(
                        "{}{}",
                        captured
                            .stdout
                            .map(|reader| reader.read_from(0).text)
                            .unwrap_or_default(),
                        captured
                            .stderr
                            .map(|reader| reader.read_from(0).text)
                            .unwrap_or_default()
                    );
                    tokio::select! {
                        result=&mut done=>break result,
                        _=tokio::time::sleep(Duration::from_millis(150))=>{
                            if Instant::now()>=deadline {timed_out=true;child.terminate();}
                        }
                    }
                };
                child.terminate();
                if !child.wait_for_exit(None).await {
                    return Err(
                        "PROCESS_TREE_STILL_RUNNING: 插件操作进程尚未完全退出，保留恢复记录".into(),
                    );
                }
                guard.disarm();
                let captured = child.collected();
                operation.state.lock().log = format!(
                    "{}{}",
                    captured
                        .stdout
                        .map(|reader| reader.read_from(0).text)
                        .unwrap_or_default(),
                    captured
                        .stderr
                        .map(|reader| reader.read_from(0).text)
                        .unwrap_or_default()
                );
                if timed_out {
                    return Err("插件操作超过 120 秒，已结束所属进程并进入恢复检查".into());
                }
                let outcome = result?;
                if outcome.exit_code != Some(0) {
                    return Err(format!("插件操作退出：{:?}", outcome.exit_code));
                }
                Ok(())
            })
        });
        let own = Self::with_runner(home, profile, runner);
        let weak = Arc::downgrade(&own);
        ctx.register_service(Arc::new(PluginOperationControl {
            run: Arc::new(move |entry, enabled, signal| {
                let weak = weak.clone();
                Box::pin(async move {
                    let manager = weak.upgrade().ok_or("插件操作服务已关闭")?;
                    manager.execute_enablement(entry, enabled, signal).await
                })
            }),
        }));
        let teardown = own.clone();
        let _ = ctx.effect(
            "plugin operation teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let own = teardown.clone();
                    Box::pin(async move {
                        own.stop().await;
                    })
                }))
            }),
        );
        own
    }
    fn with_runner(home: PathBuf, profile: String, runner: Runner) -> Arc<Self> {
        let directory = home.join("plugin-operations").join(&profile);
        let profile = home.join("profiles").join(profile);
        let recovery = Profile::open(&profile).map(drop);
        let mut previous: Vec<Status> = Vec::new();
        for entry in std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            if let Some(status) = read_status(&entry.path()) {
                previous.push(status);
                previous.sort_by_key(|status| std::cmp::Reverse(status.started_at));
                previous.truncate(20);
            }
        }
        for status in &mut previous {
            if !terminal(&status.phase) {
                if recovery.is_ok() && committed(&profile, &status.operation_id) {
                    status.phase = "succeeded".into();
                    status.restart_required = true;
                    status.effects = "committed".into();
                } else if let Err(error) = &recovery {
                    status.phase = "recovery-required".into();
                    status.error = Some(error.clone());
                    status.effects = "inspect-required".into();
                } else {
                    status.phase = "interrupted".into();
                    status.error = Some("上次操作未完成，已检查并恢复未提交配置".into());
                    status.effects = "rolled-back-or-unchanged".into();
                }
                status.finished_at = Some(now());
                let _ = dsh_workspace_resources::persist_json(
                    &directory.join(format!("{}.json", status.operation_id)),
                    status,
                );
            }
        }
        Arc::new(Self {
            directory,
            profile,
            state: Mutex::new(State {
                disposing: false,
                active: None,
                history: previous
                    .into_iter()
                    .map(|state| {
                        Arc::new(Operation {
                            state: Mutex::new(state),
                            cancelled: AtomicBool::new(false),
                            done: tokio::sync::watch::channel(true).0,
                            signal: AbortSignal::new(),
                            client_required: false,
                            client: tokio::sync::watch::channel(None).0,
                        })
                    })
                    .collect(),
            }),
            runner,
        })
    }
    fn persist(&self, status: &Status) -> Result<(), String> {
        dsh_workspace_resources::persist_json(
            &self.directory.join(format!("{}.json", status.operation_id)),
            status,
        )
    }
    pub(super) fn dispatch(self: &Arc<Self>, input: Value) -> Result<Value, String> {
        let action = input["action"].as_str().ok_or("缺少插件操作")?;
        if action == "status" {
            let state = self.state.lock();
            let selected = if let Some(id) = input["operationId"].as_str() {
                state
                    .history
                    .iter()
                    .find(|operation| operation.state.lock().operation_id == id)
                    .cloned()
            } else {
                state
                    .active
                    .clone()
                    .or_else(|| state.history.front().cloned())
            };
            let configuration_error = if state.active.is_some() {
                None
            } else {
                read_runtime(&self.profile).issue
            };
            return Ok(
                json!({"operation":selected.map(|operation|operation.state.lock().clone()),"configurationError":configuration_error}),
            );
        }
        if action == "cancel" {
            let id = input["operationId"].as_str().ok_or("缺少操作标识")?;
            let operation = self
                .state
                .lock()
                .history
                .iter()
                .find(|operation| operation.state.lock().operation_id == id)
                .cloned()
                .ok_or("插件操作不存在")?;
            let mut status = operation.state.lock();
            if !terminal(&status.phase) {
                operation.cancel();
                status.phase = "cancelling".into();
                self.persist(&status)?;
            }
            return Ok(json!({"operation":status.clone()}));
        }
        if action == "client-result" {
            let id = input["operationId"].as_str().ok_or("缺少操作标识")?;
            let operation = self
                .state
                .lock()
                .history
                .iter()
                .find(|operation| operation.state.lock().operation_id == id)
                .cloned()
                .ok_or("插件操作不存在")?;
            if !operation.client_required {
                return Err("该操作不需要浏览器确认".into());
            }
            let outcome = if input["ok"] == true {
                Ok(())
            } else {
                Err(input["error"]
                    .as_str()
                    .unwrap_or("浏览器插件切换失败")
                    .chars()
                    .take(1024)
                    .collect::<String>())
            };
            let state = operation.state.lock();
            if terminal(&state.phase) {
                return Ok(json!({"operation":state.clone()}));
            }
            if let Some(previous) = operation.client.borrow().as_ref() {
                if previous != &outcome {
                    return Err("浏览器确认与已提交结果冲突".into());
                }
                return Ok(json!({"operation":state.clone()}));
            }
            if state.phase != "awaiting-client" {
                return Err("插件操作尚未进入浏览器确认阶段".into());
            }
            operation.client.send_replace(Some(outcome));
            return Ok(json!({"operation":state.clone()}));
        }
        if !matches!(action, "add" | "remove" | "recover" | "enable" | "disable") {
            return Err("不支持的插件操作".into());
        }
        let spec = input["spec"].as_str().unwrap_or("").trim().to_string();
        if spec.len() > 400
            || spec.chars().any(char::is_control)
            || action != "recover" && spec.is_empty()
        {
            return Err("插件来源或包名无效".into());
        }
        if action == "add" {
            let source = spec
                .strip_prefix("github:")
                .ok_or("安装来源须为 github:owner/repo#完整提交 SHA")?;
            let (repo, commit) = source.split_once('#').ok_or("必须指定完整提交 SHA")?;
            if commit.len() != 40
                || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
                || repo.split('/').count() != 2
                || repo.split('/').any(|part| {
                    part.is_empty()
                        || !part.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                        })
                })
            {
                return Err("GitHub 插件来源或提交 SHA 无效".into());
            }
        } else if action == "remove" {
            dsh_app_boot::plugin_profile::package_path(&self.profile, &spec)?;
        }
        let status = Status {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            action: action.into(),
            spec: if action == "recover" {
                String::new()
            } else {
                spec
            },
            phase: "running".into(),
            log: String::new(),
            error: None,
            restart_required: false,
            effects: "pending".into(),
            started_at: now(),
            finished_at: None,
            result: None,
        };
        let operation = Arc::new(Operation {
            state: Mutex::new(status.clone()),
            cancelled: AtomicBool::new(false),
            done: tokio::sync::watch::channel(false).0,
            signal: AbortSignal::new(),
            client_required: matches!(action, "enable" | "disable") && input["clientAck"] == true,
            client: tokio::sync::watch::channel(None).0,
        });
        {
            let mut state = self.state.lock();
            if state.disposing {
                return Err("插件管理正在关闭".into());
            }
            if state.active.is_some() {
                return Err("另一项插件操作正在执行".into());
            }
            self.persist(&status)?;
            state.active = Some(operation.clone());
            state.history.push_front(operation.clone());
            while state.history.len() > 20 {
                if let Some(old) = state.history.pop_back() {
                    let _ = std::fs::remove_file(
                        self.directory
                            .join(format!("{}.json", old.state.lock().operation_id)),
                    );
                }
            }
        }
        let own = self.clone();
        let runner = self.runner.clone();
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(runner(operation.clone()))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("插件操作发生异常".into()));
            let root = own.profile.clone();
            let id = operation.state.lock().operation_id.clone();
            let recovery = if result.as_ref().err().is_some_and(|error| {
                error.starts_with("PROCESS_TREE_STILL_RUNNING:")
                    || error.starts_with("PLUGIN_RUNTIME_RECOVERY_REQUIRED:")
            }) {
                Err(result.as_ref().err().unwrap().clone())
            } else {
                tokio::task::spawn_blocking(move || {
                    let _profile = Profile::open(&root)?;
                    Ok::<_, String>(committed(&root, &id))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            };
            {
                let mut status = operation.state.lock();
                match recovery {
                    Err(error) => {
                        status.phase = "recovery-required".into();
                        status.error = Some(error);
                        status.effects = "inspect-required".into();
                    }
                    Ok(was_committed) => {
                        if was_committed
                            || result.is_ok() && !operation.cancelled.load(Ordering::Acquire)
                        {
                            status.phase = "succeeded".into();
                            status.restart_required =
                                !matches!(status.action.as_str(), "enable" | "disable");
                            status.effects = "committed".into();
                        } else {
                            status.phase = if operation.cancelled.load(Ordering::Acquire) {
                                "cancelled"
                            } else {
                                "failed"
                            }
                            .into();
                            status.error = result.err();
                            status.effects = "rolled-back-or-unchanged".into();
                        }
                    }
                }
                status.finished_at = Some(now());
                if let Err(error) = own.persist(&status) {
                    status.error = Some(format!("操作状态保存失败：{error}"));
                }
            }
            own.state.lock().active = None;
            operation.done.send_replace(true);
        });
        Ok(json!({"operation":status}))
    }
    async fn stop(&self) {
        let active = {
            let mut state = self.state.lock();
            state.disposing = true;
            state.active.clone()
        };
        if let Some(active) = active {
            active.cancel();
            let mut done = active.done.subscribe();
            let _ = done.wait_for(|done| *done).await;
        }
    }
    async fn execute_enablement(
        self: &Arc<Self>,
        entry: String,
        enabled: bool,
        signal: AbortSignal,
    ) -> Result<dsh_host_plugin_inventory::PluginSetEnabledResult, String> {
        let started =
            self.dispatch(json!({"action":if enabled {"enable"} else {"disable"},"spec":entry}))?;
        let id = started["operation"]["operationId"]
            .as_str()
            .ok_or("插件操作缺少标识")?;
        let operation = self
            .state
            .lock()
            .history
            .iter()
            .find(|operation| operation.state.lock().operation_id == id)
            .cloned()
            .ok_or("插件操作不存在")?;
        let mut guard = WaitGuard {
            operation: operation.clone(),
            finished: false,
        };
        let mut done = operation.done.subscribe();
        tokio::select! {result=done.wait_for(|done|*done)=>{result.map_err(|_|"插件操作未完成".to_string())?;},_=signal.cancelled()=>operation.cancel()}
        done.wait_for(|done| *done)
            .await
            .map_err(|_| "插件操作未完成".to_string())?;
        guard.finished = true;
        let status = operation.state.lock().clone();
        if status.phase != "succeeded" {
            return Err(status.error.unwrap_or_else(|| "插件启停已取消".into()));
        }
        serde_json::from_value(status.result.ok_or("插件启停缺少完成结果")?)
            .map_err(|error| error.to_string())
    }
}
