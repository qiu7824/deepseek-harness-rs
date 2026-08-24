use std::path::{Path, PathBuf};
use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};

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
            description: "Explicitly list, read, write, or remove auditable Markdown memories. Never infer a write: use write/remove only when the user asks to remember or forget durable information.".to_string(),
            parameters: serde_json::json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "action": { "type": "string", "enum": ["list", "read", "write", "remove"] },
                    "name": { "type": "string" },
                    "content": { "type": "string" }
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
                matches!(args.get("action").and_then(|v| v.as_str()), Some("list" | "read"))
            })),
            execute: Arc::new(move |args, _run: &ToolRunContext| {
                let root = root.clone();
                let args = args.clone();
                Box::pin(async move {
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or_default();
                    match action {
                        "list" => {
                            let mut names = Vec::new();
                            let mut entries = tokio::fs::read_dir(&root).await.map_err(|e| ToolBodyError::plain(e.to_string()))?;
                            while let Some(entry) = entries.next_entry().await.map_err(|e| ToolBodyError::plain(e.to_string()))? {
                                if !safe_existing_file(&entry.path()) { continue; }
                                if let Some(name) = entry.file_name().to_str().and_then(|n| n.strip_suffix(".md")) {
                                    if valid_name(name) { names.push(name.to_string()); }
                                }
                            }
                            names.sort();
                            Ok(serde_json::json!({ "names": names }))
                        }
                        "read" => {
                            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
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
                            let path = path_for(&root, name)?;
                            dsh_atomic_write::write_file_atomic(
                                &path,
                                content.as_bytes(),
                                dsh_atomic_write::WriteFileAtomicOptions { mode: 0o600, dir_mode: Some(0o700) },
                            ).await.map_err(|e| ToolBodyError::plain(e.to_string()))?;
                            Ok(serde_json::json!({ "name": name, "written": true }))
                        }
                        "remove" => {
                            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                            let path = path_for(&root, name)?;
                            match tokio::fs::remove_file(path).await {
                                Ok(()) => Ok(serde_json::json!({ "name": name, "removed": true })),
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({ "name": name, "removed": false })),
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
