//! Bounded local evidence ledger. Raw tool output and arguments are never persisted.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 1000;
const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
const RECOVERY_WINDOW: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Default)]
pub struct FailureObservation {
    pub workspace_key: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub tool: String,
    /// "tool", "provider", or "feedback"; only trusted structured codes choose rules.
    pub source: String,
    pub code: String,
    /// Accepted for event adapters, deliberately never copied into the ledger.
    pub message: String,
    pub call_id: String,
    pub argument_fingerprint: Option<String>,
    pub resource_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RecoveryObservation {
    pub workspace_key: String,
    pub session_id: String,
    pub tool: String,
    pub call_id: String,
    pub argument_fingerprint: String,
    pub resource_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOccurrence {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub provider_key: String,
    #[serde(default)]
    pub model_key: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningEntry {
    pub id: String,
    pub workspace_key: String,
    pub tool: String,
    pub source: String,
    pub category: String,
    pub code: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub provider_key: String,
    #[serde(default)]
    pub model_key: String,
    #[serde(default)]
    pub route_known: bool,
    pub models: Vec<ModelOccurrence>,
    pub occurrences: u64,
    pub first_seen: u64,
    pub last_seen: u64,
    pub last_recovered: Option<u64>,
    pub status: String,
    pub verification: Option<String>,
    pub rule_id: String,
    pub suggestion: String,
    pub message: String,
    pub enabled: bool,
    pub revision: u64,
    #[serde(default)]
    pub application_count: u64,
    #[serde(default)]
    pub last_applied: Option<u64>,
    #[serde(default)]
    pub last_application_outcome: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Document {
    version: u32,
    enabled: bool,
    revision: u64,
    #[serde(default, rename = "configRevision")]
    config_revision: u64,
    entries: Vec<LearningEntry>,
    #[serde(default)]
    seen_events: Vec<String>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            revision: 0,
            config_revision: 0,
            entries: vec![],
            seen_events: vec![],
        }
    }
}

#[derive(Clone)]
struct Pending {
    id: String,
    revision: u64,
    session: String,
    workspace: String,
    tool: String,
    at: u64,
    call: String,
    arguments: Option<String>,
    resource: Option<String>,
}

pub struct LearningStore {
    root: PathBuf,
    document: RwLock<Document>,
    mutation: tokio::sync::Mutex<()>,
    pending: Mutex<Vec<Pending>>,
    last_error: Mutex<Option<String>>,
    queue: Mutex<Option<tokio::sync::mpsc::Sender<Queued>>>,
    generation: AtomicU64,
    policy_enabled: AtomicBool,
    read_only_error: Option<String>,
}
enum Queued {
    Failure(u64, FailureObservation),
    Recovery(u64, RecoveryObservation),
    Flush(tokio::sync::oneshot::Sender<()>),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}
impl cordis::Service for LearningStore {
    fn service_name(&self) -> &'static str {
        "learningStore"
    }
}

pub fn workspace_key(cwd: &str) -> String {
    let mut cwd = cwd.trim().replace('\\', "/");
    if let Some(unc) = cwd
        .strip_prefix("//?/UNC/")
        .or_else(|| cwd.strip_prefix("//?/unc/"))
    {
        cwd = format!("//{unc}");
    } else if let Some(path) = cwd.strip_prefix("//?/") {
        cwd = path.to_string();
    }
    let normalized = if cfg!(windows) {
        cwd.to_lowercase()
    } else {
        cwd
    };
    digest(normalized.trim_end_matches('/').as_bytes())
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn identifier(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if value.trim().is_empty()
        || value.chars().count() > 128
        || lower.contains("sk-")
        || lower.contains("bearer")
        || lower.contains("token=")
        || !value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | ' '))
    {
        return format!("unknown-{}", digest(value.as_bytes()));
    }
    value.to_string()
}

/// Rules are product-owned, not generated from an error's text or tool result.
pub fn rule(code: &str, source: &str) -> (&'static str, &'static str, &'static str, bool) {
    if source == "feedback" {
        return ("user-feedback", "feedback", "", false);
    }
    if source == "provider" {
        return match code {
            "RATE_LIMIT" | "429" => (
                "provider-rate-limit",
                "rate-limit",
                "服务限流时遵守 Retry-After，采用有界退避；继续失败时停止并报告当前状态。",
                false,
            ),
            "AUTH" | "INVALID_CREDENTIAL" | "401" | "403" => (
                "provider-auth",
                "authentication",
                "先检查当前供应商的账号状态与模型访问权限；凭据只通过账号管理界面更新，不写入提示词或日志。",
                false,
            ),
            _ => ("provider-error", "provider", "", false),
        };
    }
    match code {
        "TOOL_INPUT_INVALID" | "INVALID_ARGUMENTS" | "INVALID_INPUT" | "VALIDATION_ERROR" => (
            "tool-input-schema",
            "arguments",
            "调用工具前按当前工具 schema 核对必填参数、参数名与类型；修正后再执行，不能猜测旧版本参数。",
            true,
        ),
        "FS_NOT_OBSERVED" => (
            "observe-before-write",
            "filesystem-observation",
            "修改文件前先读取或检查当前目标；更换路径或文件变化后重新观察，并按当前权限策略执行。",
            true,
        ),
        "UNKNOWN_TOOL" => (
            "tool-availability",
            "tool-availability",
            "执行前核对当前可用工具列表及调用名称；不存在的工具不能仅凭以前使用过就继续调用。",
            false,
        ),
        "RUNTIME_UNAVAILABLE" | "ENOENT" => (
            "runtime-unavailable",
            "runtime",
            "先查看当前工具运行环境的实际路径、版本与可用性；依赖未就绪时停止相关执行并报告。",
            false,
        ),
        "TOOL_PREFLIGHT_DENIED" => (
            "tool-preflight-denied",
            "preflight",
            "执行前核对当前工具前置条件和权限；满足条件或获得授权后再执行，不绕过现行限制。",
            false,
        ),
        _ => ("unclassified-tool-failure", "tool-error", "", false),
    }
}

impl LearningStore {
    pub async fn open(root: PathBuf) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|_| "无法创建经验存储目录".to_string())?;
        let path = root.join("learning.json");
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_DOCUMENT_BYTES as u64
            {
                return Err("经验账本不是有效的受限常规文件".into());
            }
        }
        let document = match tokio::fs::File::open(&path).await {
            Ok(file) => {
                use tokio::io::AsyncReadExt;
                let mut bytes = Vec::new();
                file.take(MAX_DOCUMENT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| "无法读取经验账本".to_string())?;
                if bytes.len() > MAX_DOCUMENT_BYTES {
                    return Err("经验账本超过读取预算".into());
                }
                let document: Document = serde_json::from_slice(&bytes)
                    .map_err(|_| "经验账本损坏，未覆盖原文件".to_string())?;
                if document.version != 1 || document.entries.len() > MAX_ENTRIES {
                    return Err("经验账本版本或数量不受支持".into());
                }
                document
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Document::default(),
            Err(_) => return Err("无法读取经验账本".into()),
        };
        Ok(Self {
            root,
            document: RwLock::new(document),
            mutation: tokio::sync::Mutex::new(()),
            pending: Mutex::new(vec![]),
            last_error: Mutex::new(None),
            queue: Mutex::new(None),
            generation: AtomicU64::new(0),
            policy_enabled: AtomicBool::new(true),
            read_only_error: None,
        })
    }

    /// Optional learning must not prevent ordinary sessions from starting when
    /// its sidecar is damaged. Keep the original file untouched and report it.
    pub async fn open_or_disabled(root: PathBuf) -> Self {
        match Self::open(root.clone()).await {
            Ok(store) => store,
            Err(error) => Self {
                root,
                document: RwLock::new(Document {
                    enabled: false,
                    ..Document::default()
                }),
                mutation: tokio::sync::Mutex::new(()),
                pending: Mutex::new(vec![]),
                last_error: Mutex::new(Some(error.clone())),
                queue: Mutex::new(None),
                generation: AtomicU64::new(0),
                policy_enabled: AtomicBool::new(true),
                read_only_error: Some(error),
            },
        }
    }

    pub fn start_worker(self: &Arc<Self>) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        if queue.is_some() {
            return Ok(());
        }
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Queued>(256);
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("dsh-learning-ledger".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        if let Some(store) = weak.upgrade() {
                            *store.last_error.lock().unwrap() = Some("经验记录线程无法启动".into());
                        }
                        return;
                    }
                };
                runtime.block_on(async move {
                    while let Some(job) = receiver.recv().await {
                        let Some(store) = weak.upgrade() else {
                            break;
                        };
                        match job {
                            Queued::Failure(epoch, observation)
                                if epoch == store.generation.load(Ordering::Acquire) =>
                            {
                                let _ = store.record_failure(observation).await;
                            }
                            Queued::Recovery(epoch, observation)
                                if epoch == store.generation.load(Ordering::Acquire) =>
                            {
                                let _ = store.record_recovery(observation).await;
                            }
                            Queued::Flush(reply) => {
                                let _ = reply.send(());
                            }
                            Queued::Shutdown(reply) => {
                                let _ = reply.send(());
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            })
            .map_err(|_| "无法创建经验记录线程".to_string())?;
        *queue = Some(sender);
        Ok(())
    }

    fn enqueue(&self, job: Queued) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        let result = self
            .queue
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| "经验记录队列未启动".to_string())
            .and_then(|queue| {
                queue
                    .try_send(job)
                    .map_err(|_| "经验记录队列已满或已停止；本条未记录".to_string())
            });
        self.remember_error(&result);
        result
    }
    pub fn enqueue_failure(&self, mut observation: FailureObservation) -> Result<(), String> {
        observation.message.clear();
        observation.provider = identifier(&observation.provider);
        observation.model = identifier(&observation.model);
        if !observation.tool.is_empty() {
            observation.tool = identifier(&observation.tool);
        }
        observation.code = identifier(&observation.code);
        observation
            .session_id
            .truncate(observation.session_id.floor_char_boundary(256));
        observation
            .call_id
            .truncate(observation.call_id.floor_char_boundary(256));
        self.enqueue(Queued::Failure(
            self.generation.load(Ordering::Acquire),
            observation,
        ))
    }
    pub fn enqueue_recovery(&self, observation: RecoveryObservation) -> Result<(), String> {
        self.enqueue(Queued::Recovery(
            self.generation.load(Ordering::Acquire),
            observation,
        ))
    }
    pub async fn flush_pending(&self) -> Result<(), String> {
        let sender = self.queue.lock().unwrap().clone();
        let Some(sender) = sender else {
            return Ok(());
        };
        let (reply, received) = tokio::sync::oneshot::channel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            sender
                .send(Queued::Flush(reply))
                .await
                .map_err(|_| "经验记录队列已停止".to_string())?;
            received
                .await
                .map_err(|_| "经验记录队列未完成保存".to_string())
        })
        .await
        .map_err(|_| "经验记录刷新超时".to_string())
        .and_then(|result| result);
        self.remember_error(&result);
        result
    }
    pub async fn shutdown(&self) {
        let sender = self.queue.lock().unwrap().take();
        let Some(sender) = sender else {
            return;
        };
        let (reply, received) = tokio::sync::oneshot::channel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            sender
                .send(Queued::Shutdown(reply))
                .await
                .map_err(|_| "经验队列已停止".to_string())?;
            received
                .await
                .map_err(|_| "经验队列关闭前未完成保存".to_string())
        })
        .await
        .map_err(|_| "经验队列关闭超时".to_string())
        .and_then(|result| result);
        self.remember_error(&result);
    }

    pub fn enabled(&self) -> bool {
        self.policy_enabled.load(Ordering::Acquire) && self.document.read().unwrap().enabled
    }
    pub fn set_policy_enabled(&self, enabled: bool) {
        if self.policy_enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.pending.lock().unwrap().clear();
        }
    }
    fn remember_error<T>(&self, result: &Result<T, String>) {
        if let Err(error) = result {
            *self.last_error.lock().unwrap() = Some(error.clone());
        }
    }

    pub fn verified(
        &self,
        workspace: &str,
        tool: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
        limit: usize,
    ) -> Vec<LearningEntry> {
        let document = self.document.read().unwrap();
        if !document.enabled || !self.policy_enabled.load(Ordering::Acquire) {
            return vec![];
        }
        let provider_key = provider.map(|provider| digest(provider.as_bytes()));
        let model_key = model.map(|model| digest(model.as_bytes()));
        let mut entries: Vec<_> = document
            .entries
            .iter()
            .filter(|entry| {
                entry.enabled
                    && entry.status == "verified"
                    && !entry.suggestion.is_empty()
                    && entry.workspace_key == workspace
                    && tool.is_none_or(|name| entry.tool == name)
                    && (entry.source == "tool"
                        || entry.route_known
                            && provider_key.as_deref() == Some(entry.provider_key.as_str())
                            && model_key.as_deref() == Some(entry.model_key.as_str()))
            })
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse((entry.occurrences, entry.last_seen)));
        entries.into_iter().take(limit.min(20)).cloned().collect()
    }

    pub async fn record_failure(
        &self,
        observation: FailureObservation,
    ) -> Result<Option<LearningEntry>, String> {
        let result = self.record_failure_inner(observation).await;
        self.remember_error(&result);
        result
    }

    pub async fn record_recovery(
        &self,
        observation: RecoveryObservation,
    ) -> Result<Vec<String>, String> {
        let result = self.record_recovery_inner(observation).await;
        self.remember_error(&result);
        result
    }

    pub async fn mark_application(
        &self,
        id: &str,
        session: &str,
        call: &str,
        outcome: &str,
    ) -> Result<(), String> {
        let result = self
            .mark_application_inner(id, session, call, outcome)
            .await;
        self.remember_error(&result);
        result
    }

    async fn change<T>(
        &self,
        update: impl FnOnce(&mut Document) -> Result<T, String>,
    ) -> Result<T, String> {
        if let Some(error) = &self.read_only_error {
            return Err(error.clone());
        }
        let _serial = self.mutation.lock().await;
        let mut next = self.document.read().unwrap().clone();
        let result = update(&mut next)?;
        next.revision = next.revision.saturating_add(1);
        let bytes =
            serde_json::to_vec_pretty(&next).map_err(|_| "无法序列化经验账本".to_string())?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("经验账本已达到存储预算".into());
        }
        let path = self.root.join("learning.json");
        let failure = if std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
        {
            Some("经验账本目标不是常规文件".to_string())
        } else {
            dsh_atomic_write::write_file_atomic(
                &path,
                &bytes,
                dsh_atomic_write::WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
            .err()
            .map(|_| "经验账本保存失败；本次更改未生效".to_string())
        };
        if let Some(error) = failure {
            *self.last_error.lock().unwrap() = Some(error.clone());
            return Err(error);
        }
        *self.document.write().unwrap() = next;
        *self.last_error.lock().unwrap() = None;
        Ok(result)
    }

    async fn record_failure_inner(
        &self,
        observation: FailureObservation,
    ) -> Result<Option<LearningEntry>, String> {
        if !self.enabled() {
            return Ok(None);
        }
        let source = match observation.source.as_str() {
            "tool" | "provider" | "feedback" => observation.source.as_str(),
            _ => return Err("未知经验来源".into()),
        };
        let code = identifier(&observation.code).to_ascii_uppercase();
        if matches!(
            code.as_str(),
            "ABORTED"
                | "ABORTED_BEFORE_DISPATCH"
                | "CANCELLED"
                | "CANCELED"
                | "USER_APPROVAL_DENIED"
                | "USER_APPROVAL_CANCELLED"
        ) {
            return Ok(None);
        }
        if observation.workspace_key.len() != 64
            || !observation
                .workspace_key
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("经验记录缺少有效工作区".into());
        }
        let (rule_id, category, suggestion, can_recover) = rule(&code, source);
        let tool = if observation.tool.is_empty() {
            String::new()
        } else {
            identifier(&observation.tool)
        };
        let provider = identifier(&observation.provider);
        let model = identifier(&observation.model);
        let provider_key = digest(observation.provider.as_bytes());
        let model_key = digest(observation.model.as_bytes());
        let route_known = !provider.starts_with("unknown-") && !model.starts_with("unknown-");
        let at = now();
        let event_key = (!observation.call_id.is_empty()).then(|| {
            digest(
                format!(
                    "{}\0{}\0{}\0{}",
                    source, observation.session_id, observation.call_id, code
                )
                .as_bytes(),
            )
        });
        let saved = self
            .change(|document| {
                if !document.enabled
                    || !self.policy_enabled.load(Ordering::Acquire)
                    || event_key
                        .as_ref()
                        .is_some_and(|key| document.seen_events.contains(key))
                {
                    return Ok(None);
                }
                if let Some(key) = event_key {
                    document.seen_events.push(key);
                    if document.seen_events.len() > 2000 {
                        document.seen_events.remove(0);
                    }
                }
                let position = document.entries.iter().position(|entry| {
                    entry.workspace_key == observation.workspace_key
                        && entry.tool == tool
                        && entry.source == source
                        && entry.rule_id == rule_id
                        && entry.code == code
                        && (source == "tool"
                            || entry.provider_key == provider_key && entry.model_key == model_key)
                });
                let position = if let Some(position) = position {
                    position
                } else {
                    if document.entries.len() >= MAX_ENTRIES {
                        let oldest = document
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, entry)| entry.status == "pending")
                            .min_by_key(|(_, entry)| entry.last_seen)
                            .map(|(index, _)| index)
                            .ok_or("已验证经验达到数量预算，请先删除不再需要的条目")?;
                        document.entries.remove(oldest);
                    }
                    document.entries.push(LearningEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        workspace_key: observation.workspace_key.clone(),
                        tool: tool.clone(),
                        source: source.into(),
                        category: category.into(),
                        code: code.clone(),
                        provider: provider.clone(),
                        model: model.clone(),
                        provider_key: provider_key.clone(),
                        model_key: model_key.clone(),
                        route_known,
                        models: vec![],
                        occurrences: 0,
                        first_seen: at,
                        last_seen: at,
                        last_recovered: None,
                        status: "pending".into(),
                        verification: None,
                        rule_id: rule_id.into(),
                        suggestion: suggestion.into(),
                        message: diagnostic(rule_id).into(),
                        enabled: true,
                        revision: 0,
                        application_count: 0,
                        last_applied: None,
                        last_application_outcome: None,
                    });
                    document.entries.len() - 1
                };
                let entry = &mut document.entries[position];
                entry.occurrences = entry.occurrences.saturating_add(1);
                entry.last_seen = at;
                entry.revision = entry.revision.saturating_add(1);
                if let Some(seen) = entry
                    .models
                    .iter_mut()
                    .find(|item| item.provider_key == provider_key && item.model_key == model_key)
                {
                    seen.count = seen.count.saturating_add(1);
                } else if entry.models.len() < 16 {
                    entry.models.push(ModelOccurrence {
                        provider: provider.clone(),
                        model: model.clone(),
                        provider_key: provider_key.clone(),
                        model_key: model_key.clone(),
                        count: 1,
                    });
                }
                Ok(Some(entry.clone()))
            })
            .await?;
        if can_recover && let Some(entry) = &saved {
            let mut pending = self.pending.lock().unwrap();
            pending.retain(|item| {
                at.saturating_sub(item.at) <= RECOVERY_WINDOW
                    && !(item.session == observation.session_id
                        && item.tool == tool
                        && item.id == entry.id)
            });
            if pending.len() >= 256 {
                pending.remove(0);
            }
            pending.push(Pending {
                id: entry.id.clone(),
                revision: entry.revision,
                session: observation.session_id,
                workspace: observation.workspace_key,
                tool,
                at,
                call: observation.call_id,
                arguments: observation.argument_fingerprint,
                resource: observation.resource_fingerprint,
            });
        }
        Ok(saved)
    }

    async fn record_recovery_inner(
        &self,
        observation: RecoveryObservation,
    ) -> Result<Vec<String>, String> {
        if !self.enabled() {
            return Ok(vec![]);
        }
        let at = now();
        let pending: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|item| {
                item.session == observation.session_id
                    && item.workspace == observation.workspace_key
                    && item.tool == observation.tool
                    && item.call != observation.call_id
                    && at.saturating_sub(item.at) <= RECOVERY_WINDOW
                    && (item.arguments.as_deref()
                        == Some(observation.argument_fingerprint.as_str())
                        || item.resource.is_some()
                            && item.resource == observation.resource_fingerprint)
            })
            .cloned()
            .collect();
        if pending.is_empty() {
            return Ok(vec![]);
        }
        let recovered = self
            .change(|document| {
                if !document.enabled || !self.policy_enabled.load(Ordering::Acquire) {
                    return Ok(vec![]);
                }
                let mut recovered = vec![];
                for item in &pending {
                    let Some(entry) = document.entries.iter_mut().find(|entry| {
                        entry.id == item.id && entry.revision == item.revision && entry.enabled
                    }) else {
                        continue;
                    };
                    if !rule(&entry.code, &entry.source).3 {
                        continue;
                    }
                    entry.status = "verified".into();
                    if entry.verification.as_deref() != Some("user-confirmed") {
                        entry.verification = Some("recovered".into());
                    }
                    entry.last_recovered = Some(at);
                    entry.revision = entry.revision.saturating_add(1);
                    recovered.push(entry.id.clone());
                }
                Ok(recovered)
            })
            .await?;
        self.pending
            .lock()
            .unwrap()
            .retain(|item| !recovered.contains(&item.id));
        Ok(recovered)
    }

    pub fn list(&self, payload: &Value) -> Value {
        let document = self.document.read().unwrap();
        let query = payload
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase()
            .chars()
            .take(200)
            .collect::<String>();
        let mut rows: Vec<_> = document
            .entries
            .iter()
            .filter(|entry| {
                payload
                    .get("workspaceKey")
                    .and_then(Value::as_str)
                    .is_none_or(|workspace| entry.workspace_key == workspace)
                    && payload
                        .get("status")
                        .and_then(Value::as_str)
                        .is_none_or(|status| entry.status == status)
                    && (query.is_empty()
                        || format!(
                            "{} {} {} {} {}",
                            entry.tool, entry.category, entry.code, entry.suggestion, entry.model
                        )
                        .to_lowercase()
                        .contains(&query))
            })
            .collect();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.last_seen));
        let total = rows.len();
        let limit = payload
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .min(200) as usize;
        let memory_enabled = self.policy_enabled.load(Ordering::Acquire);
        json!({"enabled":document.enabled,"memoryEnabled":memory_enabled,"effectiveEnabled":document.enabled && memory_enabled,"revision":document.config_revision,"lastError":self.last_error.lock().unwrap().clone(),"total":total,"items":rows.into_iter().take(limit).collect::<Vec<_>>()})
    }

    pub async fn invoke(&self, method: &str, payload: Value) -> Result<Value, String> {
        self.flush_pending().await?;
        let result = self.invoke_inner(method, payload).await;
        self.remember_error(&result);
        result
    }

    async fn invoke_inner(&self, method: &str, payload: Value) -> Result<Value, String> {
        if method == "memory.learningList" {
            return Ok(self.list(&payload));
        }
        let expected = payload.get("expectedRevision").and_then(Value::as_u64);
        if method == "memory.learningConfigure" {
            let enabled = payload
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("enabled 必须为布尔值")?;
            self.change(|document| {
                check_revision(document.config_revision, expected)?;
                document.enabled = enabled;
                document.config_revision = document.config_revision.saturating_add(1);
                Ok(())
            })
            .await?;
            self.generation.fetch_add(1, Ordering::AcqRel);
            if !enabled {
                self.pending.lock().unwrap().clear();
            }
            return Ok(self.list(&json!({})));
        }
        let id = payload
            .get("id")
            .and_then(Value::as_str)
            .ok_or("缺少经验 id")?
            .to_string();
        let value = self
            .change(|document| {
                let index = document
                    .entries
                    .iter()
                    .position(|entry| entry.id == id)
                    .ok_or("经验不存在")?;
                check_revision(document.entries[index].revision, expected)?;
                if method == "memory.learningRemove" {
                    document.entries.remove(index);
                    return Ok(json!({"removed":true}));
                }
                let entry = &mut document.entries[index];
                match method {
                    "memory.learningToggle" => {
                        entry.enabled = payload
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .ok_or("enabled 必须为布尔值")?
                    }
                    "memory.learningConfirm" => {
                        if payload.get("confirmed") != Some(&Value::Bool(true)) {
                            return Err("必须明确确认经验修正建议".into());
                        }
                        if let Some(suggestion) = payload.get("suggestion").and_then(Value::as_str)
                        {
                            entry.suggestion = confirmed_suggestion(suggestion)?;
                        }
                        if entry.suggestion.is_empty() {
                            return Err("此失败没有已知修正方法，请填写经过确认的建议".into());
                        }
                        entry.status = "verified".into();
                        entry.verification = Some("user-confirmed".into());
                    }
                    _ => return Err("未知经验操作".into()),
                }
                entry.revision = entry.revision.saturating_add(1);
                Ok(json!({"entry":entry}))
            })
            .await?;
        self.pending.lock().unwrap().retain(|item| item.id != id);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(value)
    }

    async fn mark_application_inner(
        &self,
        id: &str,
        session: &str,
        call: &str,
        outcome: &str,
    ) -> Result<(), String> {
        if !matches!(outcome, "preflight_blocked" | "advisory") {
            return Err("未知经验复用结果".into());
        }
        let key = digest(format!("application\0{id}\0{session}\0{call}\0{outcome}").as_bytes());
        self.change(|document| {
            if !document.enabled
                || !self.policy_enabled.load(Ordering::Acquire)
                || document.seen_events.contains(&key)
            {
                return Ok(());
            }
            if let Some(entry) = document
                .entries
                .iter_mut()
                .find(|entry| entry.id == id && entry.enabled && entry.status == "verified")
            {
                entry.application_count = entry.application_count.saturating_add(1);
                entry.last_applied = Some(now());
                entry.last_application_outcome = Some(outcome.into());
                // Application telemetry does not invalidate an in-flight recovery or UI edit.
                document.seen_events.push(key);
                if document.seen_events.len() > 2000 {
                    document.seen_events.remove(0);
                }
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}

fn check_revision(actual: u64, expected: Option<u64>) -> Result<(), String> {
    if expected.is_some_and(|expected| expected != actual) {
        Err(format!("经验已更新，请重新读取（当前修订 {actual}）"))
    } else {
        Ok(())
    }
}
fn diagnostic(rule_id: &str) -> &'static str {
    match rule_id {
        "tool-input-schema" => "工具参数未通过当前 schema 校验。",
        "observe-before-write" => "目标尚未完成写入前所需的读取或观察。",
        "tool-availability" => "调用的工具当前不可用。",
        "runtime-unavailable" => "工具依赖或目标不可用；需要核查当前环境。",
        "tool-preflight-denied" => "当前工具前置条件或权限检查未通过。",
        "provider-rate-limit" => "供应商返回限流状态。",
        "provider-auth" => "供应商拒绝当前账号或模型访问。",
        "provider-error" => "模型请求失败；具体输出保留在原任务中。",
        "user-feedback" => "用户标记回复存在问题；尚未确认可复用的修正方法。",
        _ => "工具执行失败；未保存或提升工具返回的原始文本。",
    }
}
fn confirmed_suggestion(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 1000 {
        return Err("确认建议需为 1–1000 字".into());
    }
    static SECRET: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)bearer\s+|sk-[a-z0-9_-]{8,}|(?:api[_-]?key|access[_-]?token|refresh[_-]?token|password|secret)\s*[:=]").unwrap()
    });
    if SECRET.is_match(value) {
        return Err("建议中疑似包含凭据，请移除后再确认".into());
    }
    Ok(value
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect())
}

#[cfg(test)]
#[path = "learning_tests.rs"]
mod tests;
