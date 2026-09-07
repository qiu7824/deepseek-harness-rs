//! Fixed one-shot Codex provider. Each accepted run owns one real
//! `codex app-server --stdio` child process.

mod transcript;

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
    SubprocessHandle, SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessStdinMode, SubprocessStdio,
};
use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Cordis plugin name.
pub const NAME: &str = "subagent-codex";
/// Fixed provider registry name.
pub const PROVIDER_NAME: &str = "codex";
/// Default process-tree termination grace.
pub const DEFAULT_DISPOSE_GRACE_MS: u64 = 3_000;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_PROTOCOL_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_NOTIFICATION_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod wire_tests {
    use super::*;

    struct LiveProcess;
    impl SubprocessHandle for LiveProcess {
        fn stdin(&self) -> Option<Box<dyn tokio::io::AsyncWrite + Unpin + Send>> {
            None
        }
        fn stdout(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
            None
        }
        fn stderr(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
            None
        }
        fn collected(&self) -> dsh_subprocess::SubprocessCollectedOutputs {
            Default::default()
        }
        fn done(&self) -> BoxFuture<'static, Result<dsh_subprocess::SubprocessOutcome, String>> {
            futures::future::pending().boxed()
        }
        fn terminate(&self) {}
        fn wait_for_exit(
            &self,
            _: Option<dsh_subprocess::SubprocessAbort>,
        ) -> BoxFuture<'static, bool> {
            async { true }.boxed()
        }
    }

    #[tokio::test]
    async fn early_terminal_notification_is_retained_until_start_acknowledgement() {
        let (reader, mut writer) = tokio::io::duplex(4096);
        let mut wire = Wire::new(Box::new(reader), Box::new(tokio::io::sink()));
        writer.write_all(b"{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}\n{\"id\":1,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}\n").await.unwrap();
        let child: Arc<dyn SubprocessHandle> = Arc::new(LiveProcess);
        let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| false);
        let result = wire
            .request("turn/start", json!({}), &child, &signal)
            .await
            .unwrap();
        assert_eq!(result["turn"]["id"], "turn-1");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), wire.read())
                .await
                .expect("terminal notification must remain buffered")
                .unwrap()["method"],
            "turn/completed"
        );
        assert_eq!(wire.pending_bytes, 0);
    }

    #[tokio::test]
    async fn a_few_large_notifications_cannot_exceed_the_pending_byte_budget() {
        let (reader, mut writer) = tokio::io::duplex(64 * 1024);
        let mut wire = Wire::new(Box::new(reader), Box::new(tokio::io::sink()));
        let producer = tokio::spawn(async move {
            let mut notification = serde_json::to_vec(
                &json!({"method":"item/completed","params":{"output":"x".repeat(6 * 1024 * 1024)}}),
            )
            .unwrap();
            notification.push(b'\n');
            for _ in 0..3 {
                if writer.write_all(&notification).await.is_err() {
                    return;
                }
            }
            let _ = writer.write_all(b"{\"id\":1,\"result\":{}}\n").await;
        });
        let child: Arc<dyn SubprocessHandle> = Arc::new(LiveProcess);
        let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| false);
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            wire.request("turn/start", json!({}), &child, &signal),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().contains("16 MiB byte budget"));
        assert!(wire.pending_bytes <= MAX_PENDING_NOTIFICATION_BYTES);
        drop(wire);
        producer.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_read_preserves_a_partial_json_frame() {
        let (reader, mut writer) = tokio::io::duplex(4096);
        let mut wire = Wire::new(Box::new(reader), Box::new(tokio::io::sink()));
        writer.write_all(b"{\"method\":\"turn/com").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(15), wire.read())
                .await
                .is_err()
        );
        writer
            .write_all(b"pleted\",\"params\":{}}\n")
            .await
            .unwrap();
        let frame = wire.read().await.unwrap();
        assert_eq!(frame["method"], "turn/completed");
    }
}

/// Deployment-owned explicit environment and teardown grace.
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

fn codex_app_server_argv(resolved: String) -> Result<Vec<String>, SubagentError> {
    if !Path::new(&resolved).is_absolute() {
        return Err(SubagentError::new(
            "PROVIDER_UNAVAILABLE",
            "subagent-codex: resolved Codex executable path was not absolute",
        ));
    }
    Ok(vec![
        resolved,
        "app-server".to_string(),
        "--stdio".to_string(),
    ])
}

fn text_task(prompt: &[ContentBlock]) -> Result<Vec<String>, SubagentError> {
    if prompt.is_empty() {
        return Err(SubagentError::new(
            "INVALID_PROMPT",
            "subagent-codex: the one-shot task must contain only text blocks",
        ));
    }
    let mut texts = Vec::with_capacity(prompt.len());
    for block in prompt {
        match block {
            ContentBlock::Text { text } => texts.push(text.clone()),
            _ => {
                return Err(SubagentError::new(
                    "INVALID_PROMPT",
                    "subagent-codex: the one-shot task must contain only text blocks",
                ));
            }
        }
    }
    if texts.iter().all(|text| text.trim().is_empty()) {
        return Err(SubagentError::new(
            "INVALID_PROMPT",
            "subagent-codex: the one-shot task must not be empty",
        ));
    }
    Ok(texts)
}

struct Wire {
    input: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    output: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    next_id: u64,
    pending: std::collections::VecDeque<(Value, usize)>,
    pending_bytes: usize,
    last_frame_bytes: usize,
    line: Vec<u8>,
}

impl Wire {
    fn new(
        input: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        output: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    ) -> Self {
        Self {
            input: BufReader::new(input),
            output,
            next_id: 1,
            pending: Default::default(),
            pending_bytes: 0,
            last_frame_bytes: 0,
            line: Vec::new(),
        }
    }

    async fn write(&mut self, value: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| format!("subagent-codex: JSON-RPC encode failed: {error}"))?;
        bytes.push(b'\n');
        self.output
            .write_all(&bytes)
            .await
            .map_err(|error| format!("subagent-codex: app-server stdin failed: {error}"))?;
        self.output
            .flush()
            .await
            .map_err(|error| format!("subagent-codex: app-server stdin flush failed: {error}"))
    }

    async fn read(&mut self) -> Result<Value, String> {
        if let Some((frame, bytes)) = self.pending.pop_front() {
            self.pending_bytes = self.pending_bytes.saturating_sub(bytes);
            return Ok(frame);
        }
        self.read_raw().await
    }

    async fn read_raw(&mut self) -> Result<Value, String> {
        loop {
            let bytes =
                self.input.fill_buf().await.map_err(|error| {
                    format!("subagent-codex: app-server stdout failed: {error}")
                })?;
            if bytes.is_empty() && self.line.is_empty() {
                return Err("subagent-codex: app-server protocol stream closed".to_string());
            }
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(bytes.len(), |index| index + 1);
            if self.line.len().saturating_add(consumed) > MAX_PROTOCOL_FRAME_BYTES {
                return Err("subagent-codex: app-server frame exceeded 16 MiB".to_string());
            }
            let eof = bytes.is_empty();
            self.line.extend_from_slice(&bytes[..consumed]);
            self.input.consume(consumed);
            if newline.is_none() && !eof {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            self.last_frame_bytes = line.len();
            if !line.iter().all(u8::is_ascii_whitespace) {
                return serde_json::from_slice(&line).map_err(|error| {
                    format!("subagent-codex: invalid JSON-RPC line on stdout: {error}")
                });
            }
        }
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        child: &Arc<dyn SubprocessHandle>,
        signal: &Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        loop {
            if signal() {
                return Err("subagent-codex: app-server request aborted".to_string());
            }
            tokio::select! {
                frame = self.read_raw() => {
                    let frame = frame?;
                    if frame.get("method").is_some() && frame.get("id").is_some() {
                        self.write(&json!({"jsonrpc":"2.0", "id":frame["id"], "error":{
                            "code":-32601, "message":"This delegated client does not implement interactive server requests"
                        }})).await?;
                        return Err(format!("subagent-codex: interactive request {} requires an approval-capable client", frame["method"]));
                    }
                    if frame.get("id").and_then(Value::as_u64) != Some(id) {
                        if frame.get("method").is_some() {
                            if self.pending.len() >= 1024 || self.pending_bytes.saturating_add(self.last_frame_bytes) > MAX_PENDING_NOTIFICATION_BYTES {
                                return Err("subagent-codex: buffered notifications exceeded their count or 16 MiB byte budget".to_string());
                            }
                            self.pending_bytes += self.last_frame_bytes;
                            self.pending.push_back((frame, self.last_frame_bytes));
                        }
                        continue;
                    }
                    if let Some(error) = frame.get("error") {
                        return Err(format!("subagent-codex: app-server request {method:?} failed: {error}"));
                    }
                    return frame.get("result").cloned().ok_or_else(|| {
                        format!("subagent-codex: app-server returned no result for {method:?}")
                    });
                }
                outcome = child.done() => {
                    let outcome = outcome?;
                    return Err(format!(
                        "subagent-codex: app-server exited before the run settled (code {:?}, signal {:?})",
                        outcome.exit_code, outcome.signal,
                    ));
                }
                _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {}
            }
        }
    }
}

struct CodexRun {
    id: SessionId,
    child: Arc<dyn SubprocessHandle>,
    cancelled: Arc<AtomicBool>,
    result: Shared<BoxFuture<'static, Result<SubagentResult, String>>>,
    disposal: tokio::sync::OnceCell<Result<(), String>>,
}

#[async_trait::async_trait]
impl SubagentRun for CodexRun {
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
                let _ = tokio::time::timeout(Duration::from_millis(250), self.result.clone()).await;
                self.child.terminate();
                if !self.child.wait_for_exit(None).await {
                    return Err("subagent-codex: app-server process tree did not exit".to_string());
                }
                self.child.done().await.map(|_| ())
            })
            .await
            .clone()
    }
}

struct CodexProvider {
    subprocess: Arc<dyn SubprocessRuntime>,
    config: Config,
}

#[async_trait::async_trait]
impl SubagentProvider for CodexProvider {
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
        let texts = text_task(&request.request.prompt)?;
        if (request.request.signal)() {
            return Err(SubagentError::new(
                "ABORTED",
                "subagent-codex: request was aborted before app-server startup",
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
                    "subagent-codex: no working directory for the child — delegate from a parent session that has one",
                )
            })?;
        if !Path::new(&cwd).is_absolute() || !Path::new(&cwd).is_dir() {
            return Err(SubagentError::new(
                "INVALID_CWD",
                format!(
                    "subagent-codex: parent session cwd is not an accessible absolute directory: {cwd}"
                ),
            ));
        }

        let resolved = self
            .subprocess
            .resolve_executable(
                "codex",
                Some(&self.config.env),
                Some(request.request.signal.clone()),
            )
            .await
            .map_err(|error| {
                SubagentError::new(
                    "PROVIDER_UNAVAILABLE",
                    format!("subagent-codex: Codex app-server is unavailable: {error}"),
                )
            })?;
        let argv = codex_app_server_argv(resolved)?;

        let child = self
            .subprocess
            .spawn(SubprocessSpawnSpec {
                argv,
                cwd: cwd.clone(),
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Pipe,
                    stdout: SubprocessOutputMode::Pipe,
                    stderr: SubprocessOutputMode::Inherit,
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
                    format!("subagent-codex: failed to start Codex app-server: {error}"),
                )
            })?;
        let stdout = child.stdout().ok_or_else(|| {
            SubagentError::new(
                "START_FAILED",
                "subagent-codex: app-server stdout was not piped",
            )
        })?;
        let stdin = child.stdin().ok_or_else(|| {
            SubagentError::new(
                "START_FAILED",
                "subagent-codex: app-server stdin was not piped",
            )
        })?;
        let mut wire = Wire::new(stdout, stdin);
        let startup = async {
            let initialize = wire
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "deepseek-harness",
                            "title": "DeepSeek Harness",
                            "version": "0.0.1"
                        },
                        "capabilities": {
                            "experimentalApi": false,
                            "requestAttestation": false
                        }
                    }),
                    &child,
                    &request.request.signal,
                )
                .await?;
            if !initialize.is_object() {
                return Err(
                    "subagent-codex: app-server returned invalid initialize response".to_string(),
                );
            }
            wire.write(&json!({ "jsonrpc": "2.0", "method": "initialized" }))
                .await?;
            // Forward the configured model selected for this individual Codex child.
            let model = request
                .request
                .agent_options
                .as_ref()
                .and_then(|options| options.model.clone());
            let mut thread_params = json!({ "cwd": cwd, "ephemeral": true });
            if let Some(model) = model {
                thread_params["model"] = Value::String(model);
            }
            let thread = wire
                .request(
                    "thread/start",
                    thread_params,
                    &child,
                    &request.request.signal,
                )
                .await?;
            let thread = thread
                .get("thread")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    "subagent-codex: app-server returned invalid thread/start thread".to_string()
                })?;
            if thread.get("ephemeral").and_then(Value::as_bool) != Some(true) {
                return Err(
                    "subagent-codex: app-server did not create an ephemeral thread".to_string(),
                );
            }
            thread
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    "subagent-codex: app-server returned invalid thread/start thread id".to_string()
                })
        }
        .await;
        let thread_id = match startup {
            Ok(id) => id,
            Err(error) => {
                child.terminate();
                let _ = child.wait_for_exit(None).await;
                let _ = child.done().await;
                return Err(SubagentError::new("START_FAILED", error));
            }
        };

        let cancelled = Arc::new(AtomicBool::new(false));
        let transcript = match transcript::Transcript::create(&request).await {
            Ok(transcript) => transcript,
            Err(error) => {
                child.terminate();
                let _ = child.wait_for_exit(None).await;
                return Err(SubagentError::new("TRANSCRIPT_START_FAILED", error));
            }
        };
        let child_id = transcript
            .as_ref()
            .map(|t| t.session.id().clone())
            .unwrap_or_else(|| session_id(uuid::Uuid::new_v4().to_string()));
        let transcript_for_finish = transcript.clone();
        let cancelled_for_result = cancelled.clone();
        let child_for_result = child.clone();
        let signal = request.request.signal.clone();
        let effort = request
            .request
            .agent_options
            .as_ref()
            .and_then(|options| options.reasoning_effort.as_ref())
            .map(|effort| effort.as_str().to_string());
        let result = async move {
            let turn = match wire
                .request(
                    "turn/start",
                    json!({
                        "threadId": thread_id,
                        "effort": effort,
                        "input": texts
                            .into_iter()
                            .map(|text| json!({ "type": "text", "text": text, "text_elements": [] }))
                            .collect::<Vec<_>>()
                    }),
                    &child_for_result,
                    &signal,
                )
                .await
            {
                Ok(turn) => turn,
                Err(_) if signal() || cancelled_for_result.load(Ordering::SeqCst) => {
                    return Ok(SubagentResult {
                        output: Vec::new(),
                        structured: None,
                        stop_reason: SubagentStopReason::Aborted,
                    });
                }
                Err(error) => return Err(error),
            };
            let Some(turn_id) = turn
                .get("turn")
                .and_then(Value::as_object)
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
            else {
                return Ok(SubagentResult {
                    output: Vec::new(),
                    structured: None,
                    stop_reason: SubagentStopReason::Error,
                });
            };
            let mut final_answer: Option<String> = None;
            let mut unphased_answer: Option<String> = None;
            loop {
                if signal() || cancelled_for_result.load(Ordering::SeqCst) {
                    let interrupt_id = wire.next_id;
                    wire.next_id += 1;
                    let _ = wire
                        .write(&json!({
                            "jsonrpc": "2.0",
                            "id": interrupt_id,
                            "method": "turn/interrupt",
                            "params": { "threadId": thread_id, "turnId": turn_id }
                        }))
                        .await;
                    let selected = final_answer.or(unphased_answer);
                    return Ok(SubagentResult {
                        output: selected
                            .filter(|text| !text.trim().is_empty())
                            .map(|text| vec![ContentBlock::Text { text }])
                            .unwrap_or_default(),
                        structured: None,
                        stop_reason: SubagentStopReason::Aborted,
                    });
                }
                tokio::select! {
                    frame = wire.read() => {
                        let frame = frame?;
                        if frame.get("id").is_some() && frame.get("method").is_some() {
                            // Never leave an unsupported server request waiting
                            // forever (for example an approval interaction).
                            wire.write(&json!({"jsonrpc":"2.0", "id":frame["id"], "error":{
                                "code":-32601, "message":"This delegated client does not implement interactive server requests"
                            }})).await?;
                            return Err(format!("subagent-codex: interactive request {} requires an approval-capable client", frame["method"]));
                        }
                        let method = frame.get("method").and_then(Value::as_str);
                        let params = frame.get("params").and_then(Value::as_object);
                        if let (Some(transcript), Some(method), Some(params)) = (&transcript, method, params) {
                            if params.get("threadId").and_then(Value::as_str) == Some(thread_id.as_str())
                                && params.get("turnId").and_then(Value::as_str) == Some(turn_id.as_str()) {
                                transcript.observe(method, &Value::Object(params.clone()))?;
                            }
                        }
                        if method == Some("item/completed") {
                            let Some(params) = params else { continue };
                            if params.get("threadId").and_then(Value::as_str) != Some(thread_id.as_str())
                                || params.get("turnId").and_then(Value::as_str) != Some(turn_id.as_str())
                            {
                                continue;
                            }
                            let Some(item) = params.get("item").and_then(Value::as_object) else { continue };
                            if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                                continue;
                            }
                            let Some(text) = item.get("text").and_then(Value::as_str) else {
                                return Ok(SubagentResult {
                                    output: Vec::new(),
                                    structured: None,
                                    stop_reason: SubagentStopReason::Error,
                                });
                            };
                            match item.get("phase") {
                                Some(Value::String(phase)) if phase == "final_answer" => {
                                    final_answer = Some(text.to_string());
                                }
                                None | Some(Value::Null) => unphased_answer = Some(text.to_string()),
                                Some(Value::String(phase)) if phase == "commentary" => {}
                                _ => {
                                    return Ok(SubagentResult {
                                        output: Vec::new(),
                                        structured: None,
                                        stop_reason: SubagentStopReason::Error,
                                    });
                                }
                            }
                            continue;
                        }
                        if method != Some("turn/completed") {
                            continue;
                        }
                        let Some(params) = params else { continue };
                        if params.get("threadId").and_then(Value::as_str) != Some(thread_id.as_str()) {
                            continue;
                        }
                        let Some(terminal) = params.get("turn").and_then(Value::as_object) else { continue };
                        if terminal.get("id").and_then(Value::as_str) != Some(turn_id.as_str()) {
                            continue;
                        }
                        let selected = final_answer.or(unphased_answer);
                        let output = selected
                            .filter(|text| !text.trim().is_empty())
                            .map(|text| vec![ContentBlock::Text { text }])
                            .unwrap_or_default();
                        let status = terminal.get("status").and_then(Value::as_str);
                        let context_exceeded = status == Some("failed")
                            && terminal
                                .get("error")
                                .and_then(Value::as_object)
                                .and_then(|error| error.get("codexErrorInfo"))
                                .and_then(Value::as_str)
                                == Some("contextWindowExceeded");
                        let stop_reason = if status == Some("completed") {
                            SubagentStopReason::Completed
                        } else if status == Some("interrupted") {
                            SubagentStopReason::Aborted
                        } else if context_exceeded {
                            SubagentStopReason::MaxTokens
                        } else {
                            return Err(terminal.get("error").and_then(|error| error.get("message"))
                                .and_then(Value::as_str).unwrap_or("subagent-codex: turn failed without error detail").to_string());
                        };
                        return Ok(SubagentResult { output, structured: None, stop_reason });
                    }
                    _ = child_for_result.done() => {
                        let output = final_answer
                            .or(unphased_answer)
                            .filter(|text| !text.trim().is_empty())
                            .map(|text| vec![ContentBlock::Text { text }])
                            .unwrap_or_default();
                        return Ok(SubagentResult {
                            output,
                            structured: None,
                            stop_reason: if signal() || cancelled_for_result.load(Ordering::SeqCst) {
                                SubagentStopReason::Aborted
                            } else {
                                SubagentStopReason::Error
                            },
                        });
                    }
                    _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {}
                }
            }
        }
        .then(move |outcome| async move {
            if let Some(transcript) = transcript_for_finish {
                if let Err(error) = transcript.finish(&outcome).await {
                    return Err(format!("subagent-codex: transcript could not be flushed: {error}"));
                }
            }
            outcome
        })
        .boxed()
        .shared();
        let driven = result.clone();
        tokio::spawn(async move {
            let _ = driven.await;
        });

        Ok(Arc::new(CodexRun {
            id: child_id,
            child,
            cancelled,
            result,
            disposal: tokio::sync::OnceCell::new(),
        }))
    }
}

/// Register the fixed `codex` provider.
pub fn apply(ctx: &cordis::Context, config: &Config) -> Result<(), SubagentError> {
    if config.dispose_grace_ms == 0 || config.dispose_grace_ms > 2_147_483_647 {
        return Err(SubagentError::new(
            "INVALID_CONFIG",
            "subagent-codex: disposeGraceMs must be a positive finite number no greater than 2147483647",
        ));
    }
    let subagents = ctx
        .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            SubagentError::new("SERVICE_UNAVAILABLE", "subagents service is not mounted")
        })?;
    let subprocess = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            SubagentError::new("SERVICE_UNAVAILABLE", "subprocess service is not mounted")
        })?;
    let provider: Arc<dyn SubagentProvider> = Arc::new(CodexProvider {
        subprocess,
        config: config.clone(),
    });
    subagents.register_provider(ctx, provider).map(|_| ())
}
