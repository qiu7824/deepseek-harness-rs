//! Explicitly approved, single-file workspace operations with recovery.
use crate::{artifacts::Artifacts, workspace_resources::Resources};
use dsh_fs::{FileSystem, FsInfoType, FsTarget, ResolveOptions};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRunContext, ToolRuntime};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn error(message: impl Into<String>, code: &str) -> ToolBodyError {
    ToolBodyError::coded(message, "FileOperationError", code)
}
struct Selection {
    root: PathBuf,
    path: PathBuf,
    relative: String,
    target: FsTarget,
}

async fn select(
    fs: &Arc<dyn FileSystem>,
    resources: &Resources,
    owner: &str,
    workspace: &Path,
    raw: &str,
    signal: dsh_tools::AbortPredicate,
) -> Result<Selection, ToolBodyError> {
    if raw.trim().is_empty() || raw.chars().any(char::is_control) {
        return Err(error("需要一个普通文件路径", "FILE_PATH_INVALID"));
    }
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        workspace.join(raw)
    };
    dsh_workspace_resources::checked_path(&candidate).map_err(|e| error(e, "FILE_PATH_INVALID"))?;
    let target = fs
        .resolve(
            raw,
            Some(&ResolveOptions {
                cwd: Some(workspace.to_string_lossy().into()),
                signal: Some(signal),
            }),
        )
        .await
        .map_err(|e| error(e.to_string(), e.code.as_str()))?;
    fs.authorize_write(&target)
        .await
        .map_err(|e| error(e.to_string(), e.code.as_str()))?;
    let path = PathBuf::from(fs.process_path(&target));
    if !path.is_absolute() {
        return Err(error(
            "该操作仅支持当前本地工作区",
            "LOCAL_WORKSPACE_REQUIRED",
        ));
    }
    let mut roots = vec![workspace.to_path_buf()];
    roots.extend(
        resources
            .list(Some(owner))
            .map_err(|e| error(e, "FILE_SCOPE_UNAVAILABLE"))?
            .into_iter()
            .filter(|row| row.owner == owner && row.kind != "trash" && !row.path.is_empty())
            .map(|row| PathBuf::from(row.path)),
    );
    for root in roots {
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if let Ok(relative) = path.strip_prefix(&root) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            let relative = relative.to_string_lossy().replace('\\', "/");
            if crate::web_preview::safe_relative(&relative).is_some() {
                return Ok(Selection {
                    root,
                    path,
                    relative,
                    target,
                });
            }
        }
    }
    Err(error(
        "只能操作当前工作区或本会话拥有的临时文件",
        "FILE_SCOPE_DENIED",
    ))
}

async fn execute(
    ctx: cordis::Context,
    resources: Arc<Resources>,
    artifacts: Arc<Artifacts>,
    args: Value,
    run: ToolRunContext,
) -> Result<Value, ToolBodyError> {
    let effects = run.track_cancellable_effects();
    let agent = run
        .agent
        .as_ref()
        .ok_or_else(|| error("文件操作需要当前会话", "FILE_SCOPE_REQUIRED"))?;
    let cwd = agent
        .session()
        .header()
        .cwd
        .as_deref()
        .ok_or_else(|| error("文件操作需要当前工作区", "FILE_SCOPE_REQUIRED"))?;
    if cwd.starts_with("dsh-remote://") {
        return Err(error(
            "该操作仅支持当前本地工作区",
            "LOCAL_WORKSPACE_REQUIRED",
        ));
    }
    let workspace =
        std::fs::canonicalize(cwd).map_err(|e| error(e.to_string(), "FILE_SCOPE_UNAVAILABLE"))?;
    let fs = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .ok_or_else(|| error("文件系统不可用", "FILE_SCOPE_UNAVAILABLE"))?
        .as_ref()
        .clone();
    let fs = fs.for_tool(Some(agent.id().as_str())).unwrap_or(fs);
    let signal = run.signal.lock().clone();
    let source = select(
        &fs,
        &resources,
        agent.id().as_str(),
        &workspace,
        args["file_path"].as_str().unwrap_or_default(),
        signal.clone(),
    )
    .await?;
    let info = fs
        .stat(&source.target, Some(signal.clone()))
        .await
        .map_err(|e| error(e.to_string(), e.code.as_str()))?
        .filter(|i| i.kind == FsInfoType::File)
        .ok_or_else(|| error("只支持单个普通文件，不删除目录或链接", "FILE_NOT_REGULAR"))?;
    if info.size.unwrap_or(u64::MAX) > 128 * 1024 * 1024 {
        return Err(error("文件操作上限为 128 MiB", "FILE_OPERATION_TOO_LARGE"));
    }
    let action = args["action"]
        .as_str()
        .ok_or_else(|| error("缺少操作", "FILE_ACTION_INVALID"))?;
    let destination = if action == "rename" {
        let new_path = args["new_path"]
            .as_str()
            .ok_or_else(|| error("重命名需要new_path", "FILE_ACTION_INVALID"))?;
        let destination = select(
            &fs,
            &resources,
            agent.id().as_str(),
            &source.root,
            new_path,
            signal.clone(),
        )
        .await?;
        if destination.root != source.root {
            return Err(error(
                "重命名须在同一工作区或同一临时资源内",
                "FILE_SCOPE_DENIED",
            ));
        }
        if fs
            .stat(&destination.target, Some(signal.clone()))
            .await
            .map_err(|e| error(e.to_string(), e.code.as_str()))?
            .is_some()
        {
            return Err(error("目标已存在，不覆盖任何文件", "FILE_TARGET_EXISTS"));
        }
        let path = source.path.clone();
        let head = tokio::task::spawn_blocking(move || crate::workspace_copy::head(&path))
            .await
            .map_err(|e| error(e.to_string(), "FILE_READ_FAILED"))?
            .map_err(|e| error(e, "FILE_READ_FAILED"))?;
        if !dsh_fs::formats::content_matches_extension(&destination.path, &head) {
            return Err(error(
                "重命名不会转换格式：文件内容不是目标扩展名对应的真实格式，原文件保留。DOCX/XLSX 请用 office_write 生成",
                "BINARY_FORMAT_REQUIRED",
            ));
        }
        Some(destination)
    } else if action == "delete" {
        None
    } else {
        return Err(error("未知文件操作", "FILE_ACTION_INVALID"));
    };
    let identity_root = source.root.clone();
    let identity_relative = source.relative.clone();
    let identity = tokio::task::spawn_blocking(move || {
        Artifacts::file_identity(&identity_root, &identity_relative)
    })
    .await
    .map_err(|e| error(e.to_string(), "FILE_READ_FAILED"))?
    .map_err(|e| error(e, "FILE_READ_FAILED"))?;
    let approval = ctx
        .get_typed::<Arc<ApprovalService>>("approval", false)
        .ok_or_else(|| error("人类审批服务不可用", "TOOL_APPROVAL_UNAVAILABLE"))?;
    let reason = match &destination {
        Some(destination) => format!(
            "重命名一个文件（{} 字节）：\n{}\n→ {}\n目标存在时不会覆盖。",
            identity["bytes"],
            source.path.display(),
            destination.path.display()
        ),
        None => format!(
            "将一个文件移入本会话可恢复的回收区（{} 字节）：\n{}\n不会永久删除或递归删除目录。",
            identity["bytes"],
            source.path.display()
        ),
    };
    match approval
        .request_human(&ApprovalRequest {
            agent: agent.clone(),
            tool_name: "file_manage".into(),
            call_id: Some(run.call_id.to_string()),
            reason: Some(reason),
            grant_key: None,
            rememberable: false,
            signal: Some(signal.clone()),
        })
        .await
        .map_err(|e| error(e, "TOOL_APPROVAL_FAILED"))?
    {
        ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways => {}
        ApprovalOutcome::TimedOut => {
            return Err(error(
                "本轮同类文件操作审批已超时；等待新的用户指令后再请求",
                "TOOL_APPROVAL_TIMED_OUT",
            ));
        }
        ApprovalOutcome::Cancelled => {
            return Err(error("文件操作已取消，原文件保留", "TOOL_ABORTED"));
        }
        _ => {
            return Err(error(
                "文件操作未获得人类批准，原文件保留",
                "TOOL_APPROVAL_DENIED",
            ));
        }
    }
    let current = select(
        &fs,
        &resources,
        agent.id().as_str(),
        &workspace,
        args["file_path"].as_str().unwrap(),
        signal.clone(),
    )
    .await?;
    if current.target.target_key != source.target.target_key {
        return Err(error("批准后路径已变化，未执行操作", "FILE_CHANGED"));
    }
    if let Some(destination) = &destination {
        fs.authorize_write(&destination.target)
            .await
            .map_err(|e| error(e.to_string(), e.code.as_str()))?;
    }
    let mut operation = json!({"action":if action=="delete"{"trash"}else{"rename"},"path":source.relative,"etag":identity["etag"],"sha256":identity["sha256"]});
    if let Some(destination) = &destination {
        operation["newPath"] = json!(destination.relative);
    }
    let owner = agent.id().as_str().to_owned();
    let root = source.root;
    let mut result = tokio::task::spawn_blocking(move || {
        artifacts.file_action_tracked(
            &owner,
            &root,
            &operation,
            &resources,
            signal.as_ref(),
            effects.as_ref(),
        )
    })
    .await
    .map_err(|e| error(e.to_string(), "FILE_OPERATION_FAILED"))?
    .map_err(|e| error(e, "FILE_OPERATION_FAILED"))?;
    result["path"] = json!(source.path);
    if let Some(destination) = destination {
        result["newPath"] = json!(destination.path);
    }
    Ok(result)
}

pub(crate) fn install(
    ctx: &cordis::Context,
    artifacts: Arc<Artifacts>,
    resources: Arc<Resources>,
) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("file_manage requires tools")?;
    let context = ctx.clone();
    tools.register(ctx,ToolDefinition{
        name:"file_manage".into(),
        description:"Rename or delete one regular file in the current local workspace or session-owned scratch. Always requires an explicit human approval for this exact operation. delete moves bytes into recoverable managed storage; it never permanently deletes. Renaming never overwrites another file. Directory recursion and links are rejected. Supply literal paths, not globs.".into(),
        parameters:json!({"type":"object","additionalProperties":false,"properties":{"action":{"type":"string","enum":["rename","delete"]},"file_path":{"type":"string","minLength":1,"maxLength":4096},"new_path":{"type":"string","minLength":1,"maxLength":4096}},"required":["action","file_path"],"oneOf":[{"type":"object","properties":{"action":{"type":"string","enum":["rename"]},"file_path":{},"new_path":{}},"required":["new_path"]},{"type":"object","properties":{"action":{"type":"string","enum":["delete"]},"file_path":{},"new_path":{}}}]}),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
        timeout_ms:Some(30_000),is_concurrency_safe:None,finalize_content:None,present_call:None,present_result:None,
        execute:Arc::new(move|args,run|{let args=args.clone();let run=run.clone();let ctx=context.clone();let artifacts=artifacts.clone();let resources=resources.clone();Box::pin(execute(ctx,resources,artifacts,args,run))}),
    }).map(|_|())
}

#[cfg(test)]
#[path = "file_operations_tests.rs"]
mod tests;
