//! Bounded local memory discovery and opt-in, conflict-preserving synchronization.
use dsh_tool_memory_local::{MemoryEntry, MemoryStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    title: String,
    category: String,
    content: String,
    file: PathBuf,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    name: String,
    scope: String,
    auto: bool,
    #[serde(default)]
    entries: BTreeMap<String, Stamp>,
    #[serde(default)]
    last_report: Value,
}
#[derive(Clone, Serialize, Deserialize)]
struct Stamp {
    revision: u64,
    hash: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    sources: BTreeMap<String, Config>,
}
pub struct Imports {
    ctx: cordis::Context,
    file: PathBuf,
    home: PathBuf,
    lock: tokio::sync::Mutex<()>,
    registry: Arc<dsh_workspace::WorkspaceRegistry>,
}
fn files(root: &Path, names: Option<&[&str]>) -> Vec<PathBuf> {
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return vec![];
    };
    let mut rows = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(512)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !kind.is_file()
                || kind.is_symlink()
                || !matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("md" | "mdc")
                )
                || names.is_some_and(|names| !names.contains(&name.as_str()))
            {
                return None;
            }
            let actual = std::fs::canonicalize(&path).ok()?;
            actual.starts_with(&canonical).then_some(actual)
        })
        .take(64)
        .collect::<Vec<_>>();
    rows.sort();
    rows
}
pub fn discover(home: &Path, workspaces: &[PathBuf]) -> Vec<Source> {
    let mut result = vec![];
    let mut add = |name: &str, root: PathBuf, allowed: Option<&[&str]>| {
        let found = files(&root, allowed);
        if !found.is_empty() {
            result.push(Source {
                id: hash(&format!("{name}:{}", root.to_string_lossy())),
                name: name.into(),
                root,
                files: found,
            });
        }
    };
    add(
        "Codex",
        home.join(".codex/memories"),
        Some(&["MEMORY.md", "memory_summary.md"]),
    );
    add(
        "Hermes",
        home.join(".hermes/memories"),
        Some(&["MEMORY.md", "USER.md"]),
    );
    add(
        "Claude Code · 全局规则",
        home.join(".claude"),
        Some(&["CLAUDE.md"]),
    );
    for dir in std::fs::read_dir(home.join(".claude/projects"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(128)
    {
        if dir
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            add(
                &format!("Claude Code · {}", dir.file_name().to_string_lossy()),
                dir.path().join("memory"),
                None,
            );
        }
    }
    add("Gemini CLI", home.join(".gemini"), Some(&["GEMINI.md"]));
    add(
        "OpenClaw",
        home.join(".openclaw/workspace"),
        Some(&["MEMORY.md", "USER.md"]),
    );
    add(
        "Windsurf",
        home.join(".codeium/windsurf/memories"),
        Some(&["global_rules.md"]),
    );
    for project in workspaces.iter().take(64) {
        let name = project.file_name().unwrap_or_default().to_string_lossy();
        let Ok(base) = std::fs::canonicalize(project) else {
            continue;
        };
        for (app, relative) in [
            ("Cursor", ".cursor/rules"),
            ("Windsurf", ".windsurf/rules"),
            ("Devin", ".devin/rules"),
        ] {
            let root = project.join(relative);
            if std::fs::canonicalize(&root).is_ok_and(|path| path.starts_with(&base)) {
                add(&format!("{app} · {name}"), root, None);
            }
        }
    }
    result
}
fn sanitized(text: &str) -> String {
    let secret=regex::Regex::new(r#"(?i)(?:api[_ -]?key|(?:access[_-]|refresh[_-])?token|password|secret(?:_access_key)?|authorization|密码|令牌|密钥)\s*[:=]\s*\S.{6,}|\bsk-[A-Za-z0-9_-]{16,}|\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"#).unwrap();
    let credentials =
        regex::Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^\s/@:]+:[^\s/@]+@").unwrap();
    let mut private = false;
    let mut rows = vec![];
    for line in text.lines() {
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            private = true;
            rows.push("[私钥已过滤]".to_string());
            continue;
        }
        if private {
            if line.contains("-----END") {
                private = false
            }
            continue;
        }
        let line = credentials.replace_all(line, "${1}[凭据已过滤]@");
        rows.push(secret.replace_all(&line, "[敏感凭据已过滤]").into_owned());
    }
    rows.join("\n")
}
fn category(title: &str, content: &str, file: &Path) -> String {
    let text = format!("{title}\n{content}").to_lowercase();
    if file.file_name().is_some_and(|n| n == "USER.md")
        || ["偏好", "prefer", "communication", "称呼"]
            .iter()
            .any(|k| text.contains(k))
    {
        "user-preference"
    } else if ["禁止", "必须", "不要", "must ", "never ", "constraint"]
        .iter()
        .any(|k| text.contains(k))
    {
        "operation-constraint"
    } else if ["错误", "报错", "bug", "workaround", "failure"]
        .iter()
        .any(|k| text.contains(k))
    {
        "known-error"
    } else if ["工具", "tool", "command", "运行环境"]
        .iter()
        .any(|k| text.contains(k))
    {
        "tool-capability"
    } else {
        "project-knowledge"
    }
    .into()
}
fn chunks(source: &Source, file: &Path, text: &str) -> Vec<Item> {
    let text = sanitized(text);
    let title = file.file_stem().unwrap_or_default().to_string_lossy();
    let mut heading = title.chars().take(180).collect::<String>();
    let mut block = String::new();
    let mut sections = vec![];
    for line in text.lines() {
        if line.starts_with('#') && !block.trim().is_empty() {
            sections.push((heading.clone(), std::mem::take(&mut block)));
        }
        if line.starts_with('#') {
            heading = line
                .trim_start_matches('#')
                .trim()
                .chars()
                .take(180)
                .collect();
        } else {
            if block.chars().count() + line.chars().count() > 8000 && !block.is_empty() {
                sections.push((heading.clone(), std::mem::take(&mut block)));
            }
            block.push_str(line);
            block.push('\n');
        }
    }
    if !block.trim().is_empty() {
        sections.push((heading, block));
    }
    let mut occurrences = BTreeMap::<String, usize>::new();
    sections
        .into_iter()
        .filter_map(|(title, content)| {
            let content = content.trim().to_string();
            if content.len() < 3 || content.chars().count() > 20000 {
                return None;
            }
            let title = if title.trim().is_empty() {
                file.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .chars()
                    .take(180)
                    .collect()
            } else {
                title
            };
            let occurrence = occurrences.entry(title.clone()).or_default();
            let index = *occurrence;
            *occurrence += 1;
            Some(Item {
                id: format!(
                    "import-{}",
                    hash(&format!(
                        "{}:{}:{title}:{index}",
                        source.id,
                        file.to_string_lossy()
                    ))
                ),
                category: category(&title, &content, file),
                title,
                content,
                file: file.to_path_buf(),
            })
        })
        .collect()
}
fn preview(source: &Source) -> Result<Value, String> {
    let mut items = vec![];
    let mut total = 0u64;
    let mut skipped = vec![];
    for file in &source.files {
        let meta = std::fs::metadata(file).map_err(|e| e.to_string())?;
        total += meta.len();
        if meta.len() > 512 * 1024 || total > 4 * 1024 * 1024 {
            skipped.push(file.clone());
            continue;
        }
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        items.extend(chunks(source, file, &text));
        if items.len() > 256 {
            return Err("来源超过256个记忆片段，请拆分或选择较小的来源".into());
        }
    }
    let revision = hash(&serde_json::to_string(&items).unwrap());
    Ok(json!({"source":source,"revision":revision,"items":items,"skipped":skipped}))
}
impl Imports {
    pub fn new(
        ctx: &cordis::Context,
        data: &Path,
        registry: Arc<dsh_workspace::WorkspaceRegistry>,
    ) -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            ctx: ctx.clone(),
            file: data.join("memory/imports.json"),
            home,
            registry,
            lock: tokio::sync::Mutex::new(()),
        }
    }
    fn sources(&self) -> Vec<Source> {
        discover(
            &self.home,
            &self
                .registry
                .list()
                .unwrap_or_default()
                .iter()
                .map(|w| PathBuf::from(w.path()))
                .collect::<Vec<_>>(),
        )
    }
    async fn document(&self) -> Result<Document, String> {
        match tokio::fs::read(&self.file).await {
            Ok(b) => {
                if b.len() > 2 * 1024 * 1024 {
                    return Err("导入状态超过大小限制".into());
                }
                serde_json::from_slice(&b).map_err(|e| e.to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Document::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    async fn persist(&self, doc: &Document) -> Result<(), String> {
        dsh_atomic_write::write_file_atomic(
            &self.file,
            &serde_json::to_vec_pretty(doc).unwrap(),
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|e| e.to_string())
    }
    async fn record_error(&self, id: &str, error: &str) {
        let _guard = self.lock.lock().await;
        if let Ok(mut doc) = self.document().await {
            if let Some(config) = doc.sources.get_mut(id) {
                config.last_report = json!({"error":error.chars().take(1000).collect::<String>(),"lastSync":dsh_workspace_resources::now()});
                let _ = self.persist(&doc).await;
            }
        }
    }
    pub async fn request(&self, action: &str, args: &Value) -> Result<Value, String> {
        let _guard = self.lock.lock().await;
        let mut doc = self.document().await?;
        if action == "discover" {
            return Ok(
                json!({"sources":self.sources(),"configured":doc.sources,"workspaces":self.registry.list()?.iter().map(|w|json!({"id":w.id(),"title":w.title(),"path":w.path()})).collect::<Vec<_>>()}),
            );
        }
        let id = args["sourceId"].as_str().ok_or("请选择记忆来源")?;
        if action == "configure" {
            let config = doc.sources.get_mut(id).ok_or("来源尚未导入")?;
            config.auto = args["auto"].as_bool().ok_or("同步开关无效")?;
            self.persist(&doc).await?;
            return Ok(json!({"saved":true}));
        }
        let source = self
            .sources()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("来源不存在或已移除")?;
        let data = preview(&source)?;
        if action == "preview" {
            return Ok(data);
        }
        if action != "import" && action != "sync" {
            return Err("未知导入操作".into());
        }
        if action == "import" {
            if args["revision"] != data["revision"] {
                return Err("来源记忆已经变化，请重新预览".into());
            }
            let scope = args["scope"].as_str().unwrap_or("default");
            if scope != "default" && !self.registry.list()?.iter().any(|w| w.path() == scope) {
                return Err("记忆范围不是已登记工作区".into());
            }
            let config = doc.sources.entry(id.into()).or_default();
            config.name = source.name.clone();
            if !config.entries.is_empty() && config.scope != scope {
                return Err("已导入来源不能直接改变范围，请在记忆列表中调整".into());
            }
            config.scope = scope.into();
            config.auto = args["auto"].as_bool().unwrap_or(false);
        }
        let config = doc.sources.get_mut(id).ok_or("请先预览并选择导入来源")?;
        let store = self
            .ctx
            .get_typed::<Arc<MemoryStore>>("memoryStore", false)
            .ok_or("记忆服务不可用")?;
        let existing = store.list(None, None).await;
        let mut changes = vec![];
        let mut unchanged = 0;
        let mut conflicts = 0;
        let mut duplicate = 0;
        let items: Vec<Item> =
            serde_json::from_value(data["items"].clone()).map_err(|e| e.to_string())?;
        for item in items {
            let content_hash = hash(&format!(
                "{}:{}:{}",
                item.title, item.category, item.content
            ));
            let old = config.entries.get(&item.id);
            let current = existing.iter().find(|r| r.id == item.id);
            if let (Some(old), Some(current)) = (old, current) {
                if current.revision != old.revision {
                    conflicts += 1;
                    continue;
                }
                if old.hash == content_hash {
                    unchanged += 1;
                    continue;
                }
            } else if old.is_some() {
                conflicts += 1;
                continue;
            } else if let Some(current) = current {
                if current.content != item.content {
                    conflicts += 1;
                    continue;
                }
            } else if existing
                .iter()
                .any(|r| r.scope == config.scope && r.content == item.content)
            {
                duplicate += 1;
                continue;
            }
            let revision = current.map(|r| r.revision);
            changes.push((
                MemoryEntry {
                    id: item.id,
                    title: item.title,
                    content: item.content,
                    category: item.category,
                    scope: config.scope.clone(),
                    enabled: current.map(|r| r.enabled).unwrap_or(true),
                    revision: 0,
                },
                revision,
            ));
        }
        let changed = changes.len();
        let saved = store.import_entries(changes).await?;
        for row in saved {
            config.entries.insert(
                row.id.clone(),
                Stamp {
                    revision: row.revision,
                    hash: hash(&format!("{}:{}:{}", row.title, row.category, row.content)),
                },
            );
        }
        let report = json!({"imported":changed,"unchanged":unchanged,"conflicts":conflicts,"duplicates":duplicate,"skipped":data["skipped"],"lastSync":dsh_workspace_resources::now()});
        config.last_report = report.clone();
        self.persist(&doc).await?;
        Ok(report)
    }
    pub fn start(self: &Arc<Self>, ctx: &cordis::Context) {
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(service) = weak.upgrade() else { break };
                let ids = service
                    .document()
                    .await
                    .map(|d| {
                        d.sources
                            .into_iter()
                            .filter(|(_, c)| c.auto)
                            .map(|(id, _)| id)
                            .take(32)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for id in ids {
                    if let Err(error) = service.request("sync", &json!({"sourceId":id})).await {
                        service.record_error(&id, &error).await;
                    }
                }
            }
        });
        let _ = ctx.effect(
            "memory import synchronization",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    task.abort();
                    Box::pin(async {})
                }))
            }),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_sources_are_detected_without_credentials_or_transcripts() {
        let root =
            std::env::temp_dir().join(format!("dsh-import-detection-{}", uuid::Uuid::new_v4()));
        for (folder, name) in [
            (".codex/memories", "MEMORY.md"),
            (".hermes/memories", "USER.md"),
            (".claude", "CLAUDE.md"),
            (".gemini", "GEMINI.md"),
            (".openclaw/workspace", "MEMORY.md"),
            (".codeium/windsurf/memories", "global_rules.md"),
            ("project/.cursor/rules", "main.mdc"),
        ] {
            let dir = root.join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), "Prefer stable tests").unwrap();
        }
        std::fs::write(root.join(".codex/memories/raw_memories.md"), "transcript").unwrap();
        std::fs::write(root.join(".codex/auth.json"), "secret").unwrap();
        let sources = discover(&root, &[root.join("project")]);
        assert_eq!(sources.len(), 7);
        assert!(sources.iter().all(|source| source.files.len() == 1));
        assert!(
            root.canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn secrets_are_removed_and_preferences_are_classified() {
        let s = sanitized(
            "prefer concise replies\napi_key=sk-123456789012345678901234\n-----BEGIN PRIVATE KEY-----\nsecret material\n-----END PRIVATE KEY-----",
        );
        assert!(!s.contains("1234567890"));
        assert!(!s.contains("secret material"));
        assert_eq!(
            category("Profile", &s, Path::new("USER.md")),
            "user-preference"
        );
    }
    #[test]
    fn imported_ids_are_stable_across_content_changes() {
        let source = Source {
            id: "fixture".into(),
            name: "Codex".into(),
            root: PathBuf::new(),
            files: vec![],
        };
        let a = chunks(&source, Path::new("MEMORY.md"), "# Work\nuse tests");
        let b = chunks(&source, Path::new("MEMORY.md"), "# Work\nuse careful tests");
        assert_eq!(a[0].id, b[0].id);
        assert_ne!(a[0].content, b[0].content);
        let c = chunks(
            &source,
            Path::new("MEMORY.md"),
            "# Earlier\nnew topic\n# Work\nuse careful tests",
        );
        assert_eq!(a[0].id, c[1].id);
    }
}
