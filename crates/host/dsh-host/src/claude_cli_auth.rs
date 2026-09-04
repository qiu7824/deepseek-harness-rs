//! Account handoff for the installed, unmodified Claude Code CLI.
//! Only `claude auth status` and `claude auth login` are invoked. Credentials
//! remain exclusively in the official client's own storage.
#[cfg(test)]
#[path = "claude_cli_auth_tests.rs"]
mod tests;
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const INSTALL_URL: &str = "https://code.claude.com/docs/en/setup";
pub(crate) const AUTH_URL: &str = "https://code.claude.com/docs/en/cli-reference";
const ID: &str = "claude-code";
const PREFIX: &str = "claude-cli-";

#[derive(Clone)]
enum LoginState {
    Pending,
    Complete,
    Cancelled,
    Failed(String),
}
struct Attempt {
    id: String,
    child: Arc<dyn SubprocessHandle>,
    cancel: Arc<AtomicBool>,
    state: Arc<parking_lot::Mutex<LoginState>>,
    expires_at: u64,
}

pub(crate) struct ClaudeCliAuth {
    subprocess: Arc<dyn SubprocessRuntime>,
    cwd: String,
    attempt: tokio::sync::Mutex<Option<Arc<Attempt>>>,
    status_timeout: Duration,
    login_timeout: Duration,
}

impl ClaudeCliAuth {
    pub(crate) fn new(subprocess: Arc<dyn SubprocessRuntime>, cwd: String) -> Arc<Self> {
        Arc::new(Self {
            subprocess,
            cwd,
            attempt: tokio::sync::Mutex::new(None),
            status_timeout: Duration::from_secs(8),
            login_timeout: Duration::from_secs(600),
        })
    }

    async fn executable(&self) -> Result<String, String> {
        tokio::time::timeout(
            Duration::from_secs(3),
            self.subprocess.resolve_executable("claude", None, None),
        )
        .await
        .map_err(|_| "Claude Code 客户端查找超时".to_string())?
        .map_err(|_| "未找到 Claude Code，请先安装官方客户端".to_string())
    }

    fn spawn(
        &self,
        executable: String,
        operation: &str,
        cancelled: Option<Arc<AtomicBool>>,
    ) -> Result<Arc<dyn SubprocessHandle>, String> {
        // The status JSON is small and projected onto loggedIn below. Login
        // output is drained with only one byte retained and is never read.
        let stdout_limit = if operation == "status" { 32 * 1024 } else { 1 };
        self.subprocess
            .spawn(SubprocessSpawnSpec {
                argv: vec![executable, "auth".into(), operation.into()],
                cwd: self.cwd.clone(),
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Ignore,
                    stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: stdout_limit,
                        spill: None,
                    }),
                    stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 1,
                        spill: None,
                    }),
                },
                grace_ms: 500,
                signal: cancelled.map(|flag| {
                    Arc::new(move || flag.load(Ordering::SeqCst)) as dsh_subprocess::SubprocessAbort
                }),
                env: None,
            })
            .map_err(|_| "无法启动 Claude Code，请检查官方客户端安装".to_string())
    }

    /// Read-only official CLI status. Never exposes the CLI JSON or tokens.
    pub(crate) async fn status(&self) -> Value {
        let executable = match self.executable().await {
            Ok(path) => path,
            Err(message) => return status_value(false, false, "unavailable", Some(message)),
        };
        let child = match self.spawn(executable, "status", None) {
            Ok(child) => child,
            Err(message) => return status_value(true, false, "error", Some(message)),
        };
        let result = tokio::time::timeout(self.status_timeout, child.done()).await;
        match result {
            Ok(Ok(outcome)) => {
                let output = child.collected().stdout.map(|reader| reader.read_from(0));
                let logged_in = output
                    .filter(|output| !output.lossy)
                    .and_then(|output| serde_json::from_str::<Value>(&output.text).ok())
                    .and_then(|value| value.get("loggedIn").and_then(Value::as_bool));
                match (outcome.exit_code,logged_in) {
                    (Some(0),Some(true))=>status_value(true,true,"ready",None),
                    (Some(1),Some(false)) | (Some(0),Some(false))=>status_value(true,false,"signedOut",None),
                    _=>status_value(true,false,"error",Some("无法识别 Claude Code 登录状态，请在终端运行 claude auth status 或更新官方客户端".into())),
                }
            }
            Ok(Err(_)) => status_value(
                true,
                false,
                "error",
                Some("Claude Code 状态检查失败".into()),
            ),
            Err(_) => {
                stop(&child).await;
                status_value(
                    true,
                    false,
                    "error",
                    Some("Claude Code 状态检查超时，请在终端检查官方客户端".into()),
                )
            }
        }
    }

    /// Called only by an explicit login click. No terminal or token importing.
    pub(crate) async fn start(self: &Arc<Self>) -> Result<Value, String> {
        let mut slot = self.attempt.lock().await;
        if let Some(attempt) = slot.as_ref() {
            if matches!(*attempt.state.lock(), LoginState::Pending) {
                return Ok(pending_value(attempt));
            }
        }
        let executable = self.executable().await?;
        let cancel = Arc::new(AtomicBool::new(false));
        let child = self.spawn(executable, "login", Some(cancel.clone()))?;
        let attempt = Arc::new(Attempt {
            id: format!("{PREFIX}{}", uuid::Uuid::new_v4()),
            child: child.clone(),
            cancel,
            state: Arc::new(parking_lot::Mutex::new(LoginState::Pending)),
            expires_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + self.login_timeout.as_secs(),
        });
        *slot = Some(attempt.clone());
        let worker = attempt.clone();
        let weak = Arc::downgrade(self);
        let timeout = self.login_timeout;
        tokio::spawn(async move {
            let result = tokio::time::timeout(timeout, child.done()).await;
            let next = match result {
                Ok(Ok(outcome)) if outcome.exit_code == Some(0) => {
                    if let Some(owner) = weak.upgrade() {
                        if owner.status().await["signedIn"] == true {
                            LoginState::Complete
                        } else {
                            LoginState::Failed("官方客户端已退出，但尚未确认登录；请运行 claude auth login 完成登录后刷新".into())
                        }
                    } else {
                        return;
                    }
                }
                Ok(_) => LoginState::Failed(
                    "官方登录未完成；如需终端交互，请运行 claude auth login，完成后刷新此页".into(),
                ),
                Err(_) => {
                    stop(&child).await;
                    LoginState::Failed(
                        "官方客户端登录已超时；请运行 claude auth login 完成登录后刷新".into(),
                    )
                }
            };
            let mut state = worker.state.lock();
            if matches!(*state, LoginState::Pending) {
                *state = next;
            }
        });
        Ok(pending_value(&attempt))
    }

    pub(crate) async fn poll(&self, id: &str) -> Result<Value, String> {
        let slot = self.attempt.lock().await;
        let attempt = slot
            .as_ref()
            .filter(|attempt| attempt.id == id)
            .ok_or("Claude Code 登录请求已结束")?;
        let state = attempt.state.lock().clone();
        match state {
            LoginState::Pending => Ok(pending_value(attempt)),
            LoginState::Complete => {
                Ok(json!({"status":"complete","provider":ID,"scope":"subagent"}))
            }
            LoginState::Cancelled => {
                Ok(json!({"status":"cancelled","provider":ID,"scope":"subagent"}))
            }
            LoginState::Failed(message) => Err(message),
        }
    }

    pub(crate) async fn cancel(&self, id: &str) -> Result<Value, String> {
        let attempt = {
            let slot = self.attempt.lock().await;
            slot.as_ref()
                .filter(|attempt| attempt.id == id)
                .cloned()
                .ok_or("Claude Code 登录请求已结束")?
        };
        {
            let mut state = attempt.state.lock();
            if matches!(*state, LoginState::Complete) {
                return Ok(json!({"status":"complete","provider":ID,"scope":"subagent"}));
            }
            *state = LoginState::Cancelled;
            attempt.cancel.store(true, Ordering::SeqCst);
        }
        stop(&attempt.child).await;
        // The official process can commit its own login immediately before
        // cancellation. Report that fact; never log the user out implicitly.
        if self.status().await["signedIn"] == true {
            *attempt.state.lock() = LoginState::Complete;
            return Ok(json!({"status":"complete","provider":ID,"scope":"subagent"}));
        }
        Ok(json!({"status":"cancelled","provider":ID,"scope":"subagent"}))
    }
}

impl Drop for ClaudeCliAuth {
    fn drop(&mut self) {
        if let Some(attempt) = self.attempt.get_mut().as_ref() {
            if matches!(*attempt.state.lock(), LoginState::Pending) {
                attempt.cancel.store(true, Ordering::SeqCst);
                attempt.child.terminate();
            }
        }
    }
}
async fn stop(child: &Arc<dyn SubprocessHandle>) {
    child.terminate();
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait_for_exit(None)).await;
}
fn status_value(installed: bool, signed_in: bool, status: &str, error: Option<String>) -> Value {
    json!({"id":ID,"name":"Claude Code","scope":"subagent","installed":installed,"signedIn":signed_in,"status":status,"installUrl":INSTALL_URL,"docsUrl":AUTH_URL,"error":error})
}
fn pending_value(attempt: &Attempt) -> Value {
    json!({"attempt":attempt.id,"provider":ID,"scope":"subagent","mode":"cli","status":"pending","interval":2,"expiresAt":attempt.expires_at,"docsUrl":AUTH_URL})
}
