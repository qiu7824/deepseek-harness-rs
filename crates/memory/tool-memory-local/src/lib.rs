use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod experience_reuse;
pub mod learning;
pub mod learning_runtime;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};
use serde::{Deserialize, Serialize};

pub const BUILTIN_CATEGORIES: &[(&str, &str)] = &[
    ("user-preference", "用户偏好"),
    ("tool-capability", "工具能力"),
    ("known-error", "已知错误"),
    ("project-knowledge", "项目知识"),
    ("operation-constraint", "操作约束"),
    ("custom", "自定义"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub title: String,
    pub content: String,
    pub enabled: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryDocument {
    version: u32,
    revision: u64,
    entries: Vec<MemoryEntry>,
}

impl Default for MemoryDocument {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            entries: Vec::new(),
        }
    }
}

pub struct MemoryStore {
    root: PathBuf,
    document: tokio::sync::Mutex<MemoryDocument>,
}

impl cordis::Service for MemoryStore {
    fn service_name(&self) -> &'static str {
        "memoryStore"
    }
}

impl MemoryStore {
    pub async fn open(root: PathBuf) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| format!("memory directory: {e}"))?;
        let path = root.join("entries.json");
        let document = match tokio::fs::read(&path).await {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("memory document: {e}"))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => MemoryDocument::default(),
            Err(error) => return Err(format!("memory document: {error}")),
        };
        Ok(Self {
            root,
            document: tokio::sync::Mutex::new(document),
        })
    }

    pub async fn list(&self, scope: Option<&str>, category: Option<&str>) -> Vec<MemoryEntry> {
        let document = self.document.lock().await;
        document
            .entries
            .iter()
            .filter(|entry| {
                scope.is_none_or(|value| entry.scope == value)
                    && category.is_none_or(|value| entry.category == value)
            })
            .cloned()
            .collect()
    }

    pub async fn upsert(
        &self,
        mut entry: MemoryEntry,
        expected_revision: Option<u64>,
    ) -> Result<MemoryEntry, String> {
        validate_entry(&entry)?;
        let mut document = self.document.lock().await;
        let mut next = document.clone();
        if let Some(position) = next
            .entries
            .iter()
            .position(|candidate| candidate.id == entry.id)
        {
            let actual = next.entries[position].revision;
            if expected_revision.is_some_and(|expected| expected != actual) {
                return Err(format!(
                    "memory entry changed since it was read (expected revision {}, now {actual})",
                    expected_revision.unwrap()
                ));
            }
            entry.revision = actual + 1;
            next.entries[position] = entry.clone();
        } else {
            if entry.id.trim().is_empty() {
                entry.id = uuid::Uuid::new_v4().to_string();
            }
            if expected_revision.is_some() {
                return Err("memory entry does not exist".to_string());
            }
            entry.revision = 1;
            next.entries.push(entry.clone());
        }
        next.revision += 1;
        persist_document(&self.root, &next).await?;
        *document = next;
        Ok(entry)
    }

    pub async fn remove(&self, id: &str, expected_revision: Option<u64>) -> Result<bool, String> {
        let mut document = self.document.lock().await;
        let Some(position) = document.entries.iter().position(|entry| entry.id == id) else {
            return Ok(false);
        };
        let actual = document.entries[position].revision;
        if expected_revision.is_some_and(|expected| expected != actual) {
            return Err(format!(
                "memory entry changed since it was read (expected revision {}, now {actual})",
                expected_revision.unwrap()
            ));
        }
        let mut next = document.clone();
        next.entries.remove(position);
        next.revision += 1;
        persist_document(&self.root, &next).await?;
        *document = next;
        Ok(true)
    }

    pub async fn render_enabled(&self, scope: &str, budget: usize) -> String {
        let document = self.document.lock().await;
        render_entries(&document, scope, budget)
    }
}

fn validate_entry(entry: &MemoryEntry) -> Result<(), String> {
    if entry.scope.trim().is_empty() {
        return Err("memory scope must be non-empty".to_string());
    }
    if !BUILTIN_CATEGORIES
        .iter()
        .any(|(id, _)| *id == entry.category)
    {
        return Err("unknown memory category".to_string());
    }
    if entry.title.trim().is_empty() {
        return Err("memory title must be non-empty".to_string());
    }
    if entry.content.trim().is_empty() {
        return Err("memory content must be non-empty".to_string());
    }
    if entry.title.chars().count() > 200 || entry.content.chars().count() > 20_000 {
        return Err(
            "memory title must be at most 200 characters and content at most 20000 characters"
                .into(),
        );
    }
    Ok(())
}

async fn persist_document(root: &Path, document: &MemoryDocument) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?;
    dsh_atomic_write::write_file_atomic(
        &root.join("entries.json"),
        &bytes,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// Render the current enabled entries for synchronous system-prompt providers.
/// A malformed or absent document fails closed to no injected memory.
pub fn render_enabled_file(root: &Path, scope: &str, budget: usize) -> String {
    let Ok(bytes) = std::fs::read(root.join("entries.json")) else {
        return String::new();
    };
    let Ok(document) = serde_json::from_slice::<MemoryDocument>(&bytes) else {
        return String::new();
    };
    let rendered = render_entries(&document, scope, budget);
    if rendered.is_empty() {
        String::new()
    } else {
        format!(
            "已保存的长期记忆：在执行相关操作前核对已知错误的触发条件，采用已验证的修正步骤，并执行其验证方法。记忆可能过时；以当前事实和用户指令为准。{rendered}"
        )
    }
}

fn render_entries(document: &MemoryDocument, scope: &str, budget: usize) -> String {
    let mut entries: Vec<_> = document
        .entries
        .iter()
        .filter(|entry| entry.enabled && (entry.scope == "default" || entry.scope == scope))
        .collect();
    // Lessons and constraints must survive a tight budget; local scope precedes
    // global notes and newer revisions break ties without changing stored order.
    entries.sort_by_key(|entry| {
        (
            match entry.category.as_str() {
                "known-error" => 0,
                "operation-constraint" => 1,
                _ => 2,
            },
            entry.scope == "default",
            std::cmp::Reverse(entry.revision),
        )
    });
    let mut rendered = String::new();
    let mut used = 0;
    for entry in entries {
        let label = BUILTIN_CATEGORIES
            .iter()
            .find(|(id, _)| *id == entry.category)
            .map(|(_, label)| *label)
            .unwrap_or(&entry.category);
        let next = format!("\n- [{label}] {}: {}", entry.title, entry.content);
        let count = next.chars().count();
        if used + count > budget {
            continue;
        }
        rendered.push_str(&next);
        used += count;
    }
    rendered
}

pub fn valid_name(name: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").unwrap());
    PATTERN.is_match(name)
}

fn safe_existing_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn path_for(root: &Path, name: &str) -> Result<PathBuf, ToolBodyError> {
    if !valid_name(name) {
        return Err(ToolBodyError::plain(
            "memory name must be lowercase kebab-case",
        ));
    }
    Ok(root.join(format!("{name}.md")))
}

pub fn install(ctx: &Context, root: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&root).map_err(|error| format!("memory directory: {error}"))?;
    let store = Arc::new(futures::executor::block_on(MemoryStore::open(
        root.clone(),
    ))?);
    ctx.register_service(store.clone());
    let learning = Arc::new(futures::executor::block_on(
        learning::LearningStore::open_or_disabled(root.clone()),
    ));
    ctx.register_service(learning.clone());
    learning_runtime::install(ctx, learning.clone())?;
    experience_reuse::install(ctx, learning)?;
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "memory requires the tools service".to_string())?;

    let listener: Arc<Listener> = Arc::new(|_ctx, args| {
        let execution = args
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|slot| slot.as_ref().clone());
        let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
        Box::pin(async move {
            if let Some(execution) = execution
                && execution.name == "memory"
                && matches!(
                    execution.arguments.get("action").and_then(|v| v.as_str()),
                    Some("write" | "remove")
                )
            {
                return Some(arc(PreToolDecision::Ask {
                    reason: Some("写入或删除长期记忆需要用户确认".to_string()),
                    grant_key: Some("tool:memory-mutation".to_string()),
                    rememberable: true,
                }));
            }
            let Some(next) = next else {
                return Some(arc(PreToolDecision::Allow));
            };
            Some(next.call().await)
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    ));

    tools.register(
        ctx,
        ToolDefinition {
            name: "memory".to_string(),
            description: "List, search, read, write, or remove persistent memories shared across tasks and models. Search known-error lessons before repeating a failed approach. A useful lesson states its trigger, wrong approach, verified correction and verification step. Use write/remove only when the user asks to remember or forget durable information; never store credentials.".to_string(),
            parameters: serde_json::json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "action": { "type": "string", "enum": ["list", "search", "read", "write", "remove"] },
                    "query": { "type": "string" },
                    "name": { "type": "string" },
                    "title": { "type": "string" },
                    "content": { "type": "string" },
                    "scope": { "type": "string" },
                    "category": { "type": "string", "enum": ["user-preference", "tool-capability", "known-error", "project-knowledge", "operation-constraint", "custom"] },
                    "enabled": { "type": "boolean" },
                    "expectedRevision": { "type": "integer" }
                },
                "required": ["action"]
            }),
            output: ToolOutputDefinition {
                schema: serde_json::json!({}),
                render: Arc::new(|_args, value| Ok(vec![dsh_llm::ContentBlock::Text {
                    text: serde_json::to_string_pretty(value).unwrap_or_default(),
                }])),
                presentation_meta: None,
            },
            timeout_ms: Some(10_000),
            is_concurrency_safe: Some(Arc::new(|args| {
                matches!(args.get("action").and_then(|v| v.as_str()), Some("list" | "search" | "read"))
            })),
            execute: Arc::new(move |args, _run: &ToolRunContext| {
                let root = root.clone();
                let store = store.clone();
                let args = args.clone();
                Box::pin(async move {
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or_default();
                    match action {
                        "list" | "search" => {
                            let mut names = Vec::new();
                            let mut entries = tokio::fs::read_dir(&root).await.map_err(|e| ToolBodyError::plain(e.to_string()))?;
                            while let Some(entry) = entries.next_entry().await.map_err(|e| ToolBodyError::plain(e.to_string()))? {
                                if !safe_existing_file(&entry.path()) { continue; }
                                if let Some(name) = entry.file_name().to_str().and_then(|n| n.strip_suffix(".md")) {
                                    if valid_name(name) { names.push(name.to_string()); }
                                }
                            }
                            names.sort();
                            let mut entries = store.list(
                                args.get("scope").and_then(|v| v.as_str()),
                                args.get("category").and_then(|v| v.as_str()),
                            ).await;
                            if action == "search" {
                                let query = args.get("query").and_then(|v|v.as_str()).unwrap_or_default().to_lowercase();
                                entries.retain(|entry| entry.enabled && format!("{} {}",entry.title,entry.content).to_lowercase().contains(&query));
                                entries.sort_by_key(|entry| entry.category != "known-error");
                            }
                            Ok(serde_json::json!({ "names": names, "entries": entries }))
                        }
                        "read" => {
                            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                            if let Some(entry) = store.list(None,None).await.into_iter().find(|entry|entry.id == name) {
                                return Ok(serde_json::json!({"name":name,"content":entry.content,"entry":entry}));
                            }
                            let path = path_for(&root, name)?;
                            if !safe_existing_file(&path) {
                                return Err(ToolBodyError::plain("memory file must be a regular non-link file"));
                            }
                            let content = tokio::fs::read_to_string(path).await.map_err(|e| ToolBodyError::plain(e.to_string()))?;
                            Ok(serde_json::json!({ "name": name, "content": content }))
                        }
                        "write" => {
                            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or_default();
                            if content.trim().is_empty() { return Err(ToolBodyError::plain("memory content must be non-empty")); }
                            path_for(&root, name)?;
                            let saved = store.upsert(MemoryEntry {
                                id: name.to_string(),
                                scope: args.get("scope").and_then(|v| v.as_str()).unwrap_or("default").to_string(),
                                category: args.get("category").and_then(|v| v.as_str()).unwrap_or("custom").to_string(),
                                title: args.get("title").and_then(|v| v.as_str()).unwrap_or(name).to_string(),
                                content: content.to_string(),
                                enabled: args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                                revision: 0,
                            }, args.get("expectedRevision").and_then(|v| v.as_u64())).await.map_err(ToolBodyError::plain)?;
                            Ok(serde_json::json!({ "name": name, "written": true, "entry": saved }))
                        }
                        "remove" => {
                            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                            let path = path_for(&root, name)?;
                            let structured = store.remove(name, args.get("expectedRevision").and_then(|v| v.as_u64())).await.map_err(ToolBodyError::plain)?;
                            match tokio::fs::remove_file(path).await {
                                Ok(()) => Ok(serde_json::json!({ "name": name, "removed": true })),
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({ "name": name, "removed": structured })),
                                Err(error) => Err(ToolBodyError::plain(error.to_string())),
                            }
                        }
                        _ => Err(ToolBodyError::plain("unknown memory action")),
                    }
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, category: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            scope: "default".to_string(),
            category: category.to_string(),
            title: "测试记忆".to_string(),
            content: content.to_string(),
            enabled: true,
            revision: 0,
        }
    }

    #[tokio::test]
    async fn categorized_entries_survive_reopen_and_filter_by_scope() {
        let root = std::env::temp_dir().join(format!("dsh-memory-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(root.clone()).await.unwrap();
        store
            .upsert(entry("one", "known-error", "不要重复旧错误"), None)
            .await
            .unwrap();
        let mut scoped = entry("two", "tool-capability", "已有浏览器工具");
        scoped.scope = "reviewer".to_string();
        store.upsert(scoped, None).await.unwrap();
        drop(store);
        let reopened = MemoryStore::open(root.clone()).await.unwrap();
        assert_eq!(reopened.list(Some("reviewer"), None).await.len(), 1);
        assert_eq!(
            reopened.list(None, Some("known-error")).await[0].content,
            "不要重复旧错误"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn stale_edits_are_rejected_and_disabled_entries_are_not_rendered() {
        let root = std::env::temp_dir().join(format!("dsh-memory-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(root.clone()).await.unwrap();
        let saved = store
            .upsert(entry("one", "user-preference", "使用中文"), None)
            .await
            .unwrap();
        let mut changed = saved.clone();
        changed.content = "使用简体中文".to_string();
        let changed = store.upsert(changed, Some(saved.revision)).await.unwrap();
        assert!(
            store
                .upsert(saved, Some(1))
                .await
                .unwrap_err()
                .contains("now 2")
        );
        let mut disabled = changed;
        disabled.enabled = false;
        store.upsert(disabled, Some(2)).await.unwrap();
        assert!(store.render_enabled("default", 2200).await.is_empty());
        assert!(render_enabled_file(&root, "default", 2200).is_empty());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn failed_persistence_does_not_change_live_memory() {
        let root = std::env::temp_dir().join(format!("dsh-memory-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(root.clone()).await.unwrap();
        let saved = store
            .upsert(entry("one", "known-error", "verified correction"), None)
            .await
            .unwrap();
        tokio::fs::remove_file(root.join("entries.json"))
            .await
            .unwrap();
        tokio::fs::create_dir(root.join("entries.json"))
            .await
            .unwrap();
        let mut changed = saved.clone();
        changed.content = "uncommitted correction".into();
        assert!(store.upsert(changed, Some(saved.revision)).await.is_err());
        assert_eq!(store.list(None, None).await[0], saved);
        assert!(store.remove("one", Some(saved.revision)).await.is_err());
        assert_eq!(store.list(None, None).await[0], saved);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn tight_budget_keeps_lessons_and_skips_oversized_notes() {
        let root = std::env::temp_dir().join(format!("dsh-memory-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(root.clone()).await.unwrap();
        store
            .upsert(entry("large", "custom", &"x".repeat(3000)), None)
            .await
            .unwrap();
        store
            .upsert(
                entry("lesson", "known-error", "verify the path before moving"),
                None,
            )
            .await
            .unwrap();
        let rendered = store.render_enabled("default", 100).await;
        assert!(rendered.contains("verify the path"));
        assert!(rendered.chars().count() <= 100);
        assert!(!rendered.contains("xxxx"));
        let reopened = MemoryStore::open(root.clone()).await.unwrap();
        assert_eq!(reopened.render_enabled("default", 100).await, rendered);
        assert!(render_enabled_file(&root, "other-model", 100).contains("verify the path"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
