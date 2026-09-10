//! Explicit, session-owned delivery of existing workspace files.
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, downcast_arc};
use dsh_fs::{FileSystem, FsInfoType, FsPathInfoType, LstatOptions, ResolveOptions};
use dsh_session::{Session, SessionEvent};
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolExecution, ToolExecutionResult, ToolOutputDefinition,
    ToolRuntime,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

const MAX_FILES: usize = 8;
#[cfg(test)]
#[path = "tool_present_tests.rs"]
mod tests;
struct Delivery {
    session: Session,
    turn: u64,
    files: Value,
}
type Pending = Arc<Mutex<HashMap<u64, Delivery>>>;

fn failure(message: impl Into<String>) -> ToolBodyError {
    ToolBodyError {
        message: message.into(),
        info: None,
    }
}

fn open_turn(events: &[SessionEvent]) -> Option<u64> {
    for event in events.iter().rev() {
        match event.type_.as_str() {
            "turn/end" => return None,
            "turn/start" => return event.data["turn"].as_u64().filter(|turn| *turn > 0),
            _ => {}
        }
    }
    None
}

async fn existing_files(
    fs: &dyn FileSystem,
    cwd: &str,
    files: &Value,
    signal: dsh_tools::AbortPredicate,
) -> Result<Value, ToolBodyError> {
    let files = files
        .as_array()
        .filter(|files| !files.is_empty() && files.len() <= MAX_FILES)
        .ok_or_else(|| failure(format!("present accepts 1 to {MAX_FILES} files")))?;
    let options = ResolveOptions {
        cwd: Some(cwd.into()),
        signal: Some(signal.clone()),
    };
    let root = fs
        .resolve(cwd, Some(&options))
        .await
        .map_err(|error| failure(error.to_string()))?;
    for file in files {
        if signal() {
            return Err(failure("present cancelled"));
        }
        let path = file["path"]
            .as_str()
            .filter(|path| {
                !path.trim().is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
            })
            .ok_or_else(|| failure("present requires a non-empty file path"))?;
        if file
            .get("description")
            .is_some_and(|value| !value.as_str().is_some_and(|text| text.len() <= 4096))
        {
            return Err(failure(
                "present description must be text of at most 4096 bytes",
            ));
        }
        let entry = fs
            .lstat(
                path,
                Some(&LstatOptions {
                    cwd: Some(cwd.into()),
                }),
                Some(signal.clone()),
            )
            .await
            .map_err(|error| failure(error.to_string()))?;
        if entry.is_some_and(|entry| entry.kind != FsPathInfoType::File) {
            return Err(failure(format!(
                "Cannot present {path}: not a regular file"
            )));
        }
        let target = fs
            .resolve(path, Some(&options))
            .await
            .map_err(|error| failure(error.to_string()))?;
        if !fs.contains(&root, &target) {
            return Err(failure(format!(
                "Cannot present {path}: outside this Session workspace"
            )));
        }
        let info = fs
            .stat(&target, Some(signal.clone()))
            .await
            .map_err(|error| failure(error.to_string()))?
            .ok_or_else(|| failure(format!("Cannot present {path}: file not found")))?;
        if info.kind != FsInfoType::File {
            return Err(failure(format!(
                "Cannot present {path}: not a regular file"
            )));
        }
    }
    if signal() {
        return Err(failure("present cancelled"));
    }
    Ok(Value::Array(files.clone()))
}

pub struct PresentPlugin;
#[async_trait::async_trait]
impl Plugin for PresentPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("tool-present")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["tools", "fs", "systemPrompt"])
    }
    async fn apply(&self, ctx: &Context, _: ArcValue) -> Result<(), PluginError> {
        let install = async {
            let fs = ctx.get_typed::<Arc<dyn FileSystem>>("fs",false).map(|value|value.as_ref().clone()).ok_or("present requires filesystem")?;
            let tools = ctx.get_typed::<Arc<ToolRuntime>>("tools",false).map(|value|value.as_ref().clone()).ok_or("present requires tools")?;
            let pending: Pending = Default::default();
            let for_execute = pending.clone();
            let file_schema=json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"description":{"type":"string"}},"required":["path"]});
            tools.register(ctx, ToolDefinition {
                name:"present".into(),
                description:"Declare existing regular files in this Session workspace as final deliverables. Call present after writing requested outputs and before the final response, including files created through shell or code execution. File contents are not copied or frozen.".into(),
                parameters:json!({"type":"object","additionalProperties":false,"properties":{"files":{"type":"array","items":file_schema.clone()}},"required":["files"]}),
                output:ToolOutputDefinition {
                    schema:json!({"type":"object","additionalProperties":false,"properties":{"turn":{"type":"integer"},"files":{"type":"array","items":file_schema}},"required":["turn","files"]}),
                    render:Arc::new(|_,value| Ok(vec![dsh_llm::ContentBlock::Text {text:value["files"].as_array().unwrap().iter().map(|file|format!("Presented {}",file["path"].as_str().unwrap())).collect::<Vec<_>>().join("\n")}])),
                    presentation_meta:None,
                },
                timeout_ms:Some(30_000),is_concurrency_safe:None,finalize_content:None,present_call:None,present_result:None,
                execute:Arc::new(move |arguments, run| {
                    let fs=fs.clone(); let pending=for_execute.clone();
                    let arguments=arguments.clone(); let execution=run.execution.clone();
                    Box::pin(async move {
                        let agent=execution.agent.as_ref().ok_or_else(||failure("present requires an agent Session"))?;
                        let session=agent.session().clone();
                        let turn=session.with_events(open_turn).ok_or_else(||failure("present requires an open turn"))?;
                        let cwd=session.header().cwd.as_deref().ok_or_else(||failure("present requires a workspace"))?;
                        let signal=execution.signal.lock().clone();
                        let files=existing_files(fs.as_ref(),cwd,&arguments["files"],signal).await?;
                        let value=json!({"turn":turn,"files":files});
                        pending.lock().insert(execution.token,Delivery {session,turn,files:value["files"].clone()});
                        Ok(value)
                    })
                }),
            })?;
            ctx.on("tools/result",Arc::new(move |listener_ctx,args| {
                let listener_ctx=listener_ctx.clone();
                let pending=pending.clone();
                let execution=args.first().and_then(downcast_arc::<Arc<ToolExecution>>).map(|value|value.as_ref().clone());
                let result=args.get(1).and_then(downcast_arc::<Arc<ToolExecutionResult>>).map(|value|value.as_ref().clone());
                Box::pin(async move {
                    if let (Some(execution),Some(result))=(execution,result) {
                        let delivery=pending.lock().remove(&execution.token);
                        if let Some(delivery)=delivery.filter(|_|!result.is_error && !(execution.signal.lock())()) {
                            if let Err(error)=delivery.session.append_if("deliverables/presented",json!({"turn":delivery.turn,"callId":execution.call_id,"files":delivery.files}),None,|events|open_turn(events)==Some(delivery.turn)) {
                                listener_ctx.named_logger(Some("present")).warn(vec![cordis::arc(format!("Could not record file delivery: {error}"))]);
                            }
                        }
                    }
                    None
                })
            }),cordis::EventOptions::default()).await;
            if let Some(prompt)=ctx.get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt",false) {
                prompt.section(ctx,dsh_system_prompt::PromptSection {name:"tool:present".into(),order:107.0,complete:None,text:dsh_tools::scoped_tool_guidance(ctx,&["present"],"When files are requested outputs, call present with the existing primary output files before the final answer. Merely mentioning a path does not declare a delivery.")});
            }
            Ok::<(),String>(())
        }.await;
        install.map_err(|error| PluginError::from(anyhow::anyhow!(error)))
    }
}
