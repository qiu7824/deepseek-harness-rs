//! File-backed loader subtree (port of the `Include` class).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, Context, EventOptions, Plugin, PluginError, arc};
use dsh_cordis_loader::{EntryOptions, EntryTree, LoaderError, LoaderService};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::AbortHandle;

use crate::patch::{PatchOptions, apply_entry_patches};
use crate::yaml::{json_to_yaml, parse_yaml};

const WRITE_RETRY_LIMIT: u32 = 10;
const WRITE_RETRY_DELAY_MS: u64 = 50;

/// Config for a file-backed loader subtree (TS `Include.Config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncludeConfig {
    /// YAML or JSON path resolved from `ctx.baseUrl`.
    pub path: String,
    /// Entry list written when the file does not already exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Vec<EntryOptions>>,
    /// Runtime patches applied after reading the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patches: Option<Vec<PatchOptions>>,
    /// Enables loader apply/reload/unload logs for this subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_logs: Option<bool>,
}

/// Supported config file types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Yaml,
    Json,
}

fn file_type(filename: &str) -> Result<FileType, String> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    match ext {
        "yaml" | "yml" => Ok(FileType::Yaml),
        "json" => Ok(FileType::Json),
        other => Err(format!("extension \"{other}\" not supported")),
    }
}

/// Failure reading/parsing/validating the config file (TS `ConfigFileError`).
#[derive(Debug, Clone)]
pub enum ConfigFileError {
    Read { path: String, message: String, not_found: bool },
    Parse { path: String, message: String },
    Validate { path: String, message: String },
}

impl std::fmt::Display for ConfigFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigFileError::Read { path, message, .. } => {
                write!(f, "failed to read config file {path}: {message}")
            }
            ConfigFileError::Parse { path, message } => {
                write!(f, "failed to parse config file {path}: {message}")
            }
            ConfigFileError::Validate { path, message } => {
                write!(f, "failed to validate config file {path}: {message}")
            }
        }
    }
}

impl std::error::Error for ConfigFileError {}

/// Runtime error surfaced by include operations.
#[derive(Debug)]
pub enum IncludeError {
    File(ConfigFileError),
    Loader(LoaderError),
    Message(String),
}

impl std::fmt::Display for IncludeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IncludeError::File(error) => write!(f, "{error}"),
            IncludeError::Loader(error) => write!(f, "{error}"),
            IncludeError::Message(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for IncludeError {}

impl From<ConfigFileError> for IncludeError {
    fn from(error: ConfigFileError) -> Self {
        IncludeError::File(error)
    }
}

impl From<LoaderError> for IncludeError {
    fn from(error: LoaderError) -> Self {
        IncludeError::Loader(error)
    }
}

pub(crate) struct ReadCandidate {
    pub(crate) content: String,
    pub(crate) data: Vec<Value>,
}
/// Loader entry tree backed by a YAML or JSON file.
pub struct Include {
    pub tree: Arc<EntryTree>,
    pub filename: String,
    pub file_type: FileType,
    pub readonly: AtomicBool,
    pub config: Mutex<IncludeConfig>,
    content: Mutex<Option<String>>,
    data: Mutex<Vec<Value>>,
    write_task: Mutex<Option<AbortHandle>>,
    /// Serializes applies (TS `applyQueue`).
    apply_lock: tokio::sync::Mutex<()>,
    /// Serializes file writes (TS `writeQueue`).
    write_lock: tokio::sync::Mutex<()>,
}

impl Include {
    /// Create the file-backed subtree for the include plugin entry.
    ///
    /// The nested tree is created from the include fiber's context; the
    /// loader core associates it with the owning entry automatically.
    pub fn new(
        ctx: Context,
        core: Arc<dsh_cordis_loader::LoaderCore>,
        config: IncludeConfig,
    ) -> Result<Arc<Self>, IncludeError> {
        let tree = EntryTree::new(ctx.clone(), core);
        if let Some(enable_logs) = config.enable_logs {
            // EntryTree.enableLogs is exposed via the loader service; keep
            // the local flag for future log filtering.
            let _ = enable_logs;
        }
        let base = ctx.base_url().unwrap_or_default();
        let filename = resolve_filename(&base, &config.path);
        let file_type = file_type(&filename).map_err(IncludeError::Message)?;
        if let Some(parent) = Path::new(&filename).parent() {
            ctx.set_base_url(Some(parent.to_string_lossy().to_string() + "/"));
        }
        let include = Arc::new(Self {
            tree,
            filename,
            file_type,
            readonly: AtomicBool::new(false),
            config: Mutex::new(config),
            content: Mutex::new(None),
            data: Mutex::new(Vec::new()),
            write_task: Mutex::new(None),
            apply_lock: tokio::sync::Mutex::new(()),
            write_lock: tokio::sync::Mutex::new(()),
        });

        // Write-backs from the loader funnel into this subtree's debounced
        // flush (TS `Include.write()` override).
        {
            let include_for_write = include.clone();
            include.tree.set_write_backend(Arc::new(move || {
                let include = include_for_write.clone();
                include.tree.ctx.emit("loader/config-update", vec![]);
                if let Some(task) = include.write_task.lock().take() {
                    task.abort();
                }
                let flush = include.clone();
                let handle = tokio::spawn(async move {
                    if let Err(error) = flush.flush_write().await {
                        tracing::warn!(
                            "failed to write config file {}: {error}",
                            flush.filename
                        );
                    }
                });
                *include.write_task.lock() = Some(handle.abort_handle());
            }));
        }

        // Store the owner handle on the tree so hosts (and HMR) can reach
        // `refresh()`. Double-wrapped: `Some(include.clone())` would unsize
        // the Arc itself and store a bare `Include`.
        *include.tree.extras.lock() = Some(Arc::new(include.clone()));

        Ok(include)
    }

    /// Serialize one child-tree mutation behind every earlier one (TS
    /// `enqueue`; the transactional group update is not reentrant).
    async fn enqueue<T>(&self, task: impl std::future::Future<Output = T>) -> T {
        let _guard = self.apply_lock.lock().await;
        task.await
    }

    /// Read the file, parsing and validating the top-level entry list.
    async fn read(&self, forced: bool) -> Result<Option<ReadCandidate>, IncludeError> {
        let content = tokio::fs::read_to_string(&self.filename)
            .await
            .map_err(|error| ConfigFileError::Read {
                path: self.filename.clone(),
                message: error.to_string(),
                not_found: error.kind() == std::io::ErrorKind::NotFound,
            })?;
        if !forced && self.content.lock().as_deref() == Some(content.as_str()) {
            return Ok(None);
        }
        let data = match self.file_type {
            FileType::Yaml => parse_yaml(&content).map_err(|message| {
                ConfigFileError::Parse { path: self.filename.clone(), message }
            })?,
            FileType::Json => serde_json::from_str::<Value>(&content).map_err(|error| {
                ConfigFileError::Parse {
                    path: self.filename.clone(),
                    message: error.to_string(),
                }
            })?,
        };
        let Value::Array(list) = data else {
            return Err(ConfigFileError::Validate {
                path: self.filename.clone(),
                message: "config file must be a top-level array".to_string(),
            }
            .into());
        };
        Ok(Some(ReadCandidate { content, data: list }))
    }

    fn warn(&self, message: &str) {
        tracing::warn!("include: {message}");
    }

    /// Apply patches and commit the entry list to the tree.
    async fn apply_candidate(&self, candidate: ReadCandidate) -> Result<(), IncludeError> {
        let config = self.config.lock().clone();
        let patched = apply_entry_patches(
            &candidate.data,
            config.patches.as_deref(),
            &mut |message| self.warn(message),
        );
        let entries: Vec<EntryOptions> = patched
            .iter()
            .map(|value| {
                serde_json::from_value::<EntryOptions>(value.clone()).map_err(|error| {
                    IncludeError::Message(format!("invalid loader entry: {error}"))
                })
            })
            .collect::<Result<_, _>>()?;
        self.tree.root_group().update(entries).await?;
        *self.content.lock() = Some(candidate.content);
        *self.data.lock() = candidate.data;
        self.check_access().await;
        Ok(())
    }

    async fn check_access(&self) {
        // Writable extensions only; mark readonly when the file cannot be
        // opened for writing (TS `checkAccess`).
        if tokio::fs::OpenOptions::new()
            .write(true)
            .open(&self.filename)
            .await
            .is_err()
        {
            self.readonly.store(true, Ordering::SeqCst);
        }
    }

    /// Initialize: read (creating the file from `initial` when absent) and
    /// apply. TS `Service.init`.
    pub async fn init(self: &Arc<Self>) -> Result<(), IncludeError> {
        let candidate = match self.read(true).await {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return Ok(()),
            Err(IncludeError::File(ConfigFileError::Read { not_found: true, .. })) => {
                let config = self.config.lock().clone();
                let Some(initial) = &config.initial else {
                    return Err(IncludeError::Message(format!(
                        "config file not found: {}",
                        self.filename
                    )));
                };
                self.write_file_now(initial.clone()).await?;
                self.read(true).await?.ok_or_else(|| {
                    IncludeError::Message(format!("config file not found: {}", self.filename))
                })?
            }
            Err(error) => return Err(error),
        };
        self.apply(candidate).await
    }

    /// Apply a candidate through the serialized apply queue (TS `apply`).
    pub(crate) async fn apply(self: &Arc<Self>, candidate: ReadCandidate) -> Result<(), IncludeError> {
        let this = self.clone();
        self.enqueue(async move { this.apply_candidate(candidate).await }).await
    }

    /// Re-read the file and transactionally refresh child entries when the
    /// content changed (TS `refresh`).
    pub async fn refresh(self: &Arc<Self>) -> Result<(), IncludeError> {
        let this = self.clone();
        this.enqueue(async {
            let candidate = this.read(false).await?;
            let Some(candidate) = candidate else { return Ok(()) };
            this.apply_candidate(candidate).await
        })
        .await
    }

    /// Stop the subtree: stop entries and flush pending writes (TS `stop`).
    pub async fn stop(self: &Arc<Self>) -> Result<(), IncludeError> {
        self.tree.root_group().stop().await?;
        self.flush_write().await?;
        Ok(())
    }

    // ---- write path ----

    /// Serialize the current root data to the file (debounced via the write
    /// backend; coalesced to the latest tree state, TS `flushWrite`).
    pub async fn flush_write(&self) -> Result<(), IncludeError> {
        let _guard = self.write_lock.lock().await;
        let config = self.tree.root_group().data.lock().clone();
        self.write_file_now(config).await
    }

    async fn write_file_now(&self, config: Vec<EntryOptions>) -> Result<(), IncludeError> {
        if self.readonly.load(Ordering::SeqCst) {
            return Err(IncludeError::Message("cannot overwrite readonly config".to_string()));
        }
        let value = serde_json::to_value(&config).map_err(|error| {
            IncludeError::Message(format!("cannot serialize entry list: {error}"))
        })?;
        let content = match self.file_type {
            FileType::Yaml => {
                serde_yaml::to_string(&json_to_yaml(&value)).map_err(|error| {
                    IncludeError::Message(format!("cannot dump YAML: {error}"))
                })?
            }
            FileType::Json => serde_json::to_string_pretty(&value).map_err(|error| {
                IncludeError::Message(format!("cannot dump JSON: {error}"))
            })?,
        };
        let tmp = format!("{}.tmp", self.filename);
        tokio::fs::write(&tmp, &content).await.map_err(|error| {
            IncludeError::Message(format!("cannot write {tmp}: {error}"))
        })?;
        for retry in 0..=WRITE_RETRY_LIMIT {
            match tokio::fs::rename(&tmp, &self.filename).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let retryable = matches!(
                        error.raw_os_error(),
                        Some(code)
                            if code == 5 /* EACCES on Windows */
                                || code == 13 /* EACCES */
                                || code == 32 /* EBUSY/EPERM on Windows */
                                || code == 1 /* EPERM */
                    );
                    if !retryable || retry >= WRITE_RETRY_LIMIT {
                        return Err(IncludeError::Message(format!(
                            "cannot rename {tmp} to {}: {error}",
                            self.filename
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(
                        (retry as u64 + 1) * WRITE_RETRY_DELAY_MS,
                    ))
                    .await;
                }
            }
        }
        unreachable!("retry loop returns")
    }
}

/// Resolve a config path against a base URL (TS `new URL(path, baseUrl)`).
fn resolve_filename(base: &str, path: &str) -> String {
    let base_path = Path::new(base);
    let combined = base_path.join(path);
    combined.to_string_lossy().to_string()
}

/// The include plugin entrypoint (`export default Include` in TS).
pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(IncludePlugin)
}

pub struct IncludePlugin;

#[async_trait::async_trait]
impl Plugin for IncludePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("include")
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["loader"])
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let loader = ctx
            .get_typed::<Arc<LoaderService>>("loader", true)
            .ok_or_else(|| PluginError::new(arc("loader service is not available".to_string())))?
            .as_ref()
            .clone();
        let core = loader.core.clone();
        // Tree-carrier marker: the include config holds entry/patch lists
        // whose `!!js` expressions belong to other fibers (TS
        // `EntryGroup.key`).
        core.mark_carrier(&ctx.fiber);
        let config_value = cordis::downcast::<Value>(&config)
            .cloned()
            .unwrap_or(Value::Null);
        let config: IncludeConfig = serde_json::from_value(config_value).map_err(|error| {
            PluginError::new(arc(format!("invalid include config: {error}")))
        })?;
        let include = Include::new(ctx.clone(), core.clone(), config.clone()).map_err(|error| {
            PluginError::new(arc(error.to_string()))
        })?;

        // internal/update: re-apply when the include config changes
        // (TS ctor listener, index.ts:206-213 — a path change passes
        // through untouched; a patches-only change re-applies over the
        // settled data and consumes the event).
        let include_for_listener = include.clone();
        ctx.on(
            "internal/update",
            Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                let include = include_for_listener.clone();
                Box::pin(async move {
                    let next = args
                        .last()
                        .and_then(|value| cordis::downcast::<cordis::NextFn>(value));
                    let pass_through = || async {
                        match next {
                            Some(next) => Some(next.call().await),
                            None => None,
                        }
                    };
                    let Some(raw) = args
                        .first()
                        .and_then(|value| cordis::downcast::<Value>(value))
                    else {
                        return pass_through().await;
                    };
                    let Ok(new_config) = serde_json::from_value::<IncludeConfig>(raw.clone())
                    else {
                        return pass_through().await;
                    };
                    let old_path = include.config.lock().path.clone();
                    if new_config.path != old_path {
                        return pass_through().await;
                    }
                    match include.enqueue(include.apply_patches_with_config(new_config)).await {
                        // TS consumes the event without calling next.
                        Ok(()) => None,
                        Err(error) => Some(arc(PluginError::new(arc(error.to_string())))),
                    }
                })
            }),
            EventOptions::default(),
        )
        .await;

        // TS `Service.init` registers the teardown before applying.
        let stop_include = include.clone();
        ctx.effect(
            "include stop",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let include = stop_include.clone();
                    Box::pin(async move {
                        if let Err(error) = include.stop().await {
                            tracing::warn!("include stop failed: {error}");
                        }
                    })
                }))
            }),
        );

        include.init().await.map_err(|error| PluginError::new(arc(error.to_string())))?;
        Ok(())
    }
}

impl Include {
    /// Re-apply patches over the settled entry data without re-reading the
    /// file (TS include internal/update path-equal branch).
    async fn apply_patches_with_config(
        self: &Arc<Self>,
        new_config: IncludeConfig,
    ) -> Result<(), IncludeError> {
        let data = self.data.lock().clone();
        let patched = apply_entry_patches(
            &data,
            new_config.patches.as_deref(),
            &mut |message| self.warn(message),
        );
        let entries: Vec<EntryOptions> = patched
            .iter()
            .map(|value| {
                serde_json::from_value::<EntryOptions>(value.clone()).map_err(|error| {
                    IncludeError::Message(format!("invalid loader entry: {error}"))
                })
            })
            .collect::<Result<_, _>>()?;
        self.tree.root_group().update(entries).await?;
        *self.config.lock() = new_config;
        Ok(())
    }
}
