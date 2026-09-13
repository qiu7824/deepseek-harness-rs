//! One durable, revision-checked Markdown task board per registered workspace.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
const START: &str = "<!-- dsh-project-tasks:start -->";
const END: &str = "<!-- dsh-project-tasks:end -->";
pub const FILE: &str = "PROJECT_TASKS.md";
const STATES: &[&str] = &["todo", "in-progress", "done", "update", "feedback"];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(deserialize_with = "priority_value")]
    pub priority: u8,
    #[serde(default)]
    pub detail: String,
}
fn priority_value<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = Value::deserialize(deserializer)?;
    value
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && (0.0..=3.0).contains(n))
        .map(|n| n as u8)
        .ok_or_else(|| serde::de::Error::custom("priority must be an integer from 0 to 3"))
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn validate(rows: &[Task]) -> Result<(), String> {
    if rows.len() > 500 {
        return Err("任务数量超过500条".into());
    }
    let mut ids = std::collections::HashSet::new();
    for r in rows {
        if !ids.insert(&r.id)
            || r.id.is_empty()
            || r.id.len() > 128
            || !r.id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("任务标识无效或重复".into());
        }
        if r.title.trim().is_empty()
            || r.title.chars().count() > 300
            || r.title.contains(['\r', '\n'])
            || r.title.contains("<!--")
            || r.detail.chars().count() > 20000
            || r.detail.contains(START)
            || r.detail.contains(END)
            || r.priority > 3
            || !STATES.contains(&r.status.as_str())
        {
            return Err("任务标题、状态或优先级无效".into());
        }
    }
    Ok(())
}
fn parse(text: &str) -> Result<Vec<Task>, String> {
    let Some((_, after)) = text.split_once(START) else {
        return Ok(vec![]);
    };
    let (block, _) = after
        .split_once(END)
        .ok_or("任务区缺少结束标记，请先修复 Markdown 文件")?;
    let re=regex::Regex::new(r"^- \[([ xX])\] \[P([0-3])\] \[(todo|in-progress|done|update|feedback)\] (.+) <!-- task:([A-Za-z0-9-]+) -->$").unwrap();
    let mut rows: Vec<Task> = vec![];
    for line in block.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(c) = re.captures(line) {
            let status = if &c[1] == "x" || &c[1] == "X" {
                "done"
            } else if &c[3] == "done" {
                "todo"
            } else {
                &c[3]
            };
            rows.push(Task {
                id: c[5].into(),
                title: c[4].into(),
                status: status.into(),
                priority: c[2].parse().unwrap(),
                detail: String::new(),
            });
        } else if let Some(detail) = line.strip_prefix("  ") {
            let row = rows.last_mut().ok_or("任务说明缺少任务标题")?;
            if !row.detail.is_empty() {
                row.detail.push('\n')
            }
            row.detail.push_str(detail);
        } else {
            return Err("任务区包含无法识别的行；原文已保留，请修复格式后重试".into());
        }
    }
    validate(&rows)?;
    Ok(rows)
}
fn render(original: &str, rows: &[Task]) -> Result<String, String> {
    validate(rows)?;
    let mut block = format!("{START}\n");
    for row in rows {
        block.push_str(&format!(
            "- [{}] [P{}] [{}] {} <!-- task:{} -->\n",
            if row.status == "done" { "x" } else { " " },
            row.priority,
            row.status,
            row.title.trim(),
            row.id
        ));
        for line in row.detail.lines() {
            block.push_str("  ");
            block.push_str(line);
            block.push('\n')
        }
        block.push('\n');
    }
    block.push_str(END);
    if let Some((prefix, after)) = original.split_once(START) {
        let (_, suffix) = after.split_once(END).ok_or("任务区缺少结束标记")?;
        if after.contains(START) || suffix.contains(END) {
            return Err("任务区标记重复".into());
        }
        Ok(format!("{prefix}{block}{suffix}"))
    } else {
        let prefix = if original.is_empty() {
            "# 项目任务\n\n"
        } else {
            original
        };
        Ok(format!("{prefix}\n{block}\n"))
    }
}
async fn read(path: &Path) -> Result<String, String> {
    if let Ok(meta) = tokio::fs::symlink_metadata(path).await {
        if meta.file_type().is_symlink() || meta.len() > 1024 * 1024 {
            return Err("任务文件是链接或超过1MiB".into());
        }
    }
    match tokio::fs::read_to_string(path).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}
pub struct Board {
    registry: Arc<dsh_workspace::WorkspaceRegistry>,
}
impl Board {
    pub fn summary(&self, cwd: &str) -> String {
        let Ok(root) = std::fs::canonicalize(cwd) else {
            return String::new();
        };
        if !self
            .registry
            .list()
            .unwrap_or_default()
            .iter()
            .any(|w| std::fs::canonicalize(w.path()).is_ok_and(|p| p == root))
        {
            return String::new();
        }
        let path = root.join(FILE);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return String::new();
        };
        if meta.file_type().is_symlink() || meta.len() > 1024 * 1024 {
            return String::new();
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return String::new();
        };
        let Ok(mut tasks) = parse(&text) else {
            return String::new();
        };
        tasks.retain(|t| t.status != "done");
        tasks.sort_by_key(|t| t.priority);
        if tasks.is_empty() {
            return String::new();
        }
        let rows = tasks
            .iter()
            .take(8)
            .map(|t| {
                format!(
                    "- {} P{} [{}] {}",
                    t.id,
                    t.priority,
                    t.status,
                    t.title.chars().take(120).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Shared project task state in PROJECT_TASKS.md (use project_tasks to read/update it when relevant; verify completion before changing status):\n{rows}"
        )
    }
    pub fn new(registry: Arc<dsh_workspace::WorkspaceRegistry>) -> Self {
        Self { registry }
    }
    pub fn workspace(&self, args: &Value) -> Result<PathBuf, String> {
        let rows = self.registry.list()?;
        let workspace = rows
            .into_iter()
            .find(|w| {
                args["workspaceId"].as_str() == Some(w.id().as_str())
                    || args["sessionId"]
                        .as_str()
                        .is_some_and(|s| w.session_ids().iter().any(|id| id.as_str() == s))
            })
            .ok_or("没有找到当前工作区")?;
        std::fs::canonicalize(workspace.path()).map_err(|e| e.to_string())
    }
    pub async fn request(&self, action: &str, args: &Value) -> Result<Value, String> {
        self.at(&self.workspace(args)?, action, args).await
    }
    pub async fn at(&self, root: &Path, action: &str, args: &Value) -> Result<Value, String> {
        let canonical = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if self
            .registry
            .resolve_by_path(&canonical.to_string_lossy())
            .await?
            .is_none()
        {
            return Err("项目任务仅支持已登记的工作区".into());
        }
        let path = canonical.join(FILE);
        if action == "save" {
            let expected = args["revision"].as_str().ok_or("保存任务需要文件版本")?;
            let rows: Vec<Task> =
                serde_json::from_value(args["tasks"].clone()).map_err(|_| "任务列表无效")?;
            dsh_atomic_write::with_file_lock(&path, async {
                let current = read(&path).await?;
                if hash(&current) != expected {
                    return Err("任务文件已被其他窗口或 AI 更新，请刷新后合并修改".to_string());
                }
                let text = render(&current, &rows)?;
                if text.len() > 1024 * 1024 {
                    return Err("任务文件超过1MiB，原文已保留".to_string());
                }
                dsh_atomic_write::write_file_atomic(
                    &path,
                    text.as_bytes(),
                    dsh_atomic_write::WriteFileAtomicOptions {
                        mode: 0o600,
                        dir_mode: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
        } else if action != "list" {
            return Err("未知任务操作".into());
        }
        let text = read(&path).await?;
        let rows = parse(&text)?;
        Ok(json!({"path":path,"revision":hash(&text),"tasks":rows,"exists":path.is_file()}))
    }
    pub fn install_tool(self: &Arc<Self>, ctx: &cordis::Context) -> Result<(), String> {
        use dsh_tools::*;
        let board = self.clone();
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .ok_or("缺少工具运行时")?;
        tools.register(ctx,ToolDefinition{name:"project_tasks".into(),description:"Read or update the shared project task board in PROJECT_TASKS.md. All conversations in a workspace share it. Use list to obtain tasks and revision, then save the complete updated list with that revision. Track requested project work, completion, needed updates and user feedback without repeating the board in every chat reply. Preserve unrelated tasks; never claim work is done before verification.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"action":{"type":"string","enum":["list","save"]},"revision":{"type":"string"},"tasks":{"type":"array","maxItems":500,"items":{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":STATES},"priority":{"type":"integer","minimum":0,"maximum":3},"detail":{"type":"string"}},"required":["id","title","status","priority"],"additionalProperties":false}}},"required":["action"]}),output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,v|Ok(vec![dsh_llm::ContentBlock::Text{text:v.to_string()}])),presentation_meta:None},timeout_ms:Some(15000),is_concurrency_safe:Some(Arc::new(|_|false)),finalize_content:None,present_call:None,present_result:None,execute:Arc::new(move|args,run|{let board=board.clone();let args=args.clone();let agent=run.execution.agent.clone();Box::pin(async move{let agent=agent.ok_or_else(||ToolBodyError::plain("需要当前工作区"))?;let cwd=agent.session().header().cwd.clone().ok_or_else(||ToolBodyError::plain("需要当前工作区"))?;board.at(Path::new(&cwd),args["action"].as_str().unwrap_or("list"),&args).await.map_err(ToolBodyError::plain)})})})?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_preserves_surrounding_notes_and_checkbox_edits() {
        let rows = vec![Task {
            id: "one".into(),
            title: "Fix parser".into(),
            status: "feedback".into(),
            priority: 0,
            detail: "用户反馈\n第二行".into(),
        }];
        let text = render("# Existing notes\n", &rows).unwrap();
        assert!(text.starts_with("# Existing notes"));
        assert_eq!(parse(&text).unwrap(), rows);
        assert_eq!(
            parse(&text.replace("- [ ]", "- [x]")).unwrap()[0].status,
            "done"
        );
        let changed = render(&(text + "\nAfterword"), &[]).unwrap();
        assert!(changed.ends_with("Afterword"));
    }
    #[test]
    fn malformed_blocks_and_duplicate_ids_do_not_overwrite() {
        assert!(render(START, &[]).is_err());
        assert!(parse(&format!("{START}\nforeign content\n{END}")).is_err());
        let row = Task {
            id: "x".into(),
            title: "a".into(),
            status: "todo".into(),
            priority: 1,
            detail: "".into(),
        };
        assert!(validate(&[row.clone(), row]).is_err());
    }
}
