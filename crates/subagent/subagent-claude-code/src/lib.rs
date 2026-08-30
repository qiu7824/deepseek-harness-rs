//! Fixed one-shot Claude Code provider. Each accepted run owns one real
//! unattended `claude` CLI process through the shared subprocess runtime.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dsh_llm::ContentBlock;
use dsh_session::{SessionId, session_id};
use dsh_subagent::{
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentError, SubagentProvider,
    SubagentResult, SubagentRun, SubagentStopReason,
};
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use futures::future::{BoxFuture, FutureExt, Shared};

pub const NAME: &str = "subagent-claude-code";
pub const PROVIDER_NAME: &str = "claude-code";
pub const DEFAULT_DISPOSE_GRACE_MS: u64 = 3_000;

#[derive(Debug, Clone)]
pub struct Config {
    pub env: Vec<(String, String)>,
    pub dispose_grace_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            env: Vec::new(),
            dispose_grace_ms: DEFAULT_DISPOSE_GRACE_MS,
        }
    }
}

fn text_task(prompt: &[ContentBlock]) -> Result<String, SubagentError> {
    if prompt.is_empty() {
        return Err(SubagentError::new(
            "INVALID_PROMPT",
            "subagent-claude-code: the one-shot task must contain only text blocks",
        ));
    }
    let mut text = String::new();
    for block in prompt {
        match block {
            ContentBlock::Text { text: part } => text.push_str(part),
            _ => {
                return Err(SubagentError::new(
                    "INVALID_PROMPT",
                    "subagent-claude-code: the one-shot task must contain only text blocks",
                ));
            }
        }
    }
    if text.trim().is_empty() {
        return Err(SubagentError::new(
            "INVALID_PROMPT",
            "subagent-claude-code: the one-shot task must not be empty",
        ));
    }
    Ok(text)
}

fn claude_argv(
    resolved: String,
    prompt: String,
    model: Option<String>,
) -> Result<Vec<String>, SubagentError> {
    if !Path::new(&resolved).is_absolute() {
        return Err(SubagentError::new(
            "PROVIDER_UNAVAILABLE",
            "subagent-claude-code: resolved Claude executable path was not absolute",
        ));
    }
    let mut argv = vec![
        resolved,
        "--print".to_string(),
        "--output-format".to_string(),
        "text".to_string(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
    ];
    if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
        argv.push("--model".to_string());
        argv.push(model);
    }
    argv.push("--".to_string());
    argv.push(prompt);
    Ok(argv)
}

struct ClaudeCodeRun {
    id: SessionId,
    child: Arc<dyn SubprocessHandle>,
    cancelled: Arc<AtomicBool>,
    result: Shared<BoxFuture<'static, Result<SubagentResult, String>>>,
    disposal: tokio::sync::OnceCell<Result<(), String>>,
}

#[async_trait::async_trait]
impl SubagentRun for ClaudeCodeRun {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn local_agent(&self) -> Option<Arc<dyn dsh_agent::Agent>> {
        None
    }

    async fn result(&self) -> Result<SubagentResult, String> {
        self.result.clone().await
    }

    async fn dispose(&self) -> Result<(), String> {
        self.disposal
            .get_or_init(|| async {
                self.cancelled.store(true, Ordering::SeqCst);
                self.child.terminate();
                if !self.child.wait_for_exit(None).await {
                    return Err("subagent-claude-code: process tree did not exit".to_string());
                }
                self.child.done().await.map(|_| ())
            })
            .await
            .clone()
    }
}

pub struct ClaudeCodeProvider {
    subprocess: Arc<dyn SubprocessRuntime>,
    config: Config,
}

impl ClaudeCodeProvider {
    pub fn new(subprocess: Arc<dyn SubprocessRuntime>, config: Config) -> Self {
        Self { subprocess, config }
    }
}

#[async_trait::async_trait]
impl SubagentProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        let prompt = text_task(&request.request.prompt)?;
        if (request.request.signal)() {
            return Err(SubagentError::new(
                "ABORTED",
                "subagent-claude-code: request was aborted before CLI startup",
            ));
        }
        let cwd = request
            .request
            .parent
            .session()
            .header()
            .cwd
            .clone()
            .ok_or_else(|| {
                SubagentError::new(
                    "INVALID_CWD",
                    "subagent-claude-code: no working directory for the child",
                )
            })?;
        if !Path::new(&cwd).is_absolute() || !Path::new(&cwd).is_dir() {
            return Err(SubagentError::new(
                "INVALID_CWD",
                "subagent-claude-code: parent cwd is not an accessible absolute directory",
            ));
        }
        let resolved = self
            .subprocess
            .resolve_executable(
                "claude",
                Some(&self.config.env),
                Some(request.request.signal.clone()),
            )
            .await
            .map_err(|error| {
                SubagentError::new(
                    "PROVIDER_UNAVAILABLE",
                    format!("subagent-claude-code: Claude Code is unavailable: {error}"),
                )
            })?;
        let model = request
            .request
            .agent_options
            .as_ref()
            .and_then(|options| options.model.clone());
        let argv = claude_argv(resolved, prompt, model)?;
        let child = self
            .subprocess
            .spawn(SubprocessSpawnSpec {
                argv,
                cwd,
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Ignore,
                    stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 4 * 1024 * 1024,
                        spill: None,
                    }),
                    stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 256 * 1024,
                        spill: None,
                    }),
                },
                grace_ms: self.config.dispose_grace_ms,
                signal: None,
                env: Some(
                    self.config
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), Some(value.clone())))
                        .collect(),
                ),
            })
            .map_err(|error| {
                SubagentError::new(
                    "PROVIDER_UNAVAILABLE",
                    format!("subagent-claude-code: failed to start Claude Code: {error}"),
                )
            })?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_result = cancelled.clone();
        let child_for_result = child.clone();
        let signal = request.request.signal.clone();
        let result = async move {
            let outcome = loop {
                if signal() || cancelled_for_result.load(Ordering::SeqCst) {
                    child_for_result.terminate();
                    let _ = child_for_result.wait_for_exit(None).await;
                    return Ok(SubagentResult {
                        output: Vec::new(),
                        structured: None,
                        stop_reason: SubagentStopReason::Aborted,
                    });
                }
                match tokio::time::timeout(Duration::from_millis(25), child_for_result.done()).await
                {
                    Ok(outcome) => break outcome?,
                    Err(_) => continue,
                }
            };
            let collected = child_for_result.collected();
            let stdout = collected
                .stdout
                .as_ref()
                .map(|reader| reader.read_from(0))
                .map(|read| read.text.trim().to_string())
                .unwrap_or_default();
            if outcome.exit_code == Some(0) && !stdout.is_empty() {
                Ok(SubagentResult {
                    output: vec![ContentBlock::Text { text: stdout }],
                    structured: None,
                    stop_reason: SubagentStopReason::Completed,
                })
            } else {
                Ok(SubagentResult {
                    output: Vec::new(),
                    structured: None,
                    stop_reason: SubagentStopReason::Error,
                })
            }
        }
        .boxed()
        .shared();
        Ok(Arc::new(ClaudeCodeRun {
            id: session_id(uuid::Uuid::new_v4().to_string()),
            child,
            cancelled,
            result,
            disposal: tokio::sync::OnceCell::new(),
        }))
    }
}

pub fn apply(ctx: &cordis::Context, config: &Config) -> Result<(), String> {
    let subagents = ctx
        .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "subagent-claude-code requires the subagents service".to_string())?;
    let subprocess = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "subagent-claude-code requires the subprocess service".to_string())?;
    subagents
        .register_provider(
            ctx,
            Arc::new(ClaudeCodeProvider::new(subprocess, config.clone())),
        )
        .map(|_| ())
        .map_err(|error| error.message)
}

#[cfg(test)]
mod tests {
    use super::claude_argv;

    #[test]
    fn configured_model_is_forwarded_to_claude_cli() {
        let executable = if cfg!(windows) {
            r"C:\claude.exe"
        } else {
            "/usr/bin/claude"
        };
        let argv = claude_argv(
            executable.to_string(),
            "task".to_string(),
            Some("claude-sonnet-4".to_string()),
        )
        .unwrap();
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["--model", "claude-sonnet-4"])
        );
    }
}
