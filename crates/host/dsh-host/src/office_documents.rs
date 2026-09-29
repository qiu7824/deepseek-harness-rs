//! Declarative Office data tools over the session's ordinary file permissions.
use cordis::Context;
use dsh_fs::{FileSystem, FsInfoType, FsWriteIntent, ResolveOptions};
use dsh_tools::{
    FileLocation, ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

#[path = "office_documents/codec.rs"]
mod codec;
#[cfg(test)]
#[path = "office_documents/tests.rs"]
mod tests;

fn error(message: impl Into<String>) -> ToolBodyError {
    let message = message.into();
    let code = if message.starts_with("OFFICE_ABORTED:") {
        "OFFICE_ABORTED"
    } else if message.starts_with("OFFICE_FORMAT_UNSUPPORTED:") {
        "OFFICE_FORMAT_UNSUPPORTED"
    } else {
        "OFFICE_DOCUMENT_FAILED"
    };
    ToolBodyError::coded(message, "OfficeDocumentError", code)
}
fn fs_error(error: dsh_fs::FsError) -> ToolBodyError {
    ToolBodyError::coded(error.to_string(), "FsError", error.code.as_str())
}
fn scalar_schema() -> Value {
    json!({"oneOf":[{"type":"string","maxLength":32767},{"type":"number"},{"type":"boolean"},{"type":"null"}]})
}
fn rows_schema() -> Value {
    json!({"type":"array","maxItems":10000,"items":{"type":"array","maxItems":128,"items":scalar_schema()}})
}
fn parameters(write: bool) -> Value {
    if !write {
        return json!({"type":"object","additionalProperties":false,"properties":{
        "file_path":{"type":"string","minLength":1},"sheet_name":{"type":"string","minLength":1},
        "table_index":{"type":"integer","minimum":1,"maximum":10000},
        "start_row":{"type":"integer","minimum":1,"maximum":1048576},
        "row_limit":{"type":"integer","minimum":1,"maximum":1000},
        "paragraph_limit":{"type":"integer","minimum":1,"maximum":1000}},"required":["file_path"]});
    }
    json!({"type":"object","additionalProperties":false,"properties":{
        "file_path":{"type":"string","minLength":1},"format":{"type":"string","enum":["docx","xlsx"]},
        "overwrite":{"type":"boolean","description":"False by default; replacing an existing file requires explicit human approval."},
        "paragraphs":{"type":"array","maxItems":1000,"items":{"type":"string","maxLength":32767}},
        "tables":{"type":"array","maxItems":32,"items":{"type":"object","additionalProperties":false,"properties":{"rows":rows_schema()},"required":["rows"]}},
        "sheets":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"object","additionalProperties":false,"properties":{"name":{"type":"string","minLength":1,"maxLength":31},"rows":rows_schema()},"required":["name","rows"]}}},
        "required":["file_path","format"]})
}

pub(crate) fn install(ctx: &Context) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("Office tools require tools")?;
    for write in [false, true] {
        let context = ctx.clone();
        tools.register(ctx,ToolDefinition{
            name:if write{"office_write"}else{"office_read"}.into(),
            description:if write {
                "Create a real DOCX or XLSX directly through the Host file service; no shell, Python, sandbox process or Office script is needed. DOCX uses paragraphs followed by rectangular tables of scalar rows; XLSX uses named sheets of scalar rows. All JSON strings are literal text (including phone numbers, leading zeroes and strings beginning with =), never formulas. Use strings for identifiers or exact long numbers. file_path must end in the requested format. Creates a new file by default; overwrite=true requires explicit approval and rejects concurrent changes. Returns actual format, counts, bytes and SHA-256 after structural checks; this is not a statement of business or visual correctness. Use office_read to inspect data and office_render for WPS page images. Legacy DOC/XLS are unsupported."
            }else{
                "Read safe DOCX paragraphs/tables and XLSX cells directly through the Host file service without a shell, Python or sandbox process. file_path may be a workspace file or an exact file admitted to this session, including an extensionless upload path. Positions, table_index and start_row are one-based; row_limit defaults to 200 and selects physical row positions. XLSX returns original rawValue, storedValue, cell type, address and numberFormat separately; displayValueKind identifies literal text, simple zero-padding, or raw values whose format was not rendered. Formulas are never evaluated; their existing cached values are labeled. Text phone numbers and leading zeroes are preserved. Select sheet_name/table_index or a smaller range for large files. Macros, embedded executable objects and legacy DOC/XLS are unsupported."
            }.into(),
            parameters:parameters(write),
            output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:Some(Arc::new(|_,value|Ok(json!({"path":value["path"],"format":value["format"]}))))},
            timeout_ms:Some(60000),is_concurrency_safe:Some(Arc::new(move |_|!write)),finalize_content:None,
            present_call:Some(Arc::new(move |args|Some(ToolCallView::Generic{title:format!("{} Office {}",if write{"写入"}else{"读取"},args["file_path"].as_str().unwrap_or("")),kind:Some(if write{ToolCallKind::Edit}else{ToolCallKind::Read}),raw_input:None,content:None,locations:Some(vec![FileLocation{path:args["file_path"].as_str().unwrap_or("").into(),line:None}])}))),present_result:None,
            execute:Arc::new(move |args,run|{let context=context.clone();let args=args.clone();let run=run.clone();Box::pin(async move{execute(&context,&args,&run,write).await})}),
        })?;
    }
    Ok(())
}

async fn execute(
    ctx: &Context,
    args: &Value,
    run: &ToolRunContext,
    write: bool,
) -> Result<Value, ToolBodyError> {
    let execution = &run.execution;
    let agent = execution
        .agent
        .as_ref()
        .ok_or_else(|| error("Office file operations require a session"))?;
    let signal = execution.signal.lock().clone();
    if signal() {
        return Err(error("OFFICE_ABORTED: Office operation cancelled"));
    }
    let fs = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .ok_or_else(|| error("Office file service is unavailable"))?
        .as_ref()
        .clone();
    let fs = fs.for_tool(Some(agent.id().as_str())).unwrap_or(fs);
    let path = args["file_path"]
        .as_str()
        .ok_or_else(|| error("file_path is required"))?;
    let target = fs
        .resolve(
            path,
            Some(&ResolveOptions {
                cwd: agent.session().header().cwd.clone(),
                signal: Some(signal.clone()),
            }),
        )
        .await
        .map_err(fs_error)?;
    if !write {
        let data = fs
            .read_bytes(&target, Some(signal.clone()), codec::MAX_BYTES)
            .await
            .map_err(fs_error)?;
        let digest = format!("{:x}", Sha256::digest(&data));
        let count = data.len();
        let options = args.clone();
        let abort = signal.clone();
        let mut value =
            tokio::task::spawn_blocking(move || codec::read(&data, &options, abort.as_ref()))
                .await
                .map_err(|failure| error(failure.to_string()))?
                .map_err(error)?;
        if signal() {
            return Err(error("OFFICE_ABORTED: Office read cancelled"));
        }
        value["path"] = json!(target.display_path);
        value["bytes"] = json!(count);
        value["sha256"] = json!(digest);
        return Ok(value);
    }
    let format = args["format"]
        .as_str()
        .ok_or_else(|| error("format is required"))?;
    if !matches!(format, "docx" | "xlsx")
        || Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case(format))
    {
        return Err(error(
            "OFFICE_FORMAT_UNSUPPORTED: output extension must match docx or xlsx",
        ));
    }
    fs.authorize_write(&target).await.map_err(fs_error)?;
    let before = fs
        .stat(&target, Some(signal.clone()))
        .await
        .map_err(fs_error)?;
    if before
        .as_ref()
        .is_some_and(|info| info.kind != FsInfoType::File)
    {
        return Err(error("Office output must be a regular file"));
    }
    if before.is_some() && args["overwrite"] != true {
        return Err(ToolBodyError::coded(
            "Output already exists. Choose a new file path or request overwrite=true for explicit approval.",
            "OfficeDocumentError",
            "OFFICE_OUTPUT_EXISTS",
        ));
    }
    let expected = before
        .as_ref()
        .map(|info| FsWriteIntent::ReplaceIfVersion {
            version: info.version.clone(),
        })
        .unwrap_or(FsWriteIntent::CreateIfAbsent);
    let mut policy = ctx
        .get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
        .ok_or_else(|| error("Office output requires a resolved file permission policy"))?
        .try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
            session: Some(Arc::new(agent.session().clone())),
            mode: None,
        })
        .map_err(error)?;
    let temporary_write = policy.mode == dsh_sandbox::SandboxMode::ReadOnly;
    if (before.is_some() || temporary_write) && !run.human_approval_granted() {
        let approval = ctx
            .get_typed::<Arc<dsh_user_approval::ApprovalService>>("approval", false)
            .ok_or_else(|| error("Explicit Office write approval is unavailable"))?;
        let outcome = approval
            .request_human(&dsh_user_approval::ApprovalRequest {
                agent: agent.clone(),
                tool_name: "office_write".into(),
                call_id: Some(execution.call_id.as_str().into()),
                reason: Some(format!(
                    "{} Office 文件，仅授权此路径本次写入：{}",
                    if before.is_some() {
                        "替换现有"
                    } else {
                        "创建"
                    },
                    target.display_path
                )),
                grant_key: None,
                rememberable: false,
                signal: Some(signal.clone()),
            })
            .await
            .map_err(error)?;
        if !matches!(
            outcome,
            dsh_user_approval::ApprovalOutcome::AllowedOnce
                | dsh_user_approval::ApprovalOutcome::AllowedAlways
        ) {
            return Err(ToolBodyError::coded(
                format!("Office write was not approved: {}", outcome.as_str()),
                "ApprovalError",
                match outcome {
                    dsh_user_approval::ApprovalOutcome::Cancelled => "USER_APPROVAL_CANCELLED",
                    dsh_user_approval::ApprovalOutcome::TimedOut => "USER_APPROVAL_TIMED_OUT",
                    dsh_user_approval::ApprovalOutcome::Unavailable => "USER_APPROVAL_UNAVAILABLE",
                    _ => "USER_APPROVAL_DENIED",
                },
            ));
        }
    }
    if temporary_write {
        policy.mode = dsh_sandbox::SandboxMode::WorkspaceWrite;
        // This exact-file root is never stored in the session or sent to a shell.
        policy.workspace_root = fs.process_path(&target);
    }
    let spec = args.clone();
    let abort = signal.clone();
    let (bytes, mut summary) =
        tokio::task::spawn_blocking(move || codec::write(&spec, abort.as_ref()))
            .await
            .map_err(|failure| error(failure.to_string()))?
            .map_err(error)?;
    if signal() {
        return Err(error(
            "OFFICE_ABORTED: Office write cancelled before publication",
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    run.bind_execution_context()(json!({"adapter":"host-office-document","path":target.display_path,"format":format,"effectiveFileMode":policy.mode.as_str(),"temporaryWriteApproval":temporary_write})).map_err(error)?;
    run.track_cancellable_effects()().map_err(error)?;
    let outcome = fs
        .write_bytes(
            &target,
            &bytes,
            Some(&expected),
            Some(signal.clone()),
            Some(&policy),
        )
        .await
        .map_err(fs_error)?;
    summary["path"] = json!(target.display_path);
    summary["bytes"] = json!(outcome.bytes);
    summary["sha256"] = json!(digest);
    summary["operation"] = json!(match outcome.operation {
        dsh_fs::FsWriteOperation::Create => "created",
        dsh_fs::FsWriteOperation::Update => "replaced",
    });
    summary["version"] = json!(outcome.version.as_str());
    Ok(summary)
}
