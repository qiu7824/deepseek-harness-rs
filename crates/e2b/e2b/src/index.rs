//! Shared ownership of one E2B sandbox. Capability adapters await the same
//! SDK handle, so filesystem and process operations inhabit one remote
//! Linux world. Rust port of `packages/e2b/e2b/src/index.ts`.
//!
//! # Deviations
//!
//! - The `e2b` npm SDK boundary collapses into the [`E2bSdk`]/
//!   [`E2bSandbox`] traits; no real HTTP backend exists yet.
//! - `SandboxNotFoundError` collapses into the [`E2bSdkErrorKind::NotFound`]
//!   kind; consumers match `not_found`.
//! - The API-key environment lookup is injectable (the process-global
//!   `E2B_API_KEY` default), keeping tests free of env races.
//! - Sandbox creation is lazy: the first `get_sandbox`/disposal starts it
//!   (the TS eager `ready` promise starts at construction; the observable
//!   contract — one shared handle, teardown kills it — is unchanged).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc, make_disposer};
use futures::future::BoxFuture;

/// Cordis plugin name (TS `name`).
pub const E2B_NAME: &str = "e2b";

/// Services required before the plugin can own a sandbox.
pub const E2B_INJECT: [&str; 0] = [];

/// Remote filesystem entry kinds (TS `FileType` subset used here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileType {
    File,
    Dir,
    #[default]
    Other,
}

/// One remote filesystem entry's facts (TS `EntryInfo` subset).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct E2bEntryInfo {
    pub name: String,
    pub path: String,
    pub file_type: FileType,
    pub size: u64,
    pub mode: u32,
    pub modified_time_ms: Option<i64>,
    /// Present when the entry is a symbolic link.
    pub symlink_target: Option<String>,
    /// Producer metadata (the `dsh-version` freshness token lives here).
    pub metadata: Option<HashMap<String, String>>,
}

/// One remote command outcome (TS `CommandResult` subset).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct E2bCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Command abort predicate (the TS `AbortSignal` collapse): polled by the
/// backend between chunks and before the call settles.
pub type E2bCommandAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// One remote command invocation's options (TS `commands.run`'s second
/// argument: envs/cwd/onStdout/onStderr/signal).
#[derive(Clone, Default)]
pub struct E2bCommandOptions {
    /// Environment overrides for the control shell.
    pub envs: Option<HashMap<String, String>>,
    /// Working directory for the control shell.
    pub cwd: Option<String>,
    /// Streaming stdout callback (chunks in delivery order; the final
    /// aggregate still lands in [`E2bCommandResult::stdout`]).
    pub on_stdout: Option<Arc<dyn Fn(&[u8]) + Send + Sync>>,
    /// Streaming stderr callback (same delivery contract).
    pub on_stderr: Option<Arc<dyn Fn(&[u8]) + Send + Sync>>,
    /// Abort predicate; a backend that observes it settles the call as
    /// aborted.
    pub signal: Option<E2bCommandAbort>,
}

impl E2bCommandOptions {
    pub fn with_envs(envs: HashMap<String, String>) -> Self {
        Self {
            envs: Some(envs),
            ..Default::default()
        }
    }
}

/// One background command invocation's options (TS `commands.run`'s
/// background-mode arguments: background/stdin/timeoutMs fixed by the mode,
/// so only the variable faces stay explicit).
#[derive(Clone, Default)]
pub struct E2bBackgroundOptions {
    /// Environment overrides for the control shell.
    pub envs: Option<HashMap<String, String>>,
    /// Working directory for the control shell.
    pub cwd: Option<String>,
    /// Whether the command's stdin stays open (TS `stdin: true`).
    pub stdin: bool,
    /// Streaming stdout callback (chunks in delivery order).
    pub on_stdout: Option<Arc<dyn Fn(&[u8]) + Send + Sync>>,
    /// Streaming stderr callback (same delivery contract).
    pub on_stderr: Option<Arc<dyn Fn(&[u8]) + Send + Sync>>,
    /// Abort predicate; a backend that observes it settles the call as
    /// aborted.
    pub signal: Option<E2bCommandAbort>,
}

/// One live background command (TS `CommandHandle` collapse): the process
/// identity, stdin, and settlement of a background-mode run.
#[async_trait::async_trait]
pub trait E2bCommandHandle: Send + Sync {
    /// The remote process id; a backend without one reports a sentinel the
    /// caller must validate.
    fn pid(&self) -> i32;
    /// Settle with the command's exit facts.
    async fn wait(&self) -> Result<E2bCommandResult, E2bSdkError>;
    /// Kill the command (best effort; already-exited is tolerated).
    async fn kill(&self) -> Result<(), E2bSdkError>;
    /// Write a batch to the command's open stdin.
    async fn send_stdin(&self, data: &[u8]) -> Result<(), E2bSdkError>;
    /// Close the command's stdin.
    async fn close_stdin(&self) -> Result<(), E2bSdkError>;
}

/// One remote stream's open handle (the TS `ReadableStream` collapse).
pub trait E2bReadStream: Send + Sync {
    /// Read the next chunk, or `None` at end of stream.
    fn read(&mut self) -> BoxFuture<'static, Result<Option<Vec<u8>>, E2bSdkError>>;
    /// Best-effort cancellation.
    fn cancel(&mut self) -> BoxFuture<'static, ()>;
}

/// The SDK error taxonomy used by the owner (the TS SDK error classes
/// collapse; `NotFound` carries the `SandboxNotFoundError` contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2bSdkErrorKind {
    /// The sandbox/entry was already gone (TS `SandboxNotFoundError` /
    /// `FileNotFoundError`).
    NotFound,
    /// A command exited non-zero (TS `CommandExitError`).
    CommandExit { exit_code: i32 },
    /// Any other SDK failure.
    Other,
}

/// One structured SDK failure.
#[derive(Debug, Clone, PartialEq)]
pub struct E2bSdkError {
    pub kind: E2bSdkErrorKind,
    pub message: String,
    /// Non-empty stderr for a command-exit failure.
    pub stderr: Option<String>,
}

impl E2bSdkError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: E2bSdkErrorKind::NotFound,
            message: message.into(),
            stderr: None,
        }
    }

    pub fn command_exit(exit_code: i32, stderr: impl Into<String>) -> Self {
        let stderr = stderr.into();
        Self {
            kind: E2bSdkErrorKind::CommandExit { exit_code },
            message: stderr.clone(),
            stderr: Some(stderr),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: E2bSdkErrorKind::Other,
            message: message.into(),
            stderr: None,
        }
    }

    pub fn is_not_found(&self) -> bool {
        self.kind == E2bSdkErrorKind::NotFound
    }

    pub fn is_abort(&self) -> bool {
        matches!(self.kind, E2bSdkErrorKind::Other) && self.message.eq_ignore_ascii_case("aborted")
    }
}

impl std::fmt::Display for E2bSdkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for E2bSdkError {}

/// The TS `SandboxNotFoundError` re-export (a `not_found` SDK error).
pub type SandboxNotFoundError = E2bSdkError;

/// One live remote sandbox (the TS `Sandbox` handle collapse).
#[async_trait::async_trait]
pub trait E2bSandbox: Send + Sync {
    fn sandbox_id(&self) -> &str;
    /// Create a directory; `false` means it already existed.
    async fn make_dir(&self, path: &str) -> Result<bool, E2bSdkError>;
    async fn get_info(&self, path: &str) -> Result<E2bEntryInfo, E2bSdkError>;
    /// Read the whole entry as bytes.
    async fn read_bytes(&self, path: &str) -> Result<Vec<u8>, E2bSdkError>;
    /// Open a streaming read (empty files collapse to an empty stream).
    async fn read_stream(&self, path: &str) -> Result<Box<dyn E2bReadStream>, E2bSdkError>;
    /// List direct children.
    async fn list(&self, path: &str) -> Result<Vec<E2bEntryInfo>, E2bSdkError>;
    /// Write a whole file with optional metadata.
    async fn write(
        &self,
        path: &str,
        content: &[u8],
        metadata: Option<HashMap<String, String>>,
    ) -> Result<(), E2bSdkError>;
    /// Rename/replace an entry; returns the new entry info.
    async fn rename(&self, from: &str, to: &str) -> Result<E2bEntryInfo, E2bSdkError>;
    async fn remove(&self, path: &str) -> Result<(), E2bSdkError>;
    async fn run(
        &self,
        command: &str,
        options: &E2bCommandOptions,
    ) -> Result<E2bCommandResult, E2bSdkError>;
    /// Start one command in background mode (TS `commands.run` with
    /// `background: true`): the returned handle owns the process lifetime
    /// and its open stdin.
    async fn run_background(
        &self,
        command: &str,
        options: &E2bBackgroundOptions,
    ) -> Result<Arc<dyn E2bCommandHandle>, E2bSdkError>;
    async fn kill(&self) -> Result<(), E2bSdkError>;
}

/// Options passed to the SDK's sandbox factory (TS `Sandbox.create`
/// arguments; `secure` and the kill-on-timeout lifecycle are fixed).
#[derive(Debug, Clone, PartialEq)]
pub struct E2bCreateOptions {
    pub api_key: String,
    pub timeout_ms: u64,
}

/// The SDK's sandbox factory (TS `Sandbox.create`).
pub trait E2bSdk: Send + Sync {
    fn create(
        &self,
        options: &E2bCreateOptions,
    ) -> BoxFuture<'static, Result<Arc<dyn E2bSandbox>, E2bSdkError>>;
}

/// Quote one opaque argument for the SDK's unavoidable `/bin/bash -l -c`
/// layer (TS `quoteE2BShellArg`).
pub fn quote_e2b_shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Isolate E2B's hard-coded login shell behind a fresh randomized home path
/// (TS `e2bControlEnvs`).
pub fn e2b_control_envs(overrides: HashMap<String, String>) -> HashMap<String, String> {
    let mut envs = overrides;
    envs.insert(
        "HOME".to_string(),
        format!("/.dsh-e2b-control-{}", uuid::Uuid::new_v4()),
    );
    envs
}

/// Configuration for the shared E2B sandbox owner.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// API key; omission reads `E2B_API_KEY`. It is never forwarded into
    /// the sandbox.
    pub api_key: Option<String>,
    /// Shared remote working directory, created before adapters receive the
    /// sandbox.
    pub cwd: Option<String>,
    /// E2B sandbox lifetime in milliseconds; expiry always deletes the
    /// sandbox.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    api_key: String,
    timeout_ms: u64,
}

/// Creates one lazily consumable E2B SDK handle and deletes the sandbox at
/// timeout or disposal (TS `E2BRuntime`).
pub struct E2bRuntime {
    cwd: String,
    runtime_root: String,
    config: ResolvedConfig,
    sdk: Arc<dyn E2bSdk>,
    ready: tokio::sync::OnceCell<Result<Arc<dyn E2bSandbox>, String>>,
    disposed: AtomicBool,
    self_arc: std::sync::OnceLock<Arc<Self>>,
}

impl Service for E2bRuntime {
    fn service_name(&self) -> &'static str {
        "e2b"
    }
}

impl E2bRuntime {
    /// Construct, validate, register as `ctx.e2b`, and attach the teardown
    /// effect (the TS constructor collapse).
    pub fn install(
        ctx: &Context,
        sdk: Arc<dyn E2bSdk>,
        config: Config,
        env_lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    ) -> Result<Arc<Self>, String> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| env_lookup("E2B_API_KEY"))
            .unwrap_or_default();
        let cwd = config
            .cwd
            .unwrap_or_else(|| "/home/user/workspace".to_string());
        let timeout_ms = config.timeout_ms.unwrap_or(300_000);
        if api_key.is_empty() {
            return Err("dsh-e2b: configure apiKey or set E2B_API_KEY".to_string());
        }
        if !cwd.starts_with('/') {
            return Err(format!(
                "dsh-e2b: cwd must be an absolute Linux path: {cwd}"
            ));
        }
        if timeout_ms == 0 {
            return Err("dsh-e2b: timeoutMs must be a positive finite number".to_string());
        }
        let runtime_root = format!("{cwd}/.dsh-e2b");
        let runtime = Arc::new(Self {
            cwd,
            runtime_root,
            config: ResolvedConfig {
                api_key,
                timeout_ms,
            },
            sdk,
            ready: tokio::sync::OnceCell::new(),
            disposed: AtomicBool::new(false),
            self_arc: std::sync::OnceLock::new(),
        });
        let _ = runtime.self_arc.set(runtime.clone());
        ctx.register_service(runtime.clone());

        // Disposal kills the shared sandbox; a not-found kill is the
        // already-gone case, every other failure is reported.
        let runtime_for_effect = runtime.clone();
        let ctx_for_effect = ctx.clone();
        let _ = ctx.effect(
            "e2b sandbox teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = runtime_for_effect.clone();
                    let ctx = ctx_for_effect.clone();
                    Box::pin(async move {
                        runtime.disposed.store(true, SeqCst);
                        let sandbox = match runtime.ensure_open().await {
                            Ok(sandbox) => sandbox,
                            Err(_) => return,
                        };
                        match sandbox.kill().await {
                            Ok(()) => {}
                            Err(error) if error.is_not_found() => {}
                            Err(error) => {
                                ctx.logger.error(&ctx, vec![arc(error.message.clone())]);
                            }
                        }
                    })
                }))
            }),
        );
        Ok(runtime)
    }

    /// Validated remote working directory shared by provider adapters.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Remote directory reserved for adapter-owned process and terminal
    /// state.
    pub fn runtime_root(&self) -> &str {
        &self.runtime_root
    }

    /// Start the shared creation once and cache its result (the TS eager
    /// `ready` promise; `get_or_init` serializes concurrent first callers).
    async fn ensure_open(&self) -> Result<Arc<dyn E2bSandbox>, String> {
        self.ready
            .get_or_init(|| {
                let runtime = self.self_arc.get().expect("installed").clone();
                async move {
                    let create = runtime.sdk.create(&E2bCreateOptions {
                        api_key: runtime.config.api_key.clone(),
                        timeout_ms: runtime.config.timeout_ms,
                    });
                    runtime
                        .finish_open(create.await)
                        .await
                        .map_err(|error| error.message)
                }
            })
            .await
            .clone()
    }

    /// The creation-window setup (TS `open`).
    async fn finish_open(
        &self,
        created: Result<Arc<dyn E2bSandbox>, E2bSdkError>,
    ) -> Result<Arc<dyn E2bSandbox>, E2bSdkError> {
        let sandbox = created?;
        let setup = async {
            sandbox.make_dir(&self.cwd).await?;
            sandbox.make_dir(&self.runtime_root).await?;
            let info = sandbox.get_info(&self.runtime_root).await?;
            if info.file_type != FileType::Dir || info.symlink_target.is_some() {
                return Err(E2bSdkError::other(format!(
                    "dsh-e2b: runtime root must be a real directory: {}",
                    self.runtime_root
                )));
            }
            sandbox
                .run(
                    &format!("chmod 700 -- {}", quote_e2b_shell_arg(&self.runtime_root)),
                    &E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
                )
                .await?;
            Ok(())
        }
        .await;
        match setup {
            Ok(()) => Ok(sandbox),
            Err(error) => {
                // One rollback attempt; the original failure is preserved.
                let _ = sandbox.kill().await;
                Err(error)
            }
        }
    }

    /// Return the shared live SDK handle (TS `getSandbox`).
    pub async fn get_sandbox(&self) -> Result<Arc<dyn E2bSandbox>, String> {
        if self.disposed.load(SeqCst) {
            return Err("E2B sandbox service is disposing".to_string());
        }
        let sandbox = self.ensure_open().await?;
        if self.disposed.load(SeqCst) {
            return Err("E2B sandbox service is disposing".to_string());
        }
        Ok(sandbox)
    }
}

/// The Cordis plugin form (TS mounts the module with its schema).
pub struct E2bPlugin {
    sdk: Arc<dyn E2bSdk>,
    env_lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl E2bPlugin {
    pub fn new(
        sdk: Arc<dyn E2bSdk>,
        env_lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    ) -> Self {
        Self { sdk, env_lookup }
    }
}

fn config_from_value(config: &ArcValue) -> Result<Config, String> {
    let Some(value) = cordis::downcast::<serde_json::Value>(config) else {
        return Ok(Config::default());
    };
    let api_key = value
        .get("apiKey")
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "dsh-e2b: apiKey must be a string".to_string())
        })
        .transpose()?;
    let cwd = value
        .get("cwd")
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "dsh-e2b: cwd must be a string".to_string())
        })
        .transpose()?;
    let timeout_ms = value
        .get("timeoutMs")
        .map(|v| {
            if let Some(integer) = v.as_u64() {
                return Ok(Some(integer));
            }
            let number = v
                .as_f64()
                .ok_or_else(|| "dsh-e2b: timeoutMs must be a number".to_string())?;
            if !number.is_finite()
                || number.fract() != 0.0
                || number <= 0.0
                || number > 9_007_199_254_740_991.0
            {
                return Err("dsh-e2b: timeoutMs must be a positive finite number".to_string());
            }
            Ok(Some(number as u64))
        })
        .transpose()?
        .flatten();
    Ok(Config {
        api_key,
        cwd,
        timeout_ms,
    })
}

#[async_trait::async_trait]
impl Plugin for E2bPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(E2B_NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(E2B_INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config_from_value(&config)
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        E2bRuntime::install(ctx, self.sdk.clone(), config, self.env_lookup.clone())
            .map(|_| ())
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
